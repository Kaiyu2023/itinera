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
    ValidationError, currency, http_url, local_time, optional_text, required_text, time_window,
    trip_dates,
};

#[derive(Debug, thiserror::Error)]
pub enum PlanServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] TripRepoError),
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
    if patch.duration_min == Some(0) {
        return Err(ValidationError("durationMin must be greater than zero").into());
    }
    if patch.duration_min.is_some_and(|duration| duration > 1_440) {
        return Err(ValidationError("durationMin must not exceed 1,440").into());
    }
    if let Some(notes) = patch.notes.take() {
        if notes.chars().count() > 10_000 {
            return Err(ValidationError("notes must be at most 10,000 characters").into());
        }
        patch.notes = Some(notes);
    }
    if let Some(Some(booking)) = patch.booking.as_mut() {
        booking.reference = required_text(
            std::mem::take(&mut booking.reference),
            "booking ref is required and must be at most 200 characters",
            200,
        )?;
        booking.url = http_url(booking.url.take())?;
        booking.ledger_entry_id = optional_text(booking.ledger_entry_id.take(), 200)?;
        if let Some(cost) = booking.cost.as_mut() {
            if !cost.amount.is_finite() || cost.amount < 0.0 {
                return Err(ValidationError("booking cost must be a non-negative number").into());
            }
            cost.currency = currency(std::mem::take(&mut cost.currency))?;
        }
    }
    repo.update_stop(trip_id, actor, stop_id, patch, &clock.now(), &ids.new_id())
        .await
        .map_err(Into::into)
}
