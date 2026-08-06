use crate::{
    domain::{
        trip::{Invite, InviteStatus, Trip, TripMember, TripRole, TripStatus, TripSummary},
        user::{Email, User, UserId},
    },
    ports::{
        access_policy::{AccessPolicy, AccessPolicyError},
        clock::Clock,
        id_gen::IdGen,
        trip::{TripRepo, TripRepoError},
        user::{UserRepo, UserRepoError},
    },
};

use super::validation::{ValidationError, currency, required_text, trip_dates};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTripInput {
    pub name: String,
    pub start_date: String,
    pub end_date: String,
    pub base_currency: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TripServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] TripRepoError),
    #[error(transparent)]
    UserRepository(#[from] UserRepoError),
    #[error(transparent)]
    AccessPolicy(#[from] AccessPolicyError),
    #[error("invalid invite email")]
    InvalidEmail,
}

pub async fn create_trip(
    repo: &dyn TripRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    actor: &User,
    input: CreateTripInput,
) -> Result<Trip, TripServiceError> {
    let name = required_text(
        input.name,
        "name is required and must be at most 120 characters",
        120,
    )?;
    trip_dates(&input.start_date, &input.end_date)?;
    let base_currency = currency(input.base_currency)?;
    let created_at = clock.now();
    let member = TripMember {
        user_id: actor.id.0.clone(),
        role: TripRole::Leader,
        joined_at: created_at.clone(),
    };
    let trip = Trip {
        id: ids.new_id(),
        name,
        cover_photo_url: None,
        accent_color: None,
        stop_kind_labels: None,
        status: TripStatus::Dreaming,
        start_date: input.start_date,
        end_date: input.end_date,
        base_currency,
        soft_budget: None,
        members: vec![member],
        current_plan_id: None,
        created_at,
    };
    repo.create_trip(trip).await.map_err(Into::into)
}

pub async fn list_trips(
    repo: &dyn TripRepo,
    actor: &UserId,
) -> Result<Vec<TripSummary>, TripServiceError> {
    repo.list_trips(actor).await.map_err(Into::into)
}

pub async fn get_trip(
    repo: &dyn TripRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Trip, TripServiceError> {
    repo.get_trip(trip_id, actor).await.map_err(Into::into)
}

pub async fn set_trip_status(
    repo: &dyn TripRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    status: TripStatus,
) -> Result<Trip, TripServiceError> {
    repo.set_trip_status(trip_id, actor, status, &clock.now(), &ids.new_id())
        .await
        .map_err(Into::into)
}

pub async fn get_members(
    repo: &dyn TripRepo,
    users: &dyn UserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Vec<User>, TripServiceError> {
    repo.get_members(trip_id, actor, users)
        .await
        .map_err(Into::into)
}

pub async fn invite(
    repo: &dyn TripRepo,
    access_policy: &dyn AccessPolicy,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    raw_email: &str,
) -> Result<Invite, TripServiceError> {
    let email = Email::parse(raw_email).map_err(|_| TripServiceError::InvalidEmail)?;

    let trip = repo.get_trip(trip_id, actor).await?;
    if !trip
        .members
        .iter()
        .any(|member| member.user_id == actor.0 && member.role == TripRole::Leader)
    {
        return Err(TripRepoError::Forbidden.into());
    }

    // Grant first. If the provider is not configured or unavailable, no
    // pending record is created that the invitee cannot use. A successful
    // grant followed by a storage failure is safe: login alone never grants
    // access to a trip, and retrying the grant is required to be idempotent.
    access_policy.grant_login(&email).await?;

    let invite = Invite {
        id: ids.new_id(),
        trip_id: trip_id.to_string(),
        email: email.to_string(),
        invited_by: actor.0.clone(),
        status: InviteStatus::Pending,
        created_at: clock.now(),
    };
    repo.create_invite(trip_id, actor, invite)
        .await
        .map_err(Into::into)
}

pub async fn remove_member(
    repo: &dyn TripRepo,
    trip_id: &str,
    actor: &UserId,
    target: &UserId,
) -> Result<(), TripServiceError> {
    repo.remove_member(trip_id, actor, target)
        .await
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_input_normalises_currency_but_not_dates() {
        assert_eq!(currency(" gbp ".into()).expect("valid"), "GBP");
        assert!(trip_dates("2026-08-01", "2026-08-03").is_ok());
    }
}
