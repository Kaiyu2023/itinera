//! Explicitly allowlisted, atomic, stale-safe content reverts.

use chrono::DateTime;
use itinera_core::{
    domain::{
        content_history::{ChangeSource, Edit, EditEntity, EditStatus},
        trip::{Booking, CandidateStatus, Place, Stop, TripStatus},
        user::UserId,
    },
    ports::{authorization::TripAuthorizationContext, content_history::ContentHistoryRepoError},
    services::{
        candidates::{validate_candidate_place_provenance, validate_candidate_place_revision},
        content_history::validate_stored_edit,
        plans::validate_stored_plan_graph,
        validation::{
            duration_min, exact_bounded_strings, exact_required_text, local_time, text_len,
            time_window, validate_booking,
        },
    },
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use sqlx::{Sqlite, Transaction};

use crate::sqlite::{
    SqliteDb,
    codec::{checked_revision, next_revision, validate_id},
    trip_repo::{
        access::{RequiredRole, authorize, validate_trip_aggregate},
        candidates::{load_candidates, load_place},
        plan_records::encode_booking_columns,
        plans::load_plan_detail,
    },
};

use super::{
    audit::{history_values, insert_edit, load_history, validate_projected},
    map_trip_error,
    records::validate_actor_id,
};

const MAX_CANDIDATE_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

pub(super) async fn revert_edit(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    edit_id: &str,
    reverted_at: &str,
    compensating_edit_id: &str,
) -> Result<(), ContentHistoryRepoError> {
    validate_id(compensating_edit_id).map_err(corrupt)?;
    if edit_id == compensating_edit_id || !is_canonical_utc(reverted_at) {
        return Err(ContentHistoryRepoError::CorruptData);
    }

    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Editor,
    )
    .await
    .map_err(map_trip_error)?;
    let actor = authorization
        .human_user_id()
        .ok_or(ContentHistoryRepoError::Forbidden)?;
    validate_actor_id(&actor.0)?;
    validate_trip_aggregate(&mut transaction, trip_id)
        .await
        .map_err(map_trip_error)?;

    let records = load_history(&mut transaction, trip_id).await?;
    let original_index = records
        .iter()
        .position(|record| record.value.id == edit_id)
        .ok_or(ContentHistoryRepoError::NotFound)?;
    let original = &records[original_index];
    if original.value.entity == EditEntity::Notice {
        // Notice authorship/leader rules and the live notice aggregate arrive
        // with the notices capability. Do not guess at either in this slice.
        return Err(ContentHistoryRepoError::Unsupported);
    }
    if original.value.status == EditStatus::Reverted {
        db.commit(transaction).await.map_err(unavailable)?;
        return Ok(());
    }
    if original.value.status != EditStatus::Applied {
        return Err(ContentHistoryRepoError::Conflict);
    }
    let original_time =
        DateTime::parse_from_rfc3339(&original.value.created_at).map_err(corrupt)?;
    let revert_time = DateTime::parse_from_rfc3339(reverted_at).map_err(corrupt)?;
    if revert_time < original_time
        || records
            .iter()
            .any(|record| record.value.id == compensating_edit_id)
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }

    let compensation_values =
        revert_live_target(&mut transaction, trip_id, &original.value).await?;
    let reverted = reverted_original(&original.value, actor, reverted_at, compensating_edit_id);
    let mut compensation =
        compensating_edit(&original.value, actor, reverted_at, compensating_edit_id);
    if let Some((before, after)) = compensation_values {
        compensation.old_value = before;
        compensation.new_value = after;
    }
    validate_stored_edit(trip_id, &reverted).map_err(corrupt)?;
    validate_stored_edit(trip_id, &compensation).map_err(corrupt)?;

    let mut projected = history_values(&records)?;
    let projected_original = projected
        .iter_mut()
        .find(|edit| edit.id == original.value.id)
        .ok_or(ContentHistoryRepoError::CorruptData)?;
    *projected_original = reverted.clone();
    projected.push(compensation.clone());
    validate_projected(&projected)?;

    let revision = next_revision(original.revision).map_err(corrupt)?;
    let updated = sqlx::query(
        "UPDATE content_edits \
         SET status = 'reverted', reverted_by = ?, reverted_at = ?, \
             revert_edit_id = ?, revision = ? \
         WHERE trip_id = ? AND id = ? AND revision = ? AND status = 'applied'",
    )
    .bind(&actor.0)
    .bind(reverted_at)
    .bind(compensating_edit_id)
    .bind(revision)
    .bind(trip_id)
    .bind(edit_id)
    .bind(original.revision)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    if updated.rows_affected() != 1 {
        return Err(ContentHistoryRepoError::Conflict);
    }
    insert_edit(&mut transaction, &compensation, 1).await?;
    db.commit(transaction).await.map_err(unavailable)
}

async fn revert_live_target(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    edit: &Edit,
) -> Result<Option<(Value, Value)>, ContentHistoryRepoError> {
    match edit.entity {
        EditEntity::Trip if edit.field == "status" => {
            revert_trip_status(transaction, trip_id, edit).await?;
            Ok(None)
        }
        EditEntity::Candidate => {
            revert_candidate(transaction, trip_id, edit).await?;
            Ok(None)
        }
        EditEntity::Day => {
            revert_day(transaction, trip_id, edit).await?;
            Ok(None)
        }
        EditEntity::Stop => revert_stop(transaction, trip_id, edit).await,
        EditEntity::Notice => Err(ContentHistoryRepoError::Unsupported),
        EditEntity::Trip => Err(ContentHistoryRepoError::Unsupported),
    }
}

async fn revert_trip_status(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    edit: &Edit,
) -> Result<(), ContentHistoryRepoError> {
    if edit.entity_id != trip_id {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let old = parse_exact::<TripStatus>(&edit.old_value)?;
    let new = parse_exact::<TripStatus>(&edit.new_value)?;
    let (stored_status, revision): (String, i64) =
        sqlx::query_as("SELECT status, revision FROM trips WHERE id = ?")
            .bind(trip_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(unavailable)?;
    checked_revision(revision).map_err(corrupt)?;
    let current = stored_status.parse::<TripStatus>().map_err(corrupt)?;
    if current != new {
        return Err(ContentHistoryRepoError::Conflict);
    }
    let updated =
        sqlx::query("UPDATE trips SET status = ?, revision = ? WHERE id = ? AND revision = ?")
            .bind(old.as_ref())
            .bind(next_revision(revision).map_err(corrupt)?)
            .bind(trip_id)
            .bind(revision)
            .execute(&mut **transaction)
            .await
            .map_err(unavailable)?;
    ensure_one(updated.rows_affected())
}

async fn revert_candidate(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    edit: &Edit,
) -> Result<(), ContentHistoryRepoError> {
    let mut candidates = load_candidates(transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    let index = candidates
        .iter()
        .position(|candidate| candidate.candidate.id == edit.entity_id)
        .ok_or(ContentHistoryRepoError::Conflict)?;
    if candidates[index].candidate.status == CandidateStatus::InPlan {
        return Err(ContentHistoryRepoError::Conflict);
    }
    let revision: i64 =
        sqlx::query_scalar("SELECT revision FROM candidates WHERE trip_id = ? AND id = ?")
            .bind(trip_id)
            .bind(&edit.entity_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(unavailable)?;
    checked_revision(revision).map_err(corrupt)?;

    match edit.field.as_str() {
        "status" => {
            let old = parse_exact::<CandidateStatus>(&edit.old_value)?;
            let new = parse_exact::<CandidateStatus>(&edit.new_value)?;
            if old == CandidateStatus::InPlan || new == CandidateStatus::InPlan {
                return Err(ContentHistoryRepoError::Unsupported);
            }
            if candidates[index].candidate.status != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            candidates[index].candidate.status = old;
            update_candidate_scalar(
                transaction,
                trip_id,
                &edit.entity_id,
                "status",
                encode_candidate_status(old),
                revision,
            )
            .await?;
        }
        "pitch" => {
            let old = parse_exact::<String>(&edit.old_value)?;
            let new = parse_exact::<String>(&edit.new_value)?;
            validate_required(&old, 2_000)?;
            validate_required(&new, 2_000)?;
            if candidates[index].candidate.pitch != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            candidates[index].candidate.pitch = old.clone();
            update_candidate_scalar(
                transaction,
                trip_id,
                &edit.entity_id,
                "pitch",
                &old,
                revision,
            )
            .await?;
        }
        "tags" => {
            let old = parse_exact::<Vec<String>>(&edit.old_value)?;
            let new = parse_exact::<Vec<String>>(&edit.new_value)?;
            validate_tags(&old)?;
            validate_tags(&new)?;
            if candidates[index].candidate.tags != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            candidates[index].candidate.tags = old.clone();
            let encoded = serde_json::to_string(&old).map_err(corrupt)?;
            update_candidate_scalar(
                transaction,
                trip_id,
                &edit.entity_id,
                "tags_json",
                &encoded,
                revision,
            )
            .await?;
        }
        "place" => {
            let old = parse_exact::<Place>(&edit.old_value)?;
            let new = parse_exact::<Place>(&edit.new_value)?;
            validate_candidate_place_revision(&old, &new).map_err(corrupt)?;
            if candidates[index].candidate.place_id != new.id || candidates[index].place != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            let stored_old = load_place(transaction, trip_id, &old.id)
                .await
                .map_err(map_trip_error)?
                .ok_or(ContentHistoryRepoError::CorruptData)?;
            let stored_new = load_place(transaction, trip_id, &new.id)
                .await
                .map_err(map_trip_error)?
                .ok_or(ContentHistoryRepoError::CorruptData)?;
            if stored_old != old || stored_new != new {
                return Err(ContentHistoryRepoError::CorruptData);
            }
            let source = match candidates[index].candidate.source_place_id.as_deref() {
                Some(source_id) => load_place(transaction, trip_id, source_id)
                    .await
                    .map_err(map_trip_error)?,
                None => None,
            };
            let mut reverted_candidate = candidates[index].candidate.clone();
            reverted_candidate.place_id = old.id.clone();
            validate_candidate_place_provenance(&reverted_candidate, &old, source.as_ref())
                .map_err(corrupt)?;
            candidates[index].candidate = reverted_candidate;
            candidates[index].place = old.clone();
            update_candidate_scalar(
                transaction,
                trip_id,
                &edit.entity_id,
                "place_id",
                &old.id,
                revision,
            )
            .await?;
        }
        _ => return Err(ContentHistoryRepoError::Unsupported),
    }
    ensure_candidate_budget(&candidates)
}

async fn update_candidate_scalar(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    candidate_id: &str,
    column: &str,
    value: &str,
    revision: i64,
) -> Result<(), ContentHistoryRepoError> {
    let statement = match column {
        "status" => {
            "UPDATE candidates SET status = ?, revision = ? WHERE trip_id = ? AND id = ? AND revision = ?"
        }
        "pitch" => {
            "UPDATE candidates SET pitch = ?, revision = ? WHERE trip_id = ? AND id = ? AND revision = ?"
        }
        "tags_json" => {
            "UPDATE candidates SET tags_json = ?, revision = ? WHERE trip_id = ? AND id = ? AND revision = ?"
        }
        "place_id" => {
            "UPDATE candidates SET place_id = ?, revision = ? WHERE trip_id = ? AND id = ? AND revision = ?"
        }
        _ => return Err(ContentHistoryRepoError::CorruptData),
    };
    let updated = sqlx::query(statement)
        .bind(value)
        .bind(next_revision(revision).map_err(corrupt)?)
        .bind(trip_id)
        .bind(candidate_id)
        .bind(revision)
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?;
    ensure_one(updated.rows_affected())
}

async fn revert_day(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    edit: &Edit,
) -> Result<(), ContentHistoryRepoError> {
    let (mut detail, revision) =
        load_current_day_target(transaction, trip_id, &edit.entity_id).await?;
    let day = detail
        .days
        .iter_mut()
        .find(|day| day.id == edit.entity_id)
        .ok_or(ContentHistoryRepoError::Conflict)?;
    let (column, restored) = match edit.field.as_str() {
        "windowStart" => {
            let old = parse_exact::<String>(&edit.old_value)?;
            let new = parse_exact::<String>(&edit.new_value)?;
            validate_local_time(&old)?;
            validate_local_time(&new)?;
            if day.window_start != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            day.window_start = old.clone();
            ("window_start", old)
        }
        "windowEnd" => {
            let old = parse_exact::<String>(&edit.old_value)?;
            let new = parse_exact::<String>(&edit.new_value)?;
            validate_local_time(&old)?;
            validate_local_time(&new)?;
            if day.window_end != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            day.window_end = old.clone();
            ("window_end", old)
        }
        "cityHint" => {
            let old = parse_exact::<String>(&edit.old_value)?;
            let new = parse_exact::<String>(&edit.new_value)?;
            validate_required(&old, 120)?;
            validate_required(&new, 120)?;
            if day.city_hint != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            day.city_hint = old.clone();
            ("city_hint", old)
        }
        _ => return Err(ContentHistoryRepoError::Unsupported),
    };
    time_window(&day.window_start, &day.window_end)
        .map_err(|_| ContentHistoryRepoError::Conflict)?;
    validate_stored_plan_graph(
        &detail.plan,
        &detail.days,
        &detail.stops,
        trip_id,
        detail.plan.version,
    )
    .map_err(corrupt)?;
    update_day_scalar(
        transaction,
        trip_id,
        detail.plan.version,
        &edit.entity_id,
        column,
        &restored,
        revision,
    )
    .await
}

async fn load_current_day_target(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    day_id: &str,
) -> Result<(itinera_core::domain::trip::PlanDetail, i64), ContentHistoryRepoError> {
    let detail = load_current_plan(transaction, trip_id).await?;
    if !detail.days.iter().any(|day| day.id == day_id) {
        return Err(ContentHistoryRepoError::Conflict);
    }
    let revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM plan_days WHERE trip_id = ? AND plan_version = ? AND id = ?",
    )
    .bind(trip_id)
    .bind(i64::from(detail.plan.version))
    .bind(day_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    checked_revision(revision).map_err(corrupt)?;
    Ok((detail, revision))
}

async fn update_day_scalar(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    version: u32,
    day_id: &str,
    column: &str,
    value: &str,
    revision: i64,
) -> Result<(), ContentHistoryRepoError> {
    let statement = match column {
        "window_start" => {
            "UPDATE plan_days SET window_start = ?, revision = ? WHERE trip_id = ? AND plan_version = ? AND id = ? AND revision = ?"
        }
        "window_end" => {
            "UPDATE plan_days SET window_end = ?, revision = ? WHERE trip_id = ? AND plan_version = ? AND id = ? AND revision = ?"
        }
        "city_hint" => {
            "UPDATE plan_days SET city_hint = ?, revision = ? WHERE trip_id = ? AND plan_version = ? AND id = ? AND revision = ?"
        }
        _ => return Err(ContentHistoryRepoError::CorruptData),
    };
    let updated = sqlx::query(statement)
        .bind(value)
        .bind(next_revision(revision).map_err(corrupt)?)
        .bind(trip_id)
        .bind(i64::from(version))
        .bind(day_id)
        .bind(revision)
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?;
    ensure_one(updated.rows_affected())
}

async fn revert_stop(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    edit: &Edit,
) -> Result<Option<(Value, Value)>, ContentHistoryRepoError> {
    let mut detail = load_current_plan(transaction, trip_id).await?;
    let revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM plan_stops WHERE trip_id = ? AND plan_version = ? AND id = ?",
    )
    .bind(trip_id)
    .bind(i64::from(detail.plan.version))
    .bind(&edit.entity_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?
    .ok_or(ContentHistoryRepoError::Conflict)?;
    checked_revision(revision).map_err(corrupt)?;
    let stop = detail
        .stops
        .iter_mut()
        .find(|stop| stop.id == edit.entity_id)
        .ok_or(ContentHistoryRepoError::Conflict)?;
    let mut compensation_values = None;
    match edit.field.as_str() {
        "plannedArrival" => {
            let old = parse_exact::<String>(&edit.old_value)?;
            let new = parse_exact::<String>(&edit.new_value)?;
            validate_local_time(&old)?;
            validate_local_time(&new)?;
            if stop.planned_arrival != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            stop.planned_arrival = old;
        }
        "durationMin" => {
            let old = parse_exact::<u32>(&edit.old_value)?;
            let new = parse_exact::<u32>(&edit.new_value)?;
            validate_duration(old)?;
            validate_duration(new)?;
            if stop.duration_min != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            stop.duration_min = old;
        }
        "notes" => {
            let old = parse_exact::<String>(&edit.old_value)?;
            let new = parse_exact::<String>(&edit.new_value)?;
            validate_text(&old, 10_000)?;
            validate_text(&new, 10_000)?;
            if stop.notes != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            stop.notes = old;
        }
        "booking" => {
            let (booking, values) =
                reverted_booking(&stop.booking, &edit.old_value, &edit.new_value)?;
            stop.booking = booking;
            compensation_values = Some(values);
        }
        _ => return Err(ContentHistoryRepoError::Unsupported),
    }
    validate_stored_plan_graph(
        &detail.plan,
        &detail.days,
        &detail.stops,
        trip_id,
        detail.plan.version,
    )
    .map_err(corrupt)?;
    let target = detail
        .stops
        .iter()
        .find(|stop| stop.id == edit.entity_id)
        .ok_or(ContentHistoryRepoError::CorruptData)?;
    update_stop_row(transaction, trip_id, detail.plan.version, target, revision).await?;
    Ok(compensation_values)
}

async fn load_current_plan(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<itinera_core::domain::trip::PlanDetail, ContentHistoryRepoError> {
    let pointer: (Option<String>, Option<i64>) =
        sqlx::query_as("SELECT current_plan_id, current_plan_version FROM trips WHERE id = ?")
            .bind(trip_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(unavailable)?;
    let (plan_id, version) = match pointer {
        (Some(plan_id), Some(version)) => (plan_id, version),
        (None, None) => return Err(ContentHistoryRepoError::Conflict),
        _ => return Err(ContentHistoryRepoError::CorruptData),
    };
    let version = u32::try_from(version)
        .ok()
        .filter(|version| *version > 0)
        .ok_or(ContentHistoryRepoError::CorruptData)?;
    load_plan_detail(transaction, trip_id, &plan_id, version)
        .await
        .map_err(map_trip_error)
}

async fn update_stop_row(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    version: u32,
    stop: &Stop,
    revision: i64,
) -> Result<(), ContentHistoryRepoError> {
    let booking = encode_booking_columns(stop.booking.as_ref()).map_err(map_trip_error)?;
    let updated = sqlx::query(
        "UPDATE plan_stops \
         SET planned_arrival = ?, duration_min = ?, notes = ?, booking_ref = ?, \
             booking_url = ?, booking_cost_amount = ?, booking_cost_currency = ?, revision = ? \
         WHERE trip_id = ? AND plan_version = ? AND id = ? AND revision = ?",
    )
    .bind(&stop.planned_arrival)
    .bind(i64::from(stop.duration_min))
    .bind(&stop.notes)
    .bind(booking.reference)
    .bind(booking.url)
    .bind(booking.amount)
    .bind(booking.currency)
    .bind(next_revision(revision).map_err(corrupt)?)
    .bind(trip_id)
    .bind(i64::from(version))
    .bind(&stop.id)
    .bind(revision)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    ensure_one(updated.rows_affected())
}

fn reverted_booking(
    current: &Option<Booking>,
    old_value: &Value,
    new_value: &Value,
) -> Result<(Option<Booking>, (Value, Value)), ContentHistoryRepoError> {
    let actual_before = serde_json::to_value(current).map_err(corrupt)?;
    let old = parse_exact::<Option<Booking>>(old_value)?;
    let new = parse_exact::<Option<Booking>>(new_value)?;
    validate_booking(old.as_ref()).map_err(corrupt)?;
    validate_booking(new.as_ref()).map_err(corrupt)?;
    if booking_link(old.as_ref()).is_some() || booking_link(new.as_ref()).is_some() {
        // SQLite plan rows do not gain their derived ledger link until the
        // ledger capability can verify the reciprocal expense claim.
        return Err(ContentHistoryRepoError::CorruptData);
    }
    if current != &new {
        return Err(ContentHistoryRepoError::Conflict);
    }
    let actual_after = serde_json::to_value(&old).map_err(corrupt)?;
    Ok((old, (actual_before, actual_after)))
}

fn booking_link(booking: Option<&Booking>) -> Option<&str> {
    booking.and_then(|booking| booking.ledger_entry_id.as_deref())
}

fn reverted_original(
    original: &Edit,
    actor: &UserId,
    reverted_at: &str,
    compensating_edit_id: &str,
) -> Edit {
    let mut reverted = original.clone();
    reverted.status = EditStatus::Reverted;
    reverted.reverted_by = Some(actor.0.clone());
    reverted.reverted_at = Some(reverted_at.to_string());
    reverted.revert_edit_id = Some(compensating_edit_id.to_string());
    reverted
}

fn compensating_edit(
    original: &Edit,
    actor: &UserId,
    reverted_at: &str,
    compensating_edit_id: &str,
) -> Edit {
    Edit {
        id: compensating_edit_id.to_string(),
        trip_id: original.trip_id.clone(),
        entity: original.entity,
        entity_id: original.entity_id.clone(),
        field: original.field.clone(),
        old_value: original.new_value.clone(),
        new_value: original.old_value.clone(),
        author: actor.0.clone(),
        source: ChangeSource::Web {},
        status: EditStatus::Applied,
        created_at: reverted_at.to_string(),
        reverted_by: None,
        reverted_at: None,
        revert_edit_id: None,
        reverts_edit_id: Some(original.id.clone()),
    }
}

fn parse_exact<T>(value: &Value) -> Result<T, ContentHistoryRepoError>
where
    T: DeserializeOwned + Serialize,
{
    let parsed = serde_json::from_value::<T>(value.clone()).map_err(corrupt)?;
    if serde_json::to_value(&parsed).map_err(corrupt)? == *value {
        Ok(parsed)
    } else {
        Err(ContentHistoryRepoError::CorruptData)
    }
}

fn validate_required(value: &str, maximum: usize) -> Result<(), ContentHistoryRepoError> {
    exact_required_text(value, "stored text is invalid", maximum).map_err(corrupt)
}

fn validate_tags(tags: &[String]) -> Result<(), ContentHistoryRepoError> {
    exact_bounded_strings(tags, 20, 60).map_err(corrupt)
}

fn validate_local_time(value: &str) -> Result<(), ContentHistoryRepoError> {
    local_time(value).map_err(corrupt)
}

fn validate_duration(value: u32) -> Result<(), ContentHistoryRepoError> {
    duration_min(value).map_err(corrupt)
}

fn validate_text(value: &str, maximum: usize) -> Result<(), ContentHistoryRepoError> {
    text_len(value, maximum).map_err(corrupt)
}

fn ensure_candidate_budget<T: Serialize>(candidates: &T) -> Result<(), ContentHistoryRepoError> {
    let encoded = serde_json::to_vec(candidates).map_err(corrupt)?;
    if encoded.len() > MAX_CANDIDATE_RESPONSE_BYTES {
        Err(ContentHistoryRepoError::SafetyLimitExceeded)
    } else {
        Ok(())
    }
}

fn encode_candidate_status(status: CandidateStatus) -> &'static str {
    match status {
        CandidateStatus::Shortlisted => "shortlisted",
        CandidateStatus::InPlan => "in_plan",
        CandidateStatus::Rejected => "rejected",
    }
}

fn ensure_one(rows: u64) -> Result<(), ContentHistoryRepoError> {
    if rows == 1 {
        Ok(())
    } else {
        Err(ContentHistoryRepoError::Conflict)
    }
}

fn is_canonical_utc(value: &str) -> bool {
    value.len() <= 64
        && value.ends_with('Z')
        && DateTime::parse_from_rfc3339(value)
            .is_ok_and(|timestamp| timestamp.offset().local_minus_utc() == 0)
}

fn unavailable<T>(_error: T) -> ContentHistoryRepoError {
    ContentHistoryRepoError::Unavailable
}

fn corrupt<T>(_error: T) -> ContentHistoryRepoError {
    ContentHistoryRepoError::CorruptData
}
