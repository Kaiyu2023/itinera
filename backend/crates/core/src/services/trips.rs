use crate::{
    domain::{
        trip::{
            Invite, NewInviteInput, NewTripInput, Trip, TripRole, TripStatus, TripSummary,
            TripValidationError,
        },
        user::{Email, User, UserId},
    },
    ports::{
        access_policy::{AccessPolicy, AccessPolicyError},
        authorization::TripAuthorizationContext,
        clock::Clock,
        id_gen::IdGen,
        trip::{TripRepo, TripRepoError},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTripInput {
    pub name: String,
    pub start_date: String,
    pub end_date: String,
    pub base_currency: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TripServiceError {
    #[error("{0}")]
    Validation(TripValidationError),
    #[error("server-created trip data violated a domain invariant: {0}")]
    InternalInvariant(TripValidationError),
    #[error(transparent)]
    Repository(#[from] TripRepoError),
    #[error(transparent)]
    AccessPolicy(#[from] AccessPolicyError),
    #[error("invalid invite email")]
    InvalidEmail,
}

pub async fn create_trip(
    repo: &dyn TripRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    authorization: &TripAuthorizationContext,
    input: CreateTripInput,
) -> Result<Trip, TripServiceError> {
    let actor = require_human(authorization)?;
    let created_at = clock.now();
    let trip = Trip::create(NewTripInput {
        id: ids.new_id(),
        name: input.name,
        start_date: input.start_date,
        end_date: input.end_date,
        base_currency: input.base_currency,
        creator_id: actor.0.clone(),
        created_at,
    })
    .map_err(classify_create_error)?;
    repo.create_trip(authorization, trip)
        .await
        .map_err(Into::into)
}

pub async fn list_trips(
    repo: &dyn TripRepo,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<TripSummary>, TripServiceError> {
    repo.list_trips(authorization).await.map_err(Into::into)
}

pub async fn get_trip(
    repo: &dyn TripRepo,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Trip, TripServiceError> {
    repo.get_trip(trip_id, authorization)
        .await
        .map_err(Into::into)
}

pub async fn set_trip_status(
    repo: &dyn TripRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    status: TripStatus,
) -> Result<Trip, TripServiceError> {
    require_human(authorization)?;
    repo.set_trip_status(trip_id, authorization, status, &clock.now(), &ids.new_id())
        .await
        .map_err(Into::into)
}

pub async fn get_members(
    repo: &dyn TripRepo,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<User>, TripServiceError> {
    repo.get_members(trip_id, authorization)
        .await
        .map_err(Into::into)
}

pub async fn invite(
    repo: &dyn TripRepo,
    access_policy: &dyn AccessPolicy,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    raw_email: &str,
) -> Result<Invite, TripServiceError> {
    let actor = require_human(authorization)?;
    let email = Email::parse(raw_email).map_err(|_| TripServiceError::InvalidEmail)?;

    let trip = repo.get_trip(trip_id, authorization).await?;
    if !trip
        .members()
        .iter()
        .any(|member| member.user_id() == actor.0 && member.role() == TripRole::Leader)
    {
        return Err(TripRepoError::Forbidden.into());
    }

    let invite = Invite::create(NewInviteInput {
        id: ids.new_id(),
        trip_id: trip_id.to_string(),
        email: email.clone(),
        invited_by: actor.0.clone(),
        created_at: clock.now(),
    })
    .map_err(TripServiceError::InternalInvariant)?;

    // Validate every local invariant before touching the external policy.
    // Persist only after the grant succeeds, so a provider failure cannot
    // leave a pending invite that the invitee cannot use. A successful grant
    // followed by a storage failure remains safe: login alone never grants
    // trip access, and grant retries are required to be idempotent.
    access_policy.grant_login(&email).await?;

    repo.create_invite(trip_id, authorization, invite)
        .await
        .map_err(Into::into)
}

pub async fn remove_member(
    repo: &dyn TripRepo,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    target: &UserId,
) -> Result<(), TripServiceError> {
    require_human(authorization)?;
    repo.remove_member(trip_id, authorization, target)
        .await
        .map_err(Into::into)
}

pub async fn accept_pending_invites(
    repo: &dyn TripRepo,
    authorization: &TripAuthorizationContext,
    user: &User,
    joined_at: &str,
) -> Result<(), TripServiceError> {
    let actor = require_human(authorization)?;
    if actor != &user.id {
        return Err(TripRepoError::Forbidden.into());
    }
    repo.accept_pending_invites(authorization, user, joined_at)
        .await
        .map_err(Into::into)
}

fn require_human(authorization: &TripAuthorizationContext) -> Result<&UserId, TripServiceError> {
    authorization
        .human_user_id()
        .ok_or_else(|| TripRepoError::Forbidden.into())
}

fn classify_create_error(error: TripValidationError) -> TripServiceError {
    match error {
        TripValidationError::InvalidName
        | TripValidationError::InvalidDate
        | TripValidationError::EndBeforeStart
        | TripValidationError::DateRangeTooLong
        | TripValidationError::InvalidCurrency(_) => TripServiceError::Validation(error),
        _ => TripServiceError::InternalInvariant(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_caller_owned_creation_fields_become_bad_requests() {
        for error in [
            TripValidationError::InvalidName,
            TripValidationError::InvalidDate,
            TripValidationError::EndBeforeStart,
            TripValidationError::DateRangeTooLong,
            TripValidationError::InvalidCurrency(crate::domain::currency::InvalidCurrencyCode),
        ] {
            assert!(matches!(
                classify_create_error(error),
                TripServiceError::Validation(found) if found == error
            ));
        }

        for error in [
            TripValidationError::InvalidTripId,
            TripValidationError::InvalidMemberId,
            TripValidationError::InvalidCreatedAt,
        ] {
            assert!(matches!(
                classify_create_error(error),
                TripServiceError::InternalInvariant(found) if found == error
            ));
        }
    }
}
