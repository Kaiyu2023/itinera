//! DynamoDB implementation of the trip repository port.
//!
//! This module is an explicit facade. Capability modules own complete use-case
//! transactions, while shared record, access, and storage primitives remain
//! private to the adapter.

use async_trait::async_trait;
use itinera_core::{
    domain::{
        trip::{
            Candidate, CandidateDisposition, CandidateWithPlace, Day, DayPatch, Invite, Place,
            Plan, PlanDetail, Stop, StopPatch, Trip, TripStatus, TripSummary,
        },
        user::{User, UserId},
    },
    ports::{
        trip::{CandidateUpdate, TripRepo, TripRepoError},
        user::UserRepo,
    },
};

use super::DynamoUserRepo;

mod access;
mod audit;
mod candidates;
mod memberships;
mod plans;
pub(in crate::dynamodb) mod records;
mod store;
mod trips;

#[async_trait]
impl TripRepo for DynamoUserRepo {
    async fn create_trip(&self, trip: Trip) -> Result<Trip, TripRepoError> {
        trips::create_trip(self, trip).await
    }

    async fn list_trips(&self, actor: &UserId) -> Result<Vec<TripSummary>, TripRepoError> {
        trips::list_trips(self, actor).await
    }

    async fn get_trip(&self, trip_id: &str, actor: &UserId) -> Result<Trip, TripRepoError> {
        trips::get_trip(self, trip_id, actor).await
    }

    async fn set_trip_status(
        &self,
        trip_id: &str,
        actor: &UserId,
        status: TripStatus,
        changed_at: &str,
        change_id: &str,
    ) -> Result<Trip, TripRepoError> {
        trips::set_trip_status(self, trip_id, actor, status, changed_at, change_id).await
    }

    async fn get_members(
        &self,
        trip_id: &str,
        actor: &UserId,
        users: &dyn UserRepo,
    ) -> Result<Vec<User>, TripRepoError> {
        memberships::get_members(self, trip_id, actor, users).await
    }

    async fn remove_member(
        &self,
        trip_id: &str,
        actor: &UserId,
        target: &UserId,
    ) -> Result<(), TripRepoError> {
        memberships::remove_member(self, trip_id, actor, target).await
    }

    async fn create_invite(
        &self,
        trip_id: &str,
        actor: &UserId,
        invite: Invite,
    ) -> Result<Invite, TripRepoError> {
        memberships::create_invite(self, trip_id, actor, invite).await
    }

    async fn accept_pending_invites(
        &self,
        user: &User,
        joined_at: &str,
    ) -> Result<(), TripRepoError> {
        memberships::accept_pending_invites(self, user, joined_at).await
    }

    async fn search_saved_places(
        &self,
        trip_id: &str,
        actor: &UserId,
        query: &str,
    ) -> Result<Vec<Place>, TripRepoError> {
        candidates::search_saved_places(self, trip_id, actor, query).await
    }

    async fn find_place(
        &self,
        trip_id: &str,
        actor: &UserId,
        place_id: &str,
    ) -> Result<Option<Place>, TripRepoError> {
        candidates::find_place(self, trip_id, actor, place_id).await
    }

    async fn list_candidates(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<CandidateWithPlace>, TripRepoError> {
        candidates::list_candidates(self, trip_id, actor).await
    }

    async fn add_candidate(
        &self,
        trip_id: &str,
        actor: &UserId,
        candidate: Candidate,
        place: Place,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        candidates::add_candidate(self, trip_id, actor, candidate, place).await
    }

    async fn update_candidate(
        &self,
        trip_id: &str,
        actor: &UserId,
        candidate_id: &str,
        update: CandidateUpdate,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        candidates::update_candidate(self, trip_id, actor, candidate_id, update).await
    }

    async fn set_candidate_status(
        &self,
        trip_id: &str,
        actor: &UserId,
        candidate_id: &str,
        status: CandidateDisposition,
        changed_at: &str,
        change_id: &str,
    ) -> Result<CandidateWithPlace, TripRepoError> {
        candidates::set_candidate_status(
            self,
            trip_id,
            actor,
            candidate_id,
            status,
            changed_at,
            change_id,
        )
        .await
    }

    async fn get_current_plan(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<PlanDetail, TripRepoError> {
        plans::get_current_plan(self, trip_id, actor).await
    }

    async fn initialize_plan(
        &self,
        trip_id: &str,
        actor: &UserId,
        anchor_place_id: &str,
        plan: Plan,
        days: Vec<Day>,
    ) -> Result<PlanDetail, TripRepoError> {
        plans::initialize_plan(self, trip_id, actor, anchor_place_id, plan, days).await
    }

    async fn list_plan_versions(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Plan>, TripRepoError> {
        plans::list_plan_versions(self, trip_id, actor).await
    }

    async fn update_day(
        &self,
        trip_id: &str,
        actor: &UserId,
        day_id: &str,
        patch: DayPatch,
        changed_at: &str,
        change_id: &str,
    ) -> Result<Day, TripRepoError> {
        plans::update_day(self, trip_id, actor, day_id, patch, changed_at, change_id).await
    }

    async fn update_stop(
        &self,
        trip_id: &str,
        actor: &UserId,
        stop_id: &str,
        patch: StopPatch,
        changed_at: &str,
        change_id: &str,
    ) -> Result<Stop, TripRepoError> {
        plans::update_stop(self, trip_id, actor, stop_id, patch, changed_at, change_id).await
    }
}

#[cfg(test)]
mod tests;
