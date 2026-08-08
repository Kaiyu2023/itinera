//! Plan v1 initialization and bounded version/current-plan reads.

use std::collections::{HashMap, HashSet};

use chrono::DateTime;
use futures_util::TryStreamExt;
use itinera_core::{
    domain::{
        proposal::{ChangeOp, Proposal, ProposalStatus},
        trip::{
            CandidateStatus, Day, DayFeasibility, Feasibility, Plan, PlanDetail, Stop, Trip,
            TripStatus,
        },
    },
    ports::{authorization::TripAuthorizationContext, trip::TripRepoError},
    services::{plans::validate_stored_plan_graph, proposals::replay_stored_change_set},
};
use sqlx::{Sqlite, Transaction};

use crate::sqlite::{
    SqliteDb,
    codec::{checked_revision, next_revision, sha256_hex, validate_id},
};

use super::{
    access::{
        RequiredRole, authorize, load_members_and_validate_capacity, load_trip, member_values,
    },
    candidate_records::encoded_size,
    candidates::{load_candidates, load_place},
    plan_records::{
        DayRow, MAX_PLAN_PROVENANCE_BYTES, MAX_PLAN_RESPONSE_BYTES, MAX_PLAN_VERSIONS,
        PLAN_VERSION_QUERY_LIMIT, PlanRow, PlanStructureRow, PlanStructureValue, StopRow,
        StoredPlan, StructuralAuditBinding,
    },
    records::TripRow,
};

const MAX_PLAN_GRAPH_ITEMS: usize = 100;
const PLAN_GRAPH_QUERY_LIMIT: i64 = 101;
const MAX_LINEAGE_PLACE_IDS: usize = 100_000;
const LINEAGE_PLACE_ID_QUERY_LIMIT: i64 = 100_001;
const MAX_LINEAGE_PLACE_ID_BYTES: usize = 32 * 1024 * 1024;
const MAX_LINEAGE_STRUCTURE_BYTES: usize = 768 * 1024 * 1024;

pub(super) async fn get_current_plan(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<PlanDetail, TripRepoError> {
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::AnyMember,
    )
    .await?;
    let (trip_row, _) = load_validated_trip(&mut transaction, trip_id).await?;
    let (plan_id, version) = owned_pointer(&trip_row)?.ok_or(TripRepoError::NotFound)?;
    let detail = load_plan_detail(&mut transaction, trip_id, &plan_id, version).await?;
    validate_plan_poll_provenance(&mut transaction, &detail.plan).await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(detail)
}

pub(super) async fn initialize_plan(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    anchor_place_id: &str,
    plan: Plan,
    days: Vec<Day>,
) -> Result<PlanDetail, TripRepoError> {
    validate_id(anchor_place_id).map_err(corrupt)?;
    validate_plan_v1_input(trip_id, &plan, &days)?;

    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Editor,
    )
    .await?;
    let (trip_row, trip) = load_validated_trip(&mut transaction, trip_id).await?;
    if let Some((current_id, current_version)) = owned_pointer(&trip_row)? {
        let existing =
            load_plan_detail(&mut transaction, trip_id, &current_id, current_version).await?;
        validate_plan_poll_provenance(&mut transaction, &existing.plan).await?;
        db.commit(transaction).await.map_err(unavailable)?;
        return Ok(existing);
    }

    let existing_plan_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM plans WHERE trip_id = ?")
            .bind(trip_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(unavailable)?;
    if existing_plan_count != 0 {
        return Err(TripRepoError::CorruptData);
    }

    let anchor = load_candidates(&mut transaction, trip_id)
        .await?
        .into_iter()
        .find(|candidate| {
            candidate.candidate.place_id == anchor_place_id
                && candidate.candidate.status == CandidateStatus::Shortlisted
        })
        .ok_or(TripRepoError::NotFound)?;
    validate_bootstrap_days(&trip, &plan, &days, &anchor.place.city, &anchor.place.tz)?;

    let projected_plans = vec![plan.clone()];
    if projected_plans.len() > MAX_PLAN_VERSIONS
        || encoded_size(&projected_plans)? > MAX_PLAN_RESPONSE_BYTES
    {
        return Err(TripRepoError::Conflict);
    }

    let structure_hash = plan_structure_hash(&plan, &days, &[])?;
    sqlx::query(
        "INSERT INTO plans ( \
             trip_id, version, id, created_from_proposal_id, created_at, \
             structure_hash, revision \
         ) VALUES (?, 1, ?, NULL, ?, ?, 1)",
    )
    .bind(trip_id)
    .bind(&plan.id)
    .bind(&plan.created_at)
    .bind(structure_hash)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    for day in &days {
        sqlx::query(
            "INSERT INTO plan_days ( \
                 trip_id, plan_version, id, plan_id, date, city_hint, tz, \
                 window_start, window_end, revision \
             ) VALUES (?, 1, ?, ?, ?, ?, ?, ?, ?, 1)",
        )
        .bind(trip_id)
        .bind(&day.id)
        .bind(&day.plan_id)
        .bind(&day.date)
        .bind(&day.city_hint)
        .bind(&day.tz)
        .bind(&day.window_start)
        .bind(&day.window_end)
        .execute(&mut *transaction)
        .await
        .map_err(unavailable)?;
    }

    let current_revision = trip_row.revision()?;
    let next = next_revision(current_revision).map_err(corrupt)?;
    let updated = sqlx::query(
        "UPDATE trips \
         SET current_plan_id = ?, current_plan_version = 1, status = ?, revision = ? \
         WHERE id = ? AND revision = ? \
           AND current_plan_id IS NULL AND current_plan_version IS NULL",
    )
    .bind(&plan.id)
    .bind(TripStatus::Planning.as_ref())
    .bind(next)
    .bind(trip_id)
    .bind(current_revision)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    if updated.rows_affected() != 1 {
        return Err(TripRepoError::Conflict);
    }

    let detail = load_plan_detail(&mut transaction, trip_id, &plan.id, 1).await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(detail)
}

pub(super) async fn list_plan_versions(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<Plan>, TripRepoError> {
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::AnyMember,
    )
    .await?;
    let (trip_row, _) = load_validated_trip(&mut transaction, trip_id).await?;
    let plans = load_plan_versions(&mut transaction, trip_id).await?;
    validate_plan_lineage_poll_provenance(&mut transaction, trip_id, &plans).await?;
    match (plans.last(), owned_pointer(&trip_row)?) {
        (None, None) => {}
        (Some(latest), Some((id, version))) if latest.id == id && latest.version == version => {}
        _ => return Err(TripRepoError::CorruptData),
    }
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(plans)
}

pub(in crate::sqlite) async fn load_plan_detail(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    plan_id: &str,
    version: u32,
) -> Result<PlanDetail, TripRepoError> {
    let (plan, days, stops) = load_plan_graph(transaction, trip_id, plan_id, version).await?;
    let mut place_ids = HashSet::new();
    let mut places = Vec::new();
    for stop in &stops {
        if place_ids.insert(stop.place_id.as_str()) {
            places.push(
                load_place(transaction, trip_id, &stop.place_id)
                    .await?
                    .ok_or(TripRepoError::CorruptData)?,
            );
        }
    }
    let day_feasibility = derive_day_feasibility(&days, &stops)?;
    let detail = PlanDetail {
        plan,
        days,
        stops,
        legs: Vec::new(),
        day_feasibility,
        places,
    };
    if encoded_size(&detail)? > MAX_PLAN_RESPONSE_BYTES {
        return Err(TripRepoError::CorruptData);
    }
    Ok(detail)
}

async fn load_plan_graph(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    plan_id: &str,
    version: u32,
) -> Result<(Plan, Vec<Day>, Vec<Stop>), TripRepoError> {
    let row = sqlx::query_as::<_, PlanRow>(
        "SELECT trip_id AS plan_trip_id, version AS plan_version, id AS plan_id, \
                created_from_proposal_id, created_at AS plan_created_at, \
                applied_change_set_json, application_entity_ids_json, \
                structural_audits_json, \
                base_structure_hash, structure_hash, \
                revision AS plan_revision \
         FROM plans WHERE trip_id = ? AND id = ? AND version = ?",
    )
    .bind(trip_id)
    .bind(plan_id)
    .bind(i64::from(version))
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?
    .ok_or(TripRepoError::CorruptData)?;
    let stored_plan = row.into_stored_plan(trip_id)?;
    validate_plan_provenance(transaction, &stored_plan).await?;
    let plan = stored_plan.value.clone();

    let day_rows = sqlx::query_as::<_, DayRow>(
        "SELECT trip_id AS day_trip_id, plan_version AS day_plan_version, \
                id AS day_id, plan_id AS day_plan_id, date AS day_date, \
                city_hint AS day_city_hint, tz AS day_tz, \
                window_start AS day_window_start, window_end AS day_window_end, \
                revision AS day_revision \
         FROM plan_days \
         WHERE trip_id = ? AND plan_version = ? \
         ORDER BY date, id LIMIT ?",
    )
    .bind(trip_id)
    .bind(i64::from(version))
    .bind(PLAN_GRAPH_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if day_rows.is_empty() || day_rows.len() > MAX_PLAN_GRAPH_ITEMS {
        return Err(TripRepoError::CorruptData);
    }
    let days = day_rows
        .into_iter()
        .map(|row| row.into_day(trip_id, version))
        .collect::<Result<Vec<_>, _>>()?;

    let stored_stop_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM plan_stops WHERE trip_id = ? AND plan_version = ?",
    )
    .bind(trip_id)
    .bind(i64::from(version))
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let stored_stop_count = usize::try_from(stored_stop_count).map_err(corrupt)?;
    if stored_stop_count > MAX_PLAN_GRAPH_ITEMS {
        return Err(TripRepoError::CorruptData);
    }
    let stop_rows = sqlx::query_as::<_, StopRow>(
        "SELECT s.trip_id AS stop_trip_id, s.plan_version AS stop_plan_version, \
                s.id AS stop_id, s.day_id AS stop_day_id, s.seq AS stop_seq, \
                s.place_id AS stop_place_id, s.stop_kind, \
                s.planned_arrival AS stop_planned_arrival, \
                s.duration_min AS stop_duration_min, s.booking_ref, s.booking_url, \
                s.booking_cost_amount, s.booking_cost_currency, \
                s.notes AS stop_notes, s.revision AS stop_revision \
         FROM plan_stops AS s \
         JOIN stop_identities AS identity \
           ON identity.trip_id = s.trip_id AND identity.id = s.id \
         WHERE s.trip_id = ? AND s.plan_version = ? \
         ORDER BY s.day_id, s.seq, s.id LIMIT ?",
    )
    .bind(trip_id)
    .bind(i64::from(version))
    .bind(PLAN_GRAPH_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if stop_rows.len() != stored_stop_count {
        return Err(TripRepoError::CorruptData);
    }
    let stops = stop_rows
        .into_iter()
        .map(|row| row.into_stop(trip_id, version))
        .collect::<Result<Vec<_>, _>>()?;
    validate_stored_plan_graph(&plan, &days, &stops, trip_id, version).map_err(corrupt)?;
    let structure_hash = plan_structure_hash(&plan, &days, &stops)?;
    if stored_plan
        .structure_hash
        .as_deref()
        .is_some_and(|stored| stored != structure_hash)
    {
        return Err(TripRepoError::CorruptData);
    }
    Ok((plan, days, stops))
}

pub(in crate::sqlite) async fn load_plan_head(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Option<Plan>, TripRepoError> {
    let (count, minimum, maximum, applied_count): (i64, Option<i64>, Option<i64>, i64) =
        sqlx::query_as(
            "SELECT COUNT(*), MIN(version), MAX(version), \
                    (SELECT COUNT(*) FROM proposals \
                     WHERE trip_id = ? AND status = 'applied') \
             FROM plans WHERE trip_id = ?",
        )
        .bind(trip_id)
        .bind(trip_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(unavailable)?;
    let count = usize::try_from(count).map_err(corrupt)?;
    let applied_count = usize::try_from(applied_count).map_err(corrupt)?;
    if count == 0 {
        return if minimum.is_none() && maximum.is_none() && applied_count == 0 {
            Ok(None)
        } else {
            Err(TripRepoError::CorruptData)
        };
    }
    let maximum = maximum
        .and_then(|value| u32::try_from(value).ok())
        .filter(|version| usize::try_from(*version).ok() == Some(count))
        .ok_or(TripRepoError::CorruptData)?;
    if minimum != Some(1) || count > MAX_PLAN_VERSIONS || applied_count != count.saturating_sub(1) {
        return Err(TripRepoError::CorruptData);
    }
    let row = sqlx::query_as::<_, PlanRow>(
        "SELECT trip_id AS plan_trip_id, version AS plan_version, id AS plan_id, \
                created_from_proposal_id, created_at AS plan_created_at, \
                applied_change_set_json, application_entity_ids_json, \
                structural_audits_json, base_structure_hash, structure_hash, \
                revision AS plan_revision \
         FROM plans WHERE trip_id = ? AND version = ?",
    )
    .bind(trip_id)
    .bind(i64::from(maximum))
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let stored = row.into_stored_plan(trip_id)?;
    let (actual_plan, actual_days, actual_stops) =
        load_plan_graph(transaction, trip_id, &stored.value.id, maximum).await?;
    if actual_plan != stored.value {
        return Err(TripRepoError::CorruptData);
    }
    if maximum == 1 {
        return Ok(Some(actual_plan));
    }
    let proposal_id = actual_plan
        .created_from_proposal_id
        .as_deref()
        .ok_or(TripRepoError::CorruptData)?;
    let proposal =
        crate::sqlite::proposal_repo::operations::load_proposal(transaction, trip_id, proposal_id)
            .await
            .map_err(map_proposal_error)?
            .ok_or(TripRepoError::CorruptData)?;
    let encoded_change_set = serde_json::to_string(&proposal.value.change_set).map_err(corrupt)?;
    if proposal.value.status != ProposalStatus::Applied
        || proposal.value.change_set.base_plan_version.checked_add(1) != Some(maximum)
        || stored.applied_change_set_json.as_deref() != Some(encoded_change_set.as_str())
    {
        return Err(TripRepoError::CorruptData);
    }
    let (base_plan_id, base_stored_hash): (String, Option<String>) =
        sqlx::query_as("SELECT id, structure_hash FROM plans WHERE trip_id = ? AND version = ?")
            .bind(trip_id)
            .bind(i64::from(maximum - 1))
            .fetch_one(&mut **transaction)
            .await
            .map_err(unavailable)?;
    let (base_plan, base_days, base_stops) =
        load_plan_graph(transaction, trip_id, &base_plan_id, maximum - 1).await?;
    let applied_at = plan_timestamp(&actual_plan.created_at)?;
    if applied_at < plan_timestamp(&proposal.value.created_at)?
        || applied_at < plan_timestamp(&base_plan.created_at)?
    {
        return Err(TripRepoError::CorruptData);
    }
    let base_hash = plan_structure_hash(&base_plan, &base_days, &base_stops)?;
    if base_stored_hash.as_deref() != Some(base_hash.as_str())
        || stored.base_structure_hash.as_deref() != Some(base_hash.as_str())
    {
        return Err(TripRepoError::CorruptData);
    }
    let known_place_ids = validate_head_place_ids(
        transaction,
        trip_id,
        base_stops
            .iter()
            .chain(&actual_stops)
            .map(|stop| &stop.place_id),
        &proposal.value.change_set.ops,
        &stored.application_entity_ids,
    )
    .await?;
    let application = replay_stored_change_set(
        &PlanDetail {
            plan: base_plan,
            days: base_days,
            stops: base_stops,
            legs: Vec::new(),
            day_feasibility: Vec::new(),
            places: Vec::new(),
        },
        &known_place_ids,
        &actual_plan,
        proposal_id,
        &proposal.value.change_set,
        &stored.application_entity_ids,
    )
    .map_err(corrupt)?;
    let replayed_hash =
        plan_structure_hash(&application.plan, &application.days, &application.stops)?;
    let actual_hash = plan_structure_hash(&actual_plan, &actual_days, &actual_stops)?;
    if replayed_hash != actual_hash
        || stored.structure_hash.as_deref() != Some(actual_hash.as_str())
    {
        return Err(TripRepoError::CorruptData);
    }
    Ok(Some(actual_plan))
}

async fn validate_head_place_ids<'a>(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    place_ids: impl Iterator<Item = &'a String>,
    operations: &[ChangeOp],
    application_entity_ids: &[String],
) -> Result<HashSet<String>, TripRepoError> {
    let mut place_ids = place_ids.cloned().collect::<HashSet<_>>();
    let mut entity_index = 0;
    for operation in operations {
        match operation {
            ChangeOp::AddStop { place_id, .. } => {
                place_ids.insert(place_id.clone());
                entity_index += 1;
            }
            ChangeOp::SwapPlace { new_place_id, .. } => {
                place_ids.insert(new_place_id.clone());
            }
            ChangeOp::AddPlaceStop { .. } => {
                // A generated place is persisted only when its generated stop
                // survives the complete ChangeSet. Surviving generated places
                // are already present in `actual_stops`; transient ones must
                // not be required to have a `trip_places` row.
                entity_index += 2;
            }
            ChangeOp::AddDay { .. } => entity_index += 1,
            ChangeOp::RemoveStop { .. }
            | ChangeOp::MoveStop { .. }
            | ChangeOp::Reorder { .. }
            | ChangeOp::RemoveDay { .. } => {}
        }
    }
    if entity_index != application_entity_ids.len()
        || place_ids.len() > MAX_PLAN_GRAPH_ITEMS.saturating_mul(2).saturating_add(40)
    {
        return Err(TripRepoError::CorruptData);
    }
    for place_id in &place_ids {
        validate_id(place_id).map_err(corrupt)?;
    }
    let encoded_ids = serde_json::to_string(&place_ids).map_err(corrupt)?;
    let stored_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trip_places WHERE trip_id = ? \
         AND id IN (SELECT CAST(value AS TEXT) FROM json_each(?))",
    )
    .bind(trip_id)
    .bind(encoded_ids)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if usize::try_from(stored_count).ok() != Some(place_ids.len()) {
        return Err(TripRepoError::CorruptData);
    }
    Ok(place_ids)
}

pub(in crate::sqlite) async fn validate_plan_metadata_poll_lineage(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<(), TripRepoError> {
    let rows = sqlx::query_as::<_, (String, i64, String, Option<String>, String, i64)>(
        "SELECT trip_id, version, id, created_from_proposal_id, created_at, revision \
         FROM plans WHERE trip_id = ? ORDER BY version LIMIT ?",
    )
    .bind(trip_id)
    .bind(PLAN_VERSION_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() > MAX_PLAN_VERSIONS {
        return Err(TripRepoError::CorruptData);
    }
    let plans = rows
        .into_iter()
        .enumerate()
        .map(
            |(index, (stored_trip_id, version, id, proposal_id, created_at, revision))| {
                let version = u32::try_from(version).map_err(corrupt)?;
                let expected_version = u32::try_from(index + 1).map_err(corrupt)?;
                checked_revision(revision).map_err(corrupt)?;
                validate_id(&id).map_err(corrupt)?;
                if stored_trip_id != trip_id
                    || version != expected_version
                    || proposal_id
                        .as_deref()
                        .is_some_and(|proposal_id| validate_id(proposal_id).is_err())
                    || (version == 1) != proposal_id.is_none()
                {
                    return Err(TripRepoError::CorruptData);
                }
                plan_timestamp(&created_at)?;
                Ok(Plan {
                    id,
                    trip_id: stored_trip_id,
                    version,
                    created_from_proposal_id: proposal_id,
                    created_at,
                })
            },
        )
        .collect::<Result<Vec<_>, _>>()?;
    if encoded_size(&plans)? > MAX_PLAN_RESPONSE_BYTES {
        return Err(TripRepoError::CorruptData);
    }
    validate_plan_lineage_poll_provenance(transaction, trip_id, &plans).await
}

pub(in crate::sqlite) async fn load_plan_versions(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<Plan>, TripRepoError> {
    Ok(load_stored_plan_versions(transaction, trip_id)
        .await?
        .into_iter()
        .map(|stored| stored.value)
        .collect())
}

async fn load_stored_plan_versions(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<StoredPlan>, TripRepoError> {
    let (stored_count, provenance_bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM( \
             length(CAST(COALESCE(applied_change_set_json, '') AS BLOB)) + \
             length(CAST(COALESCE(application_entity_ids_json, '') AS BLOB)) + \
             length(CAST(COALESCE(structural_audits_json, '') AS BLOB)) + \
             length(CAST(COALESCE(base_structure_hash, '') AS BLOB)) + \
             length(CAST(COALESCE(structure_hash, '') AS BLOB)) \
         ), 0) FROM plans WHERE trip_id = ?",
    )
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let stored_count = usize::try_from(stored_count).map_err(corrupt)?;
    let provenance_bytes = usize::try_from(provenance_bytes).map_err(corrupt)?;
    if stored_count > MAX_PLAN_VERSIONS || provenance_bytes > MAX_PLAN_PROVENANCE_BYTES {
        return Err(TripRepoError::CorruptData);
    }
    let rows = sqlx::query_as::<_, PlanRow>(
        "SELECT trip_id AS plan_trip_id, version AS plan_version, id AS plan_id, \
                created_from_proposal_id, created_at AS plan_created_at, \
                applied_change_set_json, application_entity_ids_json, \
                structural_audits_json, \
                base_structure_hash, structure_hash, \
                revision AS plan_revision \
         FROM plans WHERE trip_id = ? ORDER BY version LIMIT ?",
    )
    .bind(trip_id)
    .bind(PLAN_VERSION_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() != stored_count {
        return Err(TripRepoError::CorruptData);
    }
    let stored_plans = rows
        .into_iter()
        .map(|row| row.into_stored_plan(trip_id))
        .collect::<Result<Vec<_>, _>>()?;
    let plans = stored_plans
        .iter()
        .map(|stored| stored.value.clone())
        .collect::<Vec<_>>();
    if plans
        .iter()
        .enumerate()
        .any(|(index, plan)| usize::try_from(plan.version).ok() != Some(index + 1))
        || encoded_size(&plans)? > MAX_PLAN_RESPONSE_BYTES
    {
        return Err(TripRepoError::CorruptData);
    }
    let applied =
        crate::sqlite::proposal_repo::operations::load_applied_proposals(transaction, trip_id)
            .await
            .map_err(map_proposal_error)?;
    if applied.len() != plans.len().saturating_sub(1) {
        return Err(TripRepoError::CorruptData);
    }
    let applied_by_id = applied
        .iter()
        .map(|proposal| (proposal.value.id.as_str(), proposal))
        .collect::<HashMap<_, _>>();
    if applied_by_id.len() != applied.len() {
        return Err(TripRepoError::CorruptData);
    }
    if stored_plans.is_empty() {
        return if applied.is_empty() {
            Ok(stored_plans)
        } else {
            Err(TripRepoError::CorruptData)
        };
    }

    let known_place_ids = load_lineage_place_ids(transaction, trip_id).await?;

    let applied_by_id = applied_by_id
        .into_iter()
        .map(|(id, proposal)| (id, &proposal.value))
        .collect::<HashMap<_, _>>();
    validate_plan_lineage_graphs(
        transaction,
        trip_id,
        &stored_plans,
        &applied_by_id,
        &known_place_ids,
    )
    .await?;
    Ok(stored_plans)
}

async fn load_lineage_place_ids(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<HashSet<String>, TripRepoError> {
    let (place_count, place_id_bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(length(CAST(id AS BLOB))), 0) \
         FROM trip_places WHERE trip_id = ?",
    )
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let place_count = usize::try_from(place_count).map_err(corrupt)?;
    let place_id_bytes = usize::try_from(place_id_bytes).map_err(corrupt)?;
    if place_count > MAX_LINEAGE_PLACE_IDS || place_id_bytes > MAX_LINEAGE_PLACE_ID_BYTES {
        return Err(TripRepoError::CorruptData);
    }
    let place_ids = sqlx::query_scalar::<_, String>(
        "SELECT id FROM trip_places WHERE trip_id = ? ORDER BY id LIMIT ?",
    )
    .bind(trip_id)
    .bind(LINEAGE_PLACE_ID_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if place_ids.len() != place_count {
        return Err(TripRepoError::CorruptData);
    }
    let place_ids = place_ids.into_iter().collect::<HashSet<_>>();
    if place_ids.len() == place_count {
        Ok(place_ids)
    } else {
        Err(TripRepoError::CorruptData)
    }
}

async fn validate_plan_lineage_graphs(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    stored_plans: &[StoredPlan],
    applied_by_id: &HashMap<&str, &Proposal>,
    known_place_ids: &HashSet<String>,
) -> Result<(), TripRepoError> {
    let (day_count, stop_count, structural_bytes): (i64, i64, i64) = sqlx::query_as(
        "SELECT \
             (SELECT COUNT(*) FROM plan_days WHERE trip_id = ?), \
             (SELECT COUNT(*) FROM plan_stops WHERE trip_id = ?), \
             COALESCE(( \
                 SELECT SUM( \
                     length(CAST(trip_id AS BLOB)) + length(CAST(id AS BLOB)) + \
                     length(CAST(plan_id AS BLOB)) + length(CAST(date AS BLOB)) \
                 ) FROM plan_days WHERE trip_id = ? \
             ), 0) + COALESCE(( \
                 SELECT SUM( \
                     length(CAST(trip_id AS BLOB)) + \
                     (2 * length(CAST(id AS BLOB))) + \
                     length(CAST(day_id AS BLOB)) + length(CAST(place_id AS BLOB)) + \
                     length(CAST(stop_kind AS BLOB)) \
                 ) FROM plan_stops WHERE trip_id = ? \
             ), 0)",
    )
    .bind(trip_id)
    .bind(trip_id)
    .bind(trip_id)
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let day_count = usize::try_from(day_count).map_err(corrupt)?;
    let stop_count = usize::try_from(stop_count).map_err(corrupt)?;
    let structural_bytes = usize::try_from(structural_bytes).map_err(corrupt)?;
    let maximum_rows = stored_plans
        .len()
        .checked_mul(MAX_PLAN_GRAPH_ITEMS)
        .ok_or(TripRepoError::CorruptData)?;
    if day_count < stored_plans.len()
        || day_count > maximum_rows
        || stop_count > maximum_rows
        || structural_bytes > MAX_LINEAGE_STRUCTURE_BYTES
    {
        return Err(TripRepoError::CorruptData);
    }
    let mut rows = sqlx::query_as::<_, PlanStructureRow>(
        "SELECT d.trip_id AS structure_trip_id, \
                d.plan_version AS structure_plan_version, \
                0 AS structure_kind, d.id AS structure_id, \
                d.plan_id AS structure_parent_id, d.date AS day_date, \
                NULL AS stop_seq, NULL AS stop_place_id, NULL AS stop_kind, \
                NULL AS stop_identity_id, d.revision AS structure_revision \
         FROM plan_days AS d WHERE d.trip_id = ? \
         UNION ALL \
         SELECT s.trip_id AS structure_trip_id, \
                s.plan_version AS structure_plan_version, \
                1 AS structure_kind, s.id AS structure_id, \
                s.day_id AS structure_parent_id, NULL AS day_date, \
                s.seq AS stop_seq, s.place_id AS stop_place_id, \
                s.stop_kind, identity.id AS stop_identity_id, \
                s.revision AS structure_revision \
         FROM plan_stops AS s \
         LEFT JOIN stop_identities AS identity \
           ON identity.trip_id = s.trip_id AND identity.id = s.id \
         WHERE s.trip_id = ? \
         ORDER BY structure_plan_version, structure_kind, day_date, \
                  structure_parent_id, stop_seq, structure_id",
    )
    .bind(trip_id)
    .bind(trip_id)
    .fetch(&mut **transaction);

    let mut processed = 0_usize;
    let mut active_version = None;
    let mut days = Vec::new();
    let mut stops = Vec::new();
    let mut current = None;
    let mut current_hash = None;
    let mut linked_proposals = HashSet::new();
    while let Some(row) = rows.try_next().await.map_err(unavailable)? {
        let version = row.plan_version()?;
        if active_version.is_some_and(|active| active != version) {
            validate_lineage_version(
                stored_plans,
                applied_by_id,
                known_place_ids,
                processed,
                std::mem::take(&mut days),
                std::mem::take(&mut stops),
                &mut current,
                &mut current_hash,
                &mut linked_proposals,
            )?;
            processed += 1;
        }
        if version != u32::try_from(processed + 1).map_err(corrupt)? {
            return Err(TripRepoError::CorruptData);
        }
        active_version = Some(version);
        match row.into_value(trip_id, version)? {
            PlanStructureValue::Day(day) if days.len() < MAX_PLAN_GRAPH_ITEMS => days.push(day),
            PlanStructureValue::Stop(stop) if stops.len() < MAX_PLAN_GRAPH_ITEMS => {
                stops.push(stop);
            }
            PlanStructureValue::Day(_) | PlanStructureValue::Stop(_) => {
                return Err(TripRepoError::CorruptData);
            }
        }
    }
    drop(rows);
    if active_version.is_some() {
        validate_lineage_version(
            stored_plans,
            applied_by_id,
            known_place_ids,
            processed,
            days,
            stops,
            &mut current,
            &mut current_hash,
            &mut linked_proposals,
        )?;
        processed += 1;
    }
    if processed != stored_plans.len() || linked_proposals.len() != applied_by_id.len() {
        return Err(TripRepoError::CorruptData);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_lineage_version(
    stored_plans: &[StoredPlan],
    applied_by_id: &HashMap<&str, &Proposal>,
    known_place_ids: &HashSet<String>,
    index: usize,
    days: Vec<Day>,
    stops: Vec<Stop>,
    current: &mut Option<PlanDetail>,
    current_hash: &mut Option<String>,
    linked_proposals: &mut HashSet<String>,
) -> Result<(), TripRepoError> {
    let stored_plan = stored_plans.get(index).ok_or(TripRepoError::CorruptData)?;
    let plan = &stored_plan.value;
    validate_stored_plan_graph(plan, &days, &stops, &plan.trip_id, plan.version)
        .map_err(corrupt)?;
    if stops
        .iter()
        .any(|stop| !known_place_ids.contains(&stop.place_id))
    {
        return Err(TripRepoError::CorruptData);
    }
    let actual_hash = plan_structure_hash(plan, &days, &stops)?;
    if stored_plan
        .structure_hash
        .as_deref()
        .is_some_and(|stored| stored != actual_hash)
    {
        return Err(TripRepoError::CorruptData);
    }
    match (plan.version, plan.created_from_proposal_id.as_deref()) {
        (1, None) if index == 0 && current.is_none() => {
            *current_hash = Some(actual_hash);
            *current = Some(PlanDetail {
                plan: plan.clone(),
                days,
                stops,
                legs: Vec::new(),
                day_feasibility: Vec::new(),
                places: Vec::new(),
            });
        }
        (1, _) | (_, None) => return Err(TripRepoError::CorruptData),
        (version, Some(proposal_id)) => {
            let proposal = applied_by_id
                .get(proposal_id)
                .ok_or(TripRepoError::CorruptData)?;
            let encoded_change_set =
                serde_json::to_string(&proposal.change_set).map_err(corrupt)?;
            let base_hash = current_hash.as_deref().ok_or(TripRepoError::CorruptData)?;
            let base_plan = &current.as_ref().ok_or(TripRepoError::CorruptData)?.plan;
            let applied_at = plan_timestamp(&plan.created_at)?;
            if applied_at < plan_timestamp(&proposal.created_at)?
                || applied_at < plan_timestamp(&base_plan.created_at)?
            {
                return Err(TripRepoError::CorruptData);
            }
            if proposal.change_set.base_plan_version.checked_add(1) != Some(version)
                || stored_plan.applied_change_set_json.as_deref()
                    != Some(encoded_change_set.as_str())
                || stored_plan.base_structure_hash.as_deref() != Some(base_hash)
                || !linked_proposals.insert(proposal_id.to_string())
            {
                return Err(TripRepoError::CorruptData);
            }
            let application = replay_stored_change_set(
                current.as_ref().ok_or(TripRepoError::CorruptData)?,
                known_place_ids,
                plan,
                proposal_id,
                &proposal.change_set,
                &stored_plan.application_entity_ids,
            )
            .map_err(corrupt)?;
            let replayed_hash =
                plan_structure_hash(&application.plan, &application.days, &application.stops)?;
            if replayed_hash != actual_hash
                || stored_plan.structure_hash.as_deref() != Some(replayed_hash.as_str())
            {
                return Err(TripRepoError::CorruptData);
            }
            *current_hash = Some(replayed_hash);
            *current = Some(PlanDetail {
                plan: application.plan,
                days: application.days,
                stops: application.stops,
                legs: Vec::new(),
                day_feasibility: Vec::new(),
                places: Vec::new(),
            });
        }
    }
    Ok(())
}

fn plan_timestamp(value: &str) -> Result<DateTime<chrono::FixedOffset>, TripRepoError> {
    if value.len() > 64 || !value.ends_with('Z') {
        return Err(TripRepoError::CorruptData);
    }
    let timestamp = DateTime::parse_from_rfc3339(value).map_err(corrupt)?;
    if timestamp.offset().local_minus_utc() == 0 {
        Ok(timestamp)
    } else {
        Err(TripRepoError::CorruptData)
    }
}

pub(in crate::sqlite) async fn load_plan_structural_audits(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<(Plan, StructuralAuditBinding)>, TripRepoError> {
    let stored = load_stored_plan_versions(transaction, trip_id).await?;
    let plans = stored
        .iter()
        .map(|plan| plan.value.clone())
        .collect::<Vec<_>>();
    validate_plan_lineage_poll_provenance(transaction, trip_id, &plans).await?;
    Ok(stored
        .into_iter()
        .flat_map(|plan| {
            let value = plan.value;
            plan.structural_audits
                .into_iter()
                .map(move |binding| (value.clone(), binding))
        })
        .collect())
}

async fn validate_plan_provenance(
    transaction: &mut Transaction<'static, Sqlite>,
    plan: &StoredPlan,
) -> Result<(), TripRepoError> {
    let value = &plan.value;
    match (value.version, value.created_from_proposal_id.as_deref()) {
        (1, None) => Ok(()),
        (1, Some(_)) | (_, None) => Err(TripRepoError::CorruptData),
        (version, Some(proposal_id)) => {
            let proposal = crate::sqlite::proposal_repo::operations::load_proposal(
                transaction,
                &value.trip_id,
                proposal_id,
            )
            .await
            .map_err(map_proposal_error)?
            .ok_or(TripRepoError::CorruptData)?;
            let encoded_change_set =
                serde_json::to_string(&proposal.value.change_set).map_err(corrupt)?;
            if proposal.value.status == ProposalStatus::Applied
                && proposal.value.change_set.base_plan_version.checked_add(1) == Some(version)
                && plan.applied_change_set_json.as_deref() == Some(encoded_change_set.as_str())
            {
                Ok(())
            } else {
                Err(TripRepoError::CorruptData)
            }
        }
    }
}

pub(in crate::sqlite) fn plan_structure_hash(
    plan: &Plan,
    days: &[Day],
    stops: &[Stop],
) -> Result<String, TripRepoError> {
    let mut ordered_days = days.iter().collect::<Vec<_>>();
    ordered_days.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then_with(|| left.id.cmp(&right.id))
    });
    let day_structure = ordered_days
        .iter()
        .map(|day| (day.id.as_str(), day.plan_id.as_str(), day.date.as_str()))
        .collect::<Vec<_>>();
    let mut ordered_stops = stops.iter().collect::<Vec<_>>();
    ordered_stops.sort_by(|left, right| {
        left.day_id
            .cmp(&right.day_id)
            .then_with(|| left.seq.total_cmp(&right.seq))
            .then_with(|| left.id.cmp(&right.id))
    });
    let stop_structure = ordered_stops
        .iter()
        .map(|stop| {
            (
                stop.id.as_str(),
                stop.day_id.as_str(),
                stop.seq.to_bits(),
                stop.place_id.as_str(),
                stop.stop_kind.as_ref(),
            )
        })
        .collect::<Vec<_>>();
    let encoded = serde_json::to_vec(&(
        plan.id.as_str(),
        plan.trip_id.as_str(),
        plan.version,
        plan.created_from_proposal_id.as_deref(),
        plan.created_at.as_str(),
        day_structure,
        stop_structure,
    ))
    .map_err(corrupt)?;
    Ok(sha256_hex(&encoded))
}

pub(in crate::sqlite) async fn validate_plan_poll_provenance(
    transaction: &mut Transaction<'static, Sqlite>,
    plan: &Plan,
) -> Result<(), TripRepoError> {
    let Some(proposal_id) = plan.created_from_proposal_id.as_deref() else {
        return Ok(());
    };
    let proposal = crate::sqlite::proposal_repo::operations::load_proposal(
        transaction,
        &plan.trip_id,
        proposal_id,
    )
    .await
    .map_err(map_proposal_error)?
    .ok_or(TripRepoError::CorruptData)?;
    crate::sqlite::poll_repo::operations::validate_proposal_link(
        transaction,
        &plan.trip_id,
        &proposal.value,
    )
    .await
    .map_err(map_poll_error)
}

pub(in crate::sqlite) async fn validate_plan_lineage_poll_provenance(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    plans: &[Plan],
) -> Result<(), TripRepoError> {
    let proposals =
        crate::sqlite::proposal_repo::operations::load_applied_proposals(transaction, trip_id)
            .await
            .map_err(map_proposal_error)?;
    if plans.len().saturating_sub(1) != proposals.len()
        || plans
            .first()
            .is_some_and(|plan| plan.version != 1 || plan.created_from_proposal_id.is_some())
    {
        return Err(TripRepoError::CorruptData);
    }
    let proposals_by_id = proposals
        .iter()
        .map(|proposal| (proposal.value.id.as_str(), proposal))
        .collect::<HashMap<_, _>>();
    if proposals_by_id.len() != proposals.len() {
        return Err(TripRepoError::CorruptData);
    }
    for pair in plans.windows(2) {
        let base = &pair[0];
        let plan = &pair[1];
        let proposal_id = plan
            .created_from_proposal_id
            .as_deref()
            .ok_or(TripRepoError::CorruptData)?;
        let proposal = proposals_by_id
            .get(proposal_id)
            .ok_or(TripRepoError::CorruptData)?;
        if proposal.value.status != ProposalStatus::Applied
            || proposal.value.change_set.base_plan_version != base.version
            || plan.version
                != base
                    .version
                    .checked_add(1)
                    .ok_or(TripRepoError::CorruptData)?
            || plan_timestamp(&plan.created_at)? < plan_timestamp(&proposal.value.created_at)?
            || plan_timestamp(&plan.created_at)? < plan_timestamp(&base.created_at)?
        {
            return Err(TripRepoError::CorruptData);
        }
    }
    let polls = crate::sqlite::poll_repo::operations::load_applied_plan_polls(transaction, trip_id)
        .await
        .map_err(map_poll_error)?;
    crate::sqlite::poll_repo::operations::validate_governance_links(&polls, &proposals, plans)
        .map_err(map_poll_error)
}

fn map_poll_error(error: itinera_core::ports::poll::PollRepoError) -> TripRepoError {
    match error {
        itinera_core::ports::poll::PollRepoError::Unavailable => TripRepoError::Unavailable,
        itinera_core::ports::poll::PollRepoError::SafetyLimitExceeded
        | itinera_core::ports::poll::PollRepoError::CorruptData
        | itinera_core::ports::poll::PollRepoError::NotFound
        | itinera_core::ports::poll::PollRepoError::Forbidden
        | itinera_core::ports::poll::PollRepoError::Conflict
        | itinera_core::ports::poll::PollRepoError::InvalidVote => TripRepoError::CorruptData,
    }
}

fn map_proposal_error(error: itinera_core::ports::proposal::ProposalRepoError) -> TripRepoError {
    match error {
        itinera_core::ports::proposal::ProposalRepoError::Unavailable => TripRepoError::Unavailable,
        itinera_core::ports::proposal::ProposalRepoError::SafetyLimitExceeded => {
            TripRepoError::CorruptData
        }
        itinera_core::ports::proposal::ProposalRepoError::CorruptData
        | itinera_core::ports::proposal::ProposalRepoError::NotFound
        | itinera_core::ports::proposal::ProposalRepoError::Forbidden
        | itinera_core::ports::proposal::ProposalRepoError::Conflict
        | itinera_core::ports::proposal::ProposalRepoError::InvalidChange => {
            TripRepoError::CorruptData
        }
    }
}

async fn load_validated_trip(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<(TripRow, Trip), TripRepoError> {
    let row = load_trip(transaction, trip_id).await?;
    let profiles = load_members_and_validate_capacity(transaction, trip_id).await?;
    let trip = row.clone().into_trip(member_values(&profiles))?.value;
    Ok((row, trip))
}

fn validate_plan_v1_input(trip_id: &str, plan: &Plan, days: &[Day]) -> Result<(), TripRepoError> {
    if plan.trip_id != trip_id || plan.version != 1 || plan.created_from_proposal_id.is_some() {
        return Err(TripRepoError::CorruptData);
    }
    validate_stored_plan_graph(plan, days, &[], trip_id, 1).map_err(corrupt)
}

fn validate_bootstrap_days(
    trip: &Trip,
    plan: &Plan,
    days: &[Day],
    city: &str,
    timezone: &str,
) -> Result<(), TripRepoError> {
    let expected_dates = trip
        .dates()
        .into_iter()
        .map(|date| date.format("%Y-%m-%d").to_string())
        .collect::<Vec<_>>();
    if days.len() != expected_dates.len()
        || days.iter().zip(expected_dates).any(|(day, date)| {
            day.plan_id != plan.id
                || day.date != date
                || day.city_hint != city
                || day.tz != timezone
                || day.window_start != "09:00"
                || day.window_end != "21:00"
        })
    {
        return Err(TripRepoError::CorruptData);
    }
    Ok(())
}

fn owned_pointer(row: &TripRow) -> Result<Option<(String, u32)>, TripRepoError> {
    row.current_plan_pointer()
        .map(|pointer| pointer.map(|(id, version)| (id.to_string(), version)))
}

fn derive_day_feasibility(
    days: &[Day],
    stops: &[Stop],
) -> Result<Vec<DayFeasibility>, TripRepoError> {
    days.iter()
        .map(|day| {
            let used_min = stops
                .iter()
                .filter(|stop| stop.day_id == day.id)
                .try_fold(0_u32, |total, stop| total.checked_add(stop.duration_min))
                .ok_or(TripRepoError::CorruptData)?;
            let window_min = window_minutes(&day.window_start, &day.window_end)
                .ok_or(TripRepoError::CorruptData)?;
            let ratio = f64::from(used_min) / f64::from(window_min.max(1));
            let feasibility = if ratio > 1.0 {
                Feasibility::Unreasonable
            } else if ratio >= 0.85 {
                Feasibility::Tight
            } else {
                Feasibility::Ok
            };
            let notes = if feasibility == Feasibility::Ok {
                Vec::new()
            } else {
                vec![format!(
                    "{}% of the day window is used.",
                    (ratio * 100.0).round()
                )]
            };
            Ok(DayFeasibility {
                day_id: day.id.clone(),
                feasibility,
                used_min,
                window_min,
                notes,
            })
        })
        .collect()
}

fn window_minutes(start: &str, end: &str) -> Option<u32> {
    fn minutes(value: &str) -> Option<u32> {
        let (hours, minutes) = value.split_once(':')?;
        Some(hours.parse::<u32>().ok()? * 60 + minutes.parse::<u32>().ok()?)
    }
    minutes(end)?.checked_sub(minutes(start)?)
}

fn unavailable(_error: sqlx::Error) -> TripRepoError {
    TripRepoError::Unavailable
}

fn corrupt<T>(_error: T) -> TripRepoError {
    TripRepoError::CorruptData
}
