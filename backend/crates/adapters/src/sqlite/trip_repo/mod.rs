//! SQLite trip/member/invite repository facade.
//!
//! Users, trips, members, invites, candidate creation/reads, and plan-v1
//! initialization/reads are authoritative here. Mutations that must append
//! content history remain unavailable until that capability's reviewed
//! migration lands. The clean-break branch has no persistence-backed runtime
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
        authorization::TripAuthorizationContext,
        trip::{CandidateUpdate, TripRepo, TripRepoError},
    },
};

use super::{SqliteDb, codec::validate_id};

mod access;
mod candidate_records;
mod candidates;
mod memberships;
mod plan_records;
mod plans;
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
    async fn create_trip(
        &self,
        authorization: &TripAuthorizationContext,
        trip: Trip,
    ) -> Result<Trip, TripRepoError> {
        trips::create_trip(&self.db, authorization, trip).await
    }

    async fn list_trips(
        &self,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<TripSummary>, TripRepoError> {
        trips::list_trips(&self.db, authorization).await
    }

    async fn get_trip(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Trip, TripRepoError> {
        validate_requested_id(trip_id)?;
        trips::get_trip(&self.db, trip_id, authorization).await
    }

    async fn set_trip_status(
        &self,
        _trip_id: &str,
        _authorization: &TripAuthorizationContext,
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
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<User>, TripRepoError> {
        validate_requested_id(trip_id)?;
        // SQLite joins membership, profile, and email claim in this repository
        // transaction so authorization and returned profiles share one snapshot.
        memberships::get_members(&self.db, trip_id, authorization).await
    }

    async fn remove_member(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        target: &UserId,
    ) -> Result<(), TripRepoError> {
        validate_requested_id(trip_id)?;
        validate_requested_id(&target.0)?;
        memberships::remove_member(&self.db, trip_id, authorization, target).await
    }

    async fn create_invite(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        invite: PendingInvite,
    ) -> Result<Invite, TripRepoError> {
        validate_requested_id(trip_id)?;
        memberships::create_invite(&self.db, trip_id, authorization, invite).await
    }

    async fn accept_pending_invites(
        &self,
        authorization: &TripAuthorizationContext,
        user: &User,
        joined_at: &str,
    ) -> Result<(), TripRepoError> {
        memberships::accept_pending_invites(&self.db, authorization, user, joined_at).await
    }

    async fn search_saved_places(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        query: &str,
    ) -> Result<Vec<Place>, TripRepoError> {
        validate_requested_id(trip_id)?;
        candidates::search_saved_places(&self.db, trip_id, authorization, query).await
    }

    async fn find_place(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        place_id: &str,
    ) -> Result<Option<Place>, TripRepoError> {
        validate_requested_id(trip_id)?;
        validate_requested_id(place_id)?;
        candidates::find_place(&self.db, trip_id, authorization, place_id).await
    }

    async fn list_candidates(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<CandidateWithPlace>, TripRepoError> {
        validate_requested_id(trip_id)?;
        candidates::list_candidates(&self.db, trip_id, authorization).await
    }

    async fn add_candidate(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        candidate: Candidate,
        place: Place,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        validate_requested_id(trip_id)?;
        candidates::add_candidate(&self.db, trip_id, authorization, candidate, place).await
    }

    async fn update_candidate(
        &self,
        _trip_id: &str,
        _authorization: &TripAuthorizationContext,
        _candidate_id: &str,
        _update: CandidateUpdate,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn set_candidate_status(
        &self,
        _trip_id: &str,
        _authorization: &TripAuthorizationContext,
        _candidate_id: &str,
        _status: CandidateDisposition,
        _changed_at: &str,
        _change_id: &str,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        Err(TripRepoError::Unavailable)
    }

    async fn get_current_plan(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<PlanDetail, TripRepoError> {
        validate_requested_id(trip_id)?;
        plans::get_current_plan(&self.db, trip_id, authorization).await
    }

    async fn initialize_plan(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        anchor_place_id: &str,
        plan: Plan,
        days: Vec<Day>,
    ) -> Result<PlanDetail, TripRepoError> {
        validate_requested_id(trip_id)?;
        validate_requested_id(anchor_place_id)?;
        plans::initialize_plan(
            &self.db,
            trip_id,
            authorization,
            anchor_place_id,
            plan,
            days,
        )
        .await
    }

    async fn list_plan_versions(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<Plan>, TripRepoError> {
        validate_requested_id(trip_id)?;
        plans::list_plan_versions(&self.db, trip_id, authorization).await
    }

    async fn update_day(
        &self,
        _trip_id: &str,
        _authorization: &TripAuthorizationContext,
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
        _authorization: &TripAuthorizationContext,
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
