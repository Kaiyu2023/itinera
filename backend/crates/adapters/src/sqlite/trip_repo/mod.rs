//! SQLite trip/member/invite repository facade.
//!
//! Only this first capability slice is implemented here. Candidate, plan, and
//! history methods fail closed until their own migrations and reviewed
//! repositories land. The clean-break branch has no persistence-backed runtime
//! until the complete SQLite adapter set is ready for cutover.

use async_trait::async_trait;
use itinera_core::{
    domain::{
        trip::{
            Candidate, CandidateDisposition, CandidateWithPlace, Day, DayPatch, Invite,
            PendingInvite, Place, Plan, PlanDetail, Stop, StopPatch, Trip, TripStatus, TripSummary,
        },
        user::{User, UserId},
    },
    ports::{
        trip::{CandidateUpdate, TripRepo, TripRepoError},
        user::UserRepo,
    },
};

use super::{SqliteDb, codec::validate_id};

mod access;
mod memberships;
mod records;
mod trips;

#[derive(Clone)]
pub struct SqliteTripRepo {
    db: SqliteDb,
}

impl SqliteTripRepo {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &SqliteDb {
        &self.db
    }
}

#[async_trait]
impl TripRepo for SqliteTripRepo {
    async fn create_trip(&self, trip: Trip) -> Result<Trip, TripRepoError> {
        trips::create_trip(&self.db, trip).await
    }

    async fn list_trips(&self, actor: &UserId) -> Result<Vec<TripSummary>, TripRepoError> {
        trips::list_trips(&self.db, actor).await
    }

    async fn get_trip(&self, trip_id: &str, actor: &UserId) -> Result<Trip, TripRepoError> {
        validate_requested_id(trip_id)?;
        trips::get_trip(&self.db, trip_id, actor).await
    }

    async fn set_trip_status(
        &self,
        _trip_id: &str,
        _actor: &UserId,
        _status: TripStatus,
        _changed_at: &str,
        _change_id: &str,
    ) -> Result<Trip, TripRepoError> {
        // Status writes must append authoritative history in the same
        // transaction, and history is intentionally a later capability slice.
        Err(TripRepoError::Unavailable)
    }

    async fn get_members(
        &self,
        trip_id: &str,
        actor: &UserId,
        _users: &dyn UserRepo,
    ) -> Result<Vec<User>, TripRepoError> {
        validate_requested_id(trip_id)?;
        // Deliberately ignore the legacy composition argument. SQLite joins
        // membership, profile, and email claim in this repository transaction
        // so authorization and returned profiles share one snapshot.
        memberships::get_members(&self.db, trip_id, actor).await
    }

    async fn remove_member(
        &self,
        trip_id: &str,
        actor: &UserId,
        target: &UserId,
    ) -> Result<(), TripRepoError> {
        validate_requested_id(trip_id)?;
        validate_requested_id(&target.0)?;
        memberships::remove_member(&self.db, trip_id, actor, target).await
    }

    async fn create_invite(
        &self,
        trip_id: &str,
        actor: &UserId,
        invite: PendingInvite,
    ) -> Result<Invite, TripRepoError> {
        validate_requested_id(trip_id)?;
        memberships::create_invite(&self.db, trip_id, actor, invite).await
    }

    async fn accept_pending_invites(
        &self,
        user: &User,
        joined_at: &str,
    ) -> Result<(), TripRepoError> {
        memberships::accept_pending_invites(&self.db, user, joined_at).await
    }

    async fn search_saved_places(
        &self,
        _trip_id: &str,
        _actor: &UserId,
        _query: &str,
    ) -> Result<Vec<Place>, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn find_place(
        &self,
        _trip_id: &str,
        _actor: &UserId,
        _place_id: &str,
    ) -> Result<Option<Place>, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn list_candidates(
        &self,
        _trip_id: &str,
        _actor: &UserId,
    ) -> Result<Vec<CandidateWithPlace>, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn add_candidate(
        &self,
        _trip_id: &str,
        _actor: &UserId,
        _candidate: Candidate,
        _place: Place,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn update_candidate(
        &self,
        _trip_id: &str,
        _actor: &UserId,
        _candidate_id: &str,
        _update: CandidateUpdate,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn set_candidate_status(
        &self,
        _trip_id: &str,
        _actor: &UserId,
        _candidate_id: &str,
        _status: CandidateDisposition,
        _changed_at: &str,
        _change_id: &str,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn get_current_plan(
        &self,
        _trip_id: &str,
        _actor: &UserId,
    ) -> Result<PlanDetail, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn initialize_plan(
        &self,
        _trip_id: &str,
        _actor: &UserId,
        _anchor_place_id: &str,
        _plan: Plan,
        _days: Vec<Day>,
    ) -> Result<PlanDetail, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn list_plan_versions(
        &self,
        _trip_id: &str,
        _actor: &UserId,
    ) -> Result<Vec<Plan>, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn update_day(
        &self,
        _trip_id: &str,
        _actor: &UserId,
        _day_id: &str,
        _patch: DayPatch,
        _changed_at: &str,
        _change_id: &str,
    ) -> Result<Day, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn update_stop(
        &self,
        _trip_id: &str,
        _actor: &UserId,
        _stop_id: &str,
        _patch: StopPatch,
        _changed_at: &str,
        _change_id: &str,
    ) -> Result<Stop, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }
}

/// Resource IDs in these methods come from URL path segments. Keep malformed
/// and unknown resources indistinguishable at the repository boundary; actor
/// IDs and IDs decoded from rows are trusted-state invariants and continue to
/// fail as corruption inside the capability modules.
fn validate_requested_id(value: &str) -> Result<(), TripRepoError> {
    validate_id(value).map_err(|_| TripRepoError::NotFound)
}
