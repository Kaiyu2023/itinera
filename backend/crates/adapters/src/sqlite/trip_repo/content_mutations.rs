//! Audited trip, candidate, day, and stop content mutations.

use itinera_core::{
    domain::{
        content_history::{Edit, EditEntity},
        trip::{
            Booking, CandidateDisposition, CandidateStatus, CandidateWithPlace, Day, DayPatch,
            PlanDetail, Stop, StopPatch, Trip, TripStatus,
        },
    },
    ports::{
        authorization::TripAuthorizationContext,
        content_history::ContentHistoryRepoError,
        trip::{CandidateUpdate, TripRepoError},
    },
    services::{
        candidates::{
            validate_candidate_place_provenance, validate_candidate_place_revision,
            validate_stored_candidate,
        },
        plans::validate_stored_plan_graph,
        validation::{
            duration_min, exact_bounded_strings, exact_required_text, local_time, text_len,
            time_window, validate_booking,
        },
    },
};
use serde_json::{Value, json};
use sqlx::{Sqlite, Transaction};

use crate::sqlite::{
    SqliteDb,
    codec::{checked_revision, next_revision},
    history_repo::audit::{AuditChange, append_edits, audit, suffixed_id},
};

use super::{
    access::{
        RequiredRole, authorize, load_members_and_validate_capacity, load_trip, member_values,
    },
    candidate_records::{
        MAX_RESPONSE_BYTES, encode_candidate_status, encode_place, encode_tags, encoded_size,
    },
    candidates::{insert_place, load_candidates, load_place},
    plan_records::encode_booking_columns,
    plans::load_plan_detail,
    records::TripRow,
};

pub(super) async fn set_trip_status(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    status: TripStatus,
    changed_at: &str,
    change_id: &str,
) -> Result<Trip, TripRepoError> {
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    let actor = authorize_editor(&mut transaction, trip_id, authorization).await?;
    let (row, trip) = load_validated_trip(&mut transaction, trip_id).await?;
    if trip.status() == status {
        db.commit(transaction).await.map_err(unavailable)?;
        return Ok(trip);
    }
    let edit = audit(
        trip_id,
        actor,
        changed_at,
        change_id,
        AuditChange {
            entity: EditEntity::Trip,
            entity_id: trip_id,
            field: "status",
            old_value: json!(trip.status()),
            new_value: json!(status),
        },
    );
    append_edits(&mut transaction, trip_id, std::slice::from_ref(&edit))
        .await
        .map_err(map_history_error)?;
    let revision = row.revision()?;
    let updated =
        sqlx::query("UPDATE trips SET status = ?, revision = ? WHERE id = ? AND revision = ?")
            .bind(status.as_ref())
            .bind(next_revision(revision).map_err(corrupt)?)
            .bind(trip_id)
            .bind(revision)
            .execute(&mut *transaction)
            .await
            .map_err(unavailable)?;
    ensure_one(updated.rows_affected())?;
    let (_, result) = load_validated_trip(&mut transaction, trip_id).await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(super) async fn update_candidate(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    candidate_id: &str,
    update: CandidateUpdate,
) -> Result<CandidateWithPlace, TripRepoError> {
    let CandidateUpdate {
        place,
        pitch,
        tags,
        changed_at,
        change_id,
    } = update;
    exact_required_text(&pitch, "candidate pitch is invalid", 2_000).map_err(corrupt)?;
    exact_bounded_strings(&tags, 20, 60).map_err(corrupt)?;
    let encoded_place = encode_place(&place)?;
    let tags_json = encode_tags(&tags)?;

    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    let actor = authorize_editor(&mut transaction, trip_id, authorization).await?;
    load_validated_trip(&mut transaction, trip_id).await?;
    let mut candidates = load_candidates(&mut transaction, trip_id).await?;
    let index = candidates
        .iter()
        .position(|candidate| candidate.candidate.id == candidate_id)
        .ok_or(TripRepoError::NotFound)?;
    let current = candidates[index].clone();
    if current.place.id == place.id {
        return Err(TripRepoError::CorruptData);
    }
    let source = match current.candidate.source_place_id.as_deref() {
        Some(source_id) => load_place(&mut transaction, trip_id, source_id).await?,
        None => None,
    };
    validate_candidate_place_revision(&current.place, &place).map_err(corrupt)?;
    let mut candidate = current.candidate.clone();
    candidate.place_id = place.id.clone();
    candidate.pitch = pitch;
    candidate.tags = tags;
    validate_stored_candidate(trip_id, &candidate).map_err(corrupt)?;
    validate_candidate_place_provenance(&candidate, &place, source.as_ref()).map_err(corrupt)?;

    let collision: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM trip_places WHERE trip_id = ? AND id = ?")
            .bind(trip_id)
            .bind(&place.id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(unavailable)?;
    if collision != 0 {
        return Err(TripRepoError::Conflict);
    }
    let revision: i64 =
        sqlx::query_scalar("SELECT revision FROM candidates WHERE trip_id = ? AND id = ?")
            .bind(trip_id)
            .bind(candidate_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(unavailable)?;
    checked_revision(revision).map_err(corrupt)?;

    let result = CandidateWithPlace {
        candidate: candidate.clone(),
        place: place.clone(),
    };
    candidates[index] = result.clone();
    ensure_candidate_budget(&candidates)?;

    let mut changes = vec![(
        "place",
        serde_json::to_value(&current.place).map_err(corrupt)?,
        serde_json::to_value(&place).map_err(corrupt)?,
    )];
    if current.candidate.pitch != candidate.pitch {
        changes.push((
            "pitch",
            json!(current.candidate.pitch),
            json!(candidate.pitch),
        ));
    }
    if current.candidate.tags != candidate.tags {
        changes.push(("tags", json!(current.candidate.tags), json!(candidate.tags)));
    }
    let edits = changes
        .into_iter()
        .enumerate()
        .map(|(index, (field, old_value, new_value))| {
            audit(
                trip_id,
                actor,
                &changed_at,
                &suffixed_id(&change_id, index),
                AuditChange {
                    entity: EditEntity::Candidate,
                    entity_id: candidate_id,
                    field,
                    old_value,
                    new_value,
                },
            )
        })
        .collect::<Vec<_>>();
    append_edits(&mut transaction, trip_id, &edits)
        .await
        .map_err(map_history_error)?;
    insert_place(&mut transaction, trip_id, &place, encoded_place).await?;
    let updated = sqlx::query(
        "UPDATE candidates SET place_id = ?, pitch = ?, tags_json = ?, revision = ? \
         WHERE trip_id = ? AND id = ? AND revision = ?",
    )
    .bind(&candidate.place_id)
    .bind(&candidate.pitch)
    .bind(tags_json)
    .bind(next_revision(revision).map_err(corrupt)?)
    .bind(trip_id)
    .bind(candidate_id)
    .bind(revision)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    ensure_one(updated.rows_affected())?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(super) async fn set_candidate_status(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    candidate_id: &str,
    status: CandidateDisposition,
    changed_at: &str,
    change_id: &str,
) -> Result<CandidateWithPlace, TripRepoError> {
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    let actor = authorize_editor(&mut transaction, trip_id, authorization).await?;
    load_validated_trip(&mut transaction, trip_id).await?;
    let mut candidates = load_candidates(&mut transaction, trip_id).await?;
    let index = candidates
        .iter()
        .position(|candidate| candidate.candidate.id == candidate_id)
        .ok_or(TripRepoError::NotFound)?;
    if candidates[index].candidate.status == CandidateStatus::InPlan {
        return Err(TripRepoError::Conflict);
    }
    let desired = CandidateStatus::from(status);
    let old = candidates[index].candidate.status;
    if old == desired {
        let result = candidates[index].clone();
        db.commit(transaction).await.map_err(unavailable)?;
        return Ok(result);
    }
    candidates[index].candidate.status = desired;
    ensure_candidate_budget(&candidates)?;
    let revision: i64 =
        sqlx::query_scalar("SELECT revision FROM candidates WHERE trip_id = ? AND id = ?")
            .bind(trip_id)
            .bind(candidate_id)
            .fetch_one(&mut *transaction)
            .await
            .map_err(unavailable)?;
    checked_revision(revision).map_err(corrupt)?;
    let edit = audit(
        trip_id,
        actor,
        changed_at,
        change_id,
        AuditChange {
            entity: EditEntity::Candidate,
            entity_id: candidate_id,
            field: "status",
            old_value: json!(old),
            new_value: json!(desired),
        },
    );
    append_edits(&mut transaction, trip_id, std::slice::from_ref(&edit))
        .await
        .map_err(map_history_error)?;
    let updated = sqlx::query(
        "UPDATE candidates SET status = ?, revision = ? \
         WHERE trip_id = ? AND id = ? AND revision = ?",
    )
    .bind(encode_candidate_status(desired))
    .bind(next_revision(revision).map_err(corrupt)?)
    .bind(trip_id)
    .bind(candidate_id)
    .bind(revision)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    ensure_one(updated.rows_affected())?;
    let result = candidates[index].clone();
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(super) async fn update_day(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    day_id: &str,
    patch: DayPatch,
    changed_at: &str,
    change_id: &str,
) -> Result<Day, TripRepoError> {
    if patch.window_start.is_none() && patch.window_end.is_none() && patch.city_hint.is_none() {
        return Err(TripRepoError::CorruptData);
    }
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    let actor = authorize_editor(&mut transaction, trip_id, authorization).await?;
    let mut detail = load_current_plan(&mut transaction, trip_id).await?;
    let day = detail
        .days
        .iter_mut()
        .find(|day| day.id == day_id)
        .ok_or(TripRepoError::NotFound)?;
    let mut changes = Vec::new();
    if let Some(value) = patch.window_start {
        local_time(&value).map_err(corrupt)?;
        if day.window_start != value {
            changes.push(("windowStart", json!(day.window_start), json!(value)));
            day.window_start = value;
        }
    }
    if let Some(value) = patch.window_end {
        local_time(&value).map_err(corrupt)?;
        if day.window_end != value {
            changes.push(("windowEnd", json!(day.window_end), json!(value)));
            day.window_end = value;
        }
    }
    if let Some(value) = patch.city_hint {
        exact_required_text(&value, "city hint is invalid", 120).map_err(corrupt)?;
        if day.city_hint != value {
            changes.push(("cityHint", json!(day.city_hint), json!(value)));
            day.city_hint = value;
        }
    }
    if time_window(&day.window_start, &day.window_end).is_err() {
        return Err(TripRepoError::Conflict);
    }
    if changes.is_empty() {
        let result = day.clone();
        db.commit(transaction).await.map_err(unavailable)?;
        return Ok(result);
    }
    validate_stored_plan_graph(
        &detail.plan,
        &detail.days,
        &detail.stops,
        trip_id,
        detail.plan.version,
    )
    .map_err(corrupt)?;
    let result = detail
        .days
        .iter()
        .find(|day| day.id == day_id)
        .cloned()
        .ok_or(TripRepoError::CorruptData)?;
    let revision = day_revision(&mut transaction, trip_id, detail.plan.version, day_id).await?;
    let edits = build_edits(
        trip_id,
        actor,
        changed_at,
        change_id,
        EditEntity::Day,
        day_id,
        changes,
    );
    append_edits(&mut transaction, trip_id, &edits)
        .await
        .map_err(map_history_error)?;
    let updated = sqlx::query(
        "UPDATE plan_days \
         SET window_start = ?, window_end = ?, city_hint = ?, revision = ? \
         WHERE trip_id = ? AND plan_version = ? AND id = ? AND revision = ?",
    )
    .bind(&result.window_start)
    .bind(&result.window_end)
    .bind(&result.city_hint)
    .bind(next_revision(revision).map_err(corrupt)?)
    .bind(trip_id)
    .bind(i64::from(detail.plan.version))
    .bind(day_id)
    .bind(revision)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    ensure_one(updated.rows_affected())?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(super) async fn update_stop(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    stop_id: &str,
    patch: StopPatch,
    changed_at: &str,
    change_id: &str,
) -> Result<Stop, TripRepoError> {
    if patch.planned_arrival.is_none()
        && patch.duration_min.is_none()
        && patch.notes.is_none()
        && patch.booking.is_none()
    {
        return Err(TripRepoError::CorruptData);
    }
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    let actor = authorize_editor(&mut transaction, trip_id, authorization).await?;
    let mut detail = load_current_plan(&mut transaction, trip_id).await?;
    let stop = detail
        .stops
        .iter_mut()
        .find(|stop| stop.id == stop_id)
        .ok_or(TripRepoError::NotFound)?;
    let mut changes = Vec::new();
    if let Some(value) = patch.planned_arrival {
        local_time(&value).map_err(corrupt)?;
        if stop.planned_arrival != value {
            changes.push(("plannedArrival", json!(stop.planned_arrival), json!(value)));
            stop.planned_arrival = value;
        }
    }
    if let Some(value) = patch.duration_min {
        duration_min(value).map_err(corrupt)?;
        if stop.duration_min != value {
            changes.push(("durationMin", json!(stop.duration_min), json!(value)));
            stop.duration_min = value;
        }
    }
    if let Some(value) = patch.notes {
        text_len(&value, 10_000).map_err(corrupt)?;
        if stop.notes != value {
            changes.push(("notes", json!(stop.notes), json!(value)));
            stop.notes = value;
        }
    }
    if let Some(value) = patch.booking {
        let current_link = booking_link(stop.booking.as_ref());
        let requested_link = booking_link(value.as_ref());
        if value.is_none() && current_link.is_some() {
            return Err(TripRepoError::Conflict);
        }
        if requested_link != current_link {
            return Err(TripRepoError::CorruptData);
        }
        validate_booking(value.as_ref()).map_err(corrupt)?;
        if stop.booking != value {
            changes.push(("booking", json!(stop.booking), json!(value)));
            stop.booking = value;
        }
    }
    if changes.is_empty() {
        let result = stop.clone();
        db.commit(transaction).await.map_err(unavailable)?;
        return Ok(result);
    }
    validate_stored_plan_graph(
        &detail.plan,
        &detail.days,
        &detail.stops,
        trip_id,
        detail.plan.version,
    )
    .map_err(corrupt)?;
    let result = detail
        .stops
        .iter()
        .find(|stop| stop.id == stop_id)
        .cloned()
        .ok_or(TripRepoError::CorruptData)?;
    let revision = stop_revision(&mut transaction, trip_id, detail.plan.version, stop_id).await?;
    let edits = build_edits(
        trip_id,
        actor,
        changed_at,
        change_id,
        EditEntity::Stop,
        stop_id,
        changes,
    );
    append_edits(&mut transaction, trip_id, &edits)
        .await
        .map_err(map_history_error)?;
    update_stop_row(
        &mut transaction,
        trip_id,
        detail.plan.version,
        &result,
        revision,
    )
    .await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

fn build_edits(
    trip_id: &str,
    actor: &itinera_core::domain::user::UserId,
    changed_at: &str,
    change_id: &str,
    entity: EditEntity,
    entity_id: &str,
    changes: Vec<(&'static str, Value, Value)>,
) -> Vec<Edit> {
    changes
        .into_iter()
        .enumerate()
        .map(|(index, (field, old_value, new_value))| {
            audit(
                trip_id,
                actor,
                changed_at,
                &suffixed_id(change_id, index),
                AuditChange {
                    entity,
                    entity_id,
                    field,
                    old_value,
                    new_value,
                },
            )
        })
        .collect()
}

async fn authorize_editor<'a>(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    authorization: &'a TripAuthorizationContext,
) -> Result<&'a itinera_core::domain::user::UserId, TripRepoError> {
    authorize(transaction, trip_id, authorization, RequiredRole::Editor).await?;
    authorization
        .human_user_id()
        .ok_or(TripRepoError::Forbidden)
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

async fn load_current_plan(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<PlanDetail, TripRepoError> {
    let (trip, _) = load_validated_trip(transaction, trip_id).await?;
    let (plan_id, version) = trip
        .current_plan_pointer()?
        .ok_or(TripRepoError::NotFound)?;
    load_plan_detail(transaction, trip_id, plan_id, version).await
}

async fn day_revision(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    version: u32,
    day_id: &str,
) -> Result<i64, TripRepoError> {
    let revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM plan_days WHERE trip_id = ? AND plan_version = ? AND id = ?",
    )
    .bind(trip_id)
    .bind(i64::from(version))
    .bind(day_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    checked_revision(revision).map_err(corrupt)
}

async fn stop_revision(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    version: u32,
    stop_id: &str,
) -> Result<i64, TripRepoError> {
    let revision: i64 = sqlx::query_scalar(
        "SELECT revision FROM plan_stops WHERE trip_id = ? AND plan_version = ? AND id = ?",
    )
    .bind(trip_id)
    .bind(i64::from(version))
    .bind(stop_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    checked_revision(revision).map_err(corrupt)
}

async fn update_stop_row(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    version: u32,
    stop: &Stop,
    revision: i64,
) -> Result<(), TripRepoError> {
    let booking = encode_booking_columns(stop.booking.as_ref())?;
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

fn booking_link(booking: Option<&Booking>) -> Option<&str> {
    booking.and_then(|booking| booking.ledger_entry_id.as_deref())
}

fn ensure_candidate_budget(candidates: &[CandidateWithPlace]) -> Result<(), TripRepoError> {
    if encoded_size(candidates)? > MAX_RESPONSE_BYTES {
        Err(TripRepoError::Conflict)
    } else {
        Ok(())
    }
}

fn map_history_error(error: ContentHistoryRepoError) -> TripRepoError {
    match error {
        ContentHistoryRepoError::Unavailable => TripRepoError::Unavailable,
        ContentHistoryRepoError::SafetyLimitExceeded | ContentHistoryRepoError::Conflict => {
            TripRepoError::Conflict
        }
        ContentHistoryRepoError::CorruptData
        | ContentHistoryRepoError::NotFound
        | ContentHistoryRepoError::Forbidden
        | ContentHistoryRepoError::Unsupported => TripRepoError::CorruptData,
    }
}

fn ensure_one(rows: u64) -> Result<(), TripRepoError> {
    if rows == 1 {
        Ok(())
    } else {
        Err(TripRepoError::Conflict)
    }
}

fn unavailable<T>(_error: T) -> TripRepoError {
    TripRepoError::Unavailable
}

fn corrupt<T>(_error: T) -> TripRepoError {
    TripRepoError::CorruptData
}
