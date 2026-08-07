use std::collections::HashSet;

use chrono::DateTime;

use crate::{
    domain::{
        trip::{Day, DayPatch, Plan, PlanDetail, Stop, StopPatch},
        user::UserId,
    },
    ports::{
        clock::Clock,
        id_gen::IdGen,
        trip::{TripRepo, TripRepoError},
    },
};

use super::validation::{
    ValidationError, date, duration_min, exact_required_text, local_time, normalise_booking,
    required_text, text_len, time_window, trip_dates, validate_booking,
};

#[derive(Debug, thiserror::Error)]
pub enum PlanServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] TripRepoError),
}

/// Validates the canonical persisted shape of one plan and its owned day/stop graph.
///
/// Repositories that use a plan row as an authorization or concurrency input must
/// call this before trusting its identifiers or revisions. Place ownership is a
/// separate repository concern because place snapshots do not live under the plan
/// sort-key prefix.
pub fn validate_stored_plan_graph(
    plan: &Plan,
    days: &[Day],
    stops: &[Stop],
    expected_trip_id: &str,
    expected_version: u32,
) -> Result<(), ValidationError> {
    if plan.trip_id != expected_trip_id
        || plan.version == 0
        || plan.version != expected_version
        || exact_required_text(&plan.id, "plan id is invalid", 200).is_err()
        || plan
            .created_from_proposal_id
            .as_deref()
            .is_some_and(|id| exact_required_text(id, "proposal id is invalid", 200).is_err())
        || !is_utc_instant(&plan.created_at)
        || days.is_empty()
    {
        return Err(ValidationError("stored plan is invalid"));
    }

    let mut day_ids = HashSet::new();
    let mut day_dates = HashSet::new();
    if days.iter().any(|day| {
        day.plan_id != plan.id
            || !day_ids.insert(day.id.as_str())
            || !day_dates.insert(day.date.as_str())
            || exact_required_text(&day.id, "day id is invalid", 200).is_err()
            || date(&day.date).is_err()
            || exact_required_text(&day.city_hint, "city hint is invalid", 120).is_err()
            || exact_required_text(&day.tz, "time zone is invalid", 100).is_err()
            || time_window(&day.window_start, &day.window_end).is_err()
    }) {
        return Err(ValidationError("stored day is invalid"));
    }

    let mut stop_ids = HashSet::new();
    if stops.iter().any(|stop| {
        !day_ids.contains(stop.day_id.as_str())
            || !stop_ids.insert(stop.id.as_str())
            || exact_required_text(&stop.id, "stop id is invalid", 200).is_err()
            || exact_required_text(&stop.place_id, "place id is invalid", 200).is_err()
            || !stop.seq.is_finite()
            || stop.seq <= 0.0
            || stop.seq > 1_000_000.0
            || stop.seq.fract() != 0.0
            || local_time(&stop.planned_arrival).is_err()
            || duration_min(stop.duration_min).is_err()
            || text_len(&stop.notes, 10_000).is_err()
            || validate_booking(stop.booking.as_ref()).is_err()
    }) {
        return Err(ValidationError("stored stop is invalid"));
    }
    Ok(())
}

fn is_utc_instant(value: &str) -> bool {
    value.len() <= 64
        && value.ends_with('Z')
        && DateTime::parse_from_rfc3339(value)
            .is_ok_and(|timestamp| timestamp.offset().local_minus_utc() == 0)
}

pub async fn get_current_plan(
    repo: &dyn TripRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<PlanDetail, PlanServiceError> {
    repo.get_current_plan(trip_id, actor)
        .await
        .map_err(Into::into)
}

pub async fn initialize_plan(
    repo: &dyn TripRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    anchor_place_id: &str,
) -> Result<PlanDetail, PlanServiceError> {
    let anchor_place_id = required_text(
        anchor_place_id.to_string(),
        "anchorPlaceId is required",
        200,
    )?;
    match repo.get_current_plan(trip_id, actor).await {
        Ok(existing) => return Ok(existing),
        Err(TripRepoError::NotFound) => {}
        Err(error) => return Err(error.into()),
    }
    let trip = repo.get_trip(trip_id, actor).await?;
    let candidates = repo.list_candidates(trip_id, actor).await?;
    let anchor = candidates
        .into_iter()
        .find(|candidate| {
            candidate.candidate.place_id == anchor_place_id
                && candidate.candidate.status == crate::domain::trip::CandidateStatus::Shortlisted
        })
        .ok_or(TripRepoError::NotFound)?;

    let created_at = clock.now();
    let plan = Plan {
        id: ids.new_id(),
        trip_id: trip_id.to_string(),
        version: 1,
        created_from_proposal_id: None,
        created_at,
    };
    let days = trip_dates(&trip.start_date, &trip.end_date)?
        .into_iter()
        .map(|date| Day {
            id: ids.new_id(),
            plan_id: plan.id.clone(),
            date: date.format("%Y-%m-%d").to_string(),
            city_hint: anchor.place.city.clone(),
            tz: anchor.place.tz.clone(),
            window_start: "09:00".to_string(),
            window_end: "21:00".to_string(),
        })
        .collect();

    repo.initialize_plan(trip_id, actor, &anchor_place_id, plan, days)
        .await
        .map_err(Into::into)
}

pub async fn list_plan_versions(
    repo: &dyn TripRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Vec<Plan>, PlanServiceError> {
    repo.list_plan_versions(trip_id, actor)
        .await
        .map_err(Into::into)
}

pub async fn update_day(
    repo: &dyn TripRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    day_id: &str,
    mut patch: DayPatch,
) -> Result<Day, PlanServiceError> {
    if patch.window_start.is_none() && patch.window_end.is_none() && patch.city_hint.is_none() {
        return Err(ValidationError("at least one day field is required").into());
    }
    if let Some(value) = patch.window_start.as_deref() {
        local_time(value)?;
    }
    if let Some(value) = patch.window_end.as_deref() {
        local_time(value)?;
    }
    if let Some(city) = patch.city_hint.take() {
        patch.city_hint = Some(required_text(city, "cityHint must not be empty", 120)?);
    }
    let current = repo.get_current_plan(trip_id, actor).await?;
    let day = current
        .days
        .into_iter()
        .find(|day| day.id == day_id)
        .ok_or(TripRepoError::NotFound)?;
    time_window(
        patch.window_start.as_deref().unwrap_or(&day.window_start),
        patch.window_end.as_deref().unwrap_or(&day.window_end),
    )?;
    repo.update_day(trip_id, actor, day_id, patch, &clock.now(), &ids.new_id())
        .await
        .map_err(Into::into)
}

pub async fn update_stop(
    repo: &dyn TripRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    stop_id: &str,
    mut patch: StopPatch,
) -> Result<Stop, PlanServiceError> {
    if patch.planned_arrival.is_none()
        && patch.duration_min.is_none()
        && patch.notes.is_none()
        && patch.booking.is_none()
    {
        return Err(ValidationError("at least one stop field is required").into());
    }
    if let Some(value) = patch.planned_arrival.as_deref() {
        local_time(value)?;
    }
    if let Some(duration) = patch.duration_min {
        duration_min(duration)?;
    }
    if let Some(notes) = patch.notes.take() {
        text_len(&notes, 10_000)?;
        patch.notes = Some(notes);
    }
    if let Some(requested_booking) = patch.booking.as_mut() {
        let current = repo.get_current_plan(trip_id, actor).await?;
        let current_stop = current
            .stops
            .iter()
            .find(|stop| stop.id == stop_id)
            .ok_or(TripRepoError::NotFound)?;
        let current_link = current_stop
            .booking
            .as_ref()
            .and_then(|booking| booking.ledger_entry_id.clone());
        match requested_booking {
            Some(booking) => {
                if booking.ledger_entry_id.is_some() {
                    return Err(ValidationError("booking.ledgerEntryId is server-owned").into());
                }
                booking.ledger_entry_id = current_link;
                *booking = normalise_booking(booking.clone())?;
            }
            None if current_link.is_some() => {
                return Err(TripRepoError::Conflict.into());
            }
            None => {}
        }
    }
    repo.update_stop(trip_id, actor, stop_id, patch, &clock.now(), &ids.new_id())
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use crate::domain::trip::StopKind;

    use super::*;

    fn graph() -> (Plan, Vec<Day>, Vec<Stop>) {
        let plan = Plan {
            id: "plan-a".into(),
            trip_id: "trip-a".into(),
            version: 1,
            created_from_proposal_id: None,
            created_at: "2026-08-06T10:00:00Z".into(),
        };
        let days = vec![Day {
            id: "day-a".into(),
            plan_id: plan.id.clone(),
            date: "2026-11-01".into(),
            city_hint: "Kyoto".into(),
            tz: "Asia/Tokyo".into(),
            window_start: "09:00".into(),
            window_end: "21:00".into(),
        }];
        let stops = vec![Stop {
            id: "stop-a".into(),
            day_id: days[0].id.clone(),
            place_id: "place-a".into(),
            seq: 1.0,
            stop_kind: StopKind::Visit,
            planned_arrival: "10:00".into(),
            duration_min: 60,
            notes: String::new(),
            booking: None,
        }];
        (plan, days, stops)
    }

    #[test]
    fn stored_plan_graph_validation_covers_every_persisted_shape() {
        let (plan, days, stops) = graph();
        assert!(validate_stored_plan_graph(&plan, &days, &stops, "trip-a", 1).is_ok());

        let mut malformed = plan.clone();
        malformed.created_at = "2026-08-06T11:00:00+01:00".into();
        assert!(validate_stored_plan_graph(&malformed, &days, &stops, "trip-a", 1).is_err());

        let mut malformed = days.clone();
        malformed[0].window_end = "08:00".into();
        assert!(validate_stored_plan_graph(&plan, &malformed, &stops, "trip-a", 1).is_err());

        let mut malformed = stops;
        malformed[0].planned_arrival = "9:00".into();
        assert!(validate_stored_plan_graph(&plan, &days, &malformed, "trip-a", 1).is_err());
    }
}
