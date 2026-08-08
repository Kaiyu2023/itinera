use async_trait::async_trait;

use crate::domain::{
    trip::{
        Candidate, CandidateDisposition, CandidateWithPlace, Day, DayPatch, Invite, PendingInvite,
        Place, Plan, PlanDetail, Stop, StopPatch, Trip, TripStatus, TripSummary,
    },
    user::{Email, User, UserId},
};

use super::authorization::TripAuthorizationContext;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TripRepoError {
    #[error("trip storage is unavailable")]
    Unavailable,
    #[error("trip storage returned corrupt data")]
    CorruptData,
    #[error("resource not found")]
    NotFound,
    #[error("the actor does not have permission for this operation")]
    Forbidden,
    #[error("the operation conflicts with current state")]
    Conflict,
    #[error("an invite for this email already exists")]
    DuplicateInvite,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CandidateUpdate {
    pub place: Place,
    pub pitch: String,
    pub tags: Vec<String>,
    pub changed_at: String,
    pub change_id: String,
}

#[async_trait]
pub trait TripRepo: Send + Sync {
    async fn create_trip(
        &self,
        authorization: &TripAuthorizationContext,
        trip: Trip,
    ) -> Result<Trip, TripRepoError>;
    async fn list_trips(
        &self,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<TripSummary>, TripRepoError>;
    async fn get_trip(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Trip, TripRepoError>;
    async fn set_trip_status(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        status: TripStatus,
        changed_at: &str,
        change_id: &str,
    ) -> Result<Trip, TripRepoError>;
    async fn get_members(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<User>, TripRepoError>;
    async fn remove_member(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        target: &UserId,
    ) -> Result<(), TripRepoError>;
    async fn create_invite(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        invite: PendingInvite,
    ) -> Result<Invite, TripRepoError>;
    async fn accept_pending_invites(
        &self,
        authorization: &TripAuthorizationContext,
        user: &User,
        joined_at: &str,
    ) -> Result<(), TripRepoError>;

    async fn search_saved_places(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        query: &str,
    ) -> Result<Vec<Place>, TripRepoError>;
    async fn find_place(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        place_id: &str,
    ) -> Result<Option<Place>, TripRepoError>;
    async fn list_candidates(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<CandidateWithPlace>, TripRepoError>;
    async fn add_candidate(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        candidate: Candidate,
        place: Place,
    ) -> Result<CandidateWithPlace, TripRepoError>;
    async fn update_candidate(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        candidate_id: &str,
        update: CandidateUpdate,
    ) -> Result<CandidateWithPlace, TripRepoError>;
    async fn set_candidate_status(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        candidate_id: &str,
        status: CandidateDisposition,
        changed_at: &str,
        change_id: &str,
    ) -> Result<CandidateWithPlace, TripRepoError>;

    async fn get_current_plan(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<PlanDetail, TripRepoError>;
    async fn initialize_plan(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        anchor_place_id: &str,
        plan: Plan,
        days: Vec<Day>,
    ) -> Result<PlanDetail, TripRepoError>;
    async fn list_plan_versions(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<Plan>, TripRepoError>;
    async fn update_day(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        day_id: &str,
        patch: DayPatch,
        changed_at: &str,
        change_id: &str,
    ) -> Result<Day, TripRepoError>;
    async fn update_stop(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        stop_id: &str,
        patch: StopPatch,
        changed_at: &str,
        change_id: &str,
    ) -> Result<Stop, TripRepoError>;
}

pub fn canonical_invitee(email: &Email) -> &str {
    email.as_str()
}
