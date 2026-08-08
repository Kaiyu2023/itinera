//! Plan v1 initialization and bounded version/current-plan reads.

use std::collections::HashSet;

use itinera_core::{
    domain::trip::{
        CandidateStatus, Day, DayFeasibility, Feasibility, Plan, PlanDetail, Stop, Trip, TripStatus,
    },
    ports::{authorization::TripAuthorizationContext, trip::TripRepoError},
    services::plans::validate_stored_plan_graph,
};
use sqlx::{Sqlite, Transaction};

use crate::sqlite::{
    SqliteDb,
    codec::{next_revision, validate_id},
};

use super::{
    access::{
        RequiredRole, authorize, load_members_and_validate_capacity, load_trip, member_values,
    },
    candidate_records::encoded_size,
    candidates::{load_candidates, load_place},
    plan_records::{
        DayRow, MAX_PLAN_RESPONSE_BYTES, MAX_PLAN_VERSIONS, PLAN_VERSION_QUERY_LIMIT, PlanRow,
        StopRow,
    },
    records::TripRow,
};

const MAX_PLAN_GRAPH_ITEMS: usize = 100;
const PLAN_GRAPH_QUERY_LIMIT: i64 = 101;

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

    sqlx::query(
        "INSERT INTO plans ( \
             trip_id, version, id, created_from_proposal_id, created_at, revision \
         ) VALUES (?, 1, ?, NULL, ?, 1)",
    )
    .bind(trip_id)
    .bind(&plan.id)
    .bind(&plan.created_at)
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
    match (plans.last(), owned_pointer(&trip_row)?) {
        (None, None) => {}
        (Some(latest), Some((id, version))) if latest.id == id && latest.version == version => {}
        _ => return Err(TripRepoError::CorruptData),
    }
    for plan in &plans {
        // Until proposals are authoritative, only the provenance-free v1 row
        // can be trusted. The proposal slice replaces this with reciprocal
        // applied-proposal validation for every later version.
        if plan.version != 1 || plan.created_from_proposal_id.is_some() {
            return Err(TripRepoError::CorruptData);
        }
        load_plan_detail(&mut transaction, trip_id, &plan.id, plan.version).await?;
    }
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(plans)
}

pub(super) async fn load_plan_detail(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    plan_id: &str,
    version: u32,
) -> Result<PlanDetail, TripRepoError> {
    let row = sqlx::query_as::<_, PlanRow>(
        "SELECT trip_id AS plan_trip_id, version AS plan_version, id AS plan_id, \
                created_from_proposal_id, created_at AS plan_created_at, \
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
    let plan = row.into_plan(trip_id)?;
    // Until proposal persistence is authoritative, a SQLite plan can only be
    // the provenance-free bootstrap version. Keep this check in the shared
    // graph loader so current-plan reads, trip summaries, and initialization's
    // idempotent path cannot trust a manually inserted later version.
    if plan.version != 1 || plan.created_from_proposal_id.is_some() {
        return Err(TripRepoError::CorruptData);
    }

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

async fn load_plan_versions(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<Plan>, TripRepoError> {
    let rows = sqlx::query_as::<_, PlanRow>(
        "SELECT trip_id AS plan_trip_id, version AS plan_version, id AS plan_id, \
                created_from_proposal_id, created_at AS plan_created_at, \
                revision AS plan_revision \
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
        .map(|row| row.into_plan(trip_id))
        .collect::<Result<Vec<_>, _>>()?;
    if plans
        .iter()
        .enumerate()
        .any(|(index, plan)| usize::try_from(plan.version).ok() != Some(index + 1))
        || encoded_size(&plans)? > MAX_PLAN_RESPONSE_BYTES
    {
        return Err(TripRepoError::CorruptData);
    }
    Ok(plans)
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
