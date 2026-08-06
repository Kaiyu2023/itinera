//! DynamoDB poll repository.
//!
//! Poll metadata, one ballot per voter, proposal routing, and plan-changing
//! close transactions live in this capability rather than expanding
//! `TripRepo` or `ProposalRepo` into a governance monolith.

use async_trait::async_trait;
use itinera_core::{
    domain::{poll::Poll, proposal::Proposal, user::UserId},
    ports::{
        poll::{NewDecisionPoll, NewPlanChangePoll, PollRepo, PollRepoError},
        proposal::ProposalApplicationIds,
    },
};

use super::DynamoUserRepo;

mod access;
mod operations;
pub(in crate::dynamodb) mod records;
mod transitions;

#[async_trait]
impl PollRepo for DynamoUserRepo {
    async fn list_polls(&self, trip_id: &str, actor: &UserId) -> Result<Vec<Poll>, PollRepoError> {
        operations::list_polls(self, trip_id, actor).await
    }

    async fn create_decision_poll(
        &self,
        trip_id: &str,
        actor: &UserId,
        poll: NewDecisionPoll,
    ) -> Result<Poll, PollRepoError> {
        operations::create_decision_poll(self, trip_id, actor, poll).await
    }

    async fn create_proposal_poll(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal: Proposal,
        poll: NewPlanChangePoll,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, PollRepoError> {
        transitions::create_proposal_poll(self, trip_id, actor, proposal, poll, application_ids)
            .await
    }

    async fn route_proposal_to_poll(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal_id: &str,
        poll: NewPlanChangePoll,
        application_ids: ProposalApplicationIds,
    ) -> Result<Poll, PollRepoError> {
        transitions::route_proposal_to_poll(
            self,
            trip_id,
            actor,
            proposal_id,
            poll,
            application_ids,
        )
        .await
    }

    async fn open_poll(
        &self,
        trip_id: &str,
        actor: &UserId,
        poll_id: &str,
        opened_at: &str,
    ) -> Result<Poll, PollRepoError> {
        operations::open_poll(self, trip_id, actor, poll_id, opened_at).await
    }

    async fn cast_vote(
        &self,
        trip_id: &str,
        actor: &UserId,
        poll_id: &str,
        option_ids: &[String],
        voted_at: &str,
    ) -> Result<Poll, PollRepoError> {
        operations::cast_vote(self, trip_id, actor, poll_id, option_ids, voted_at).await
    }

    async fn close_poll(
        &self,
        trip_id: &str,
        actor: &UserId,
        poll_id: &str,
        decided_at: &str,
        application_ids: ProposalApplicationIds,
    ) -> Result<Poll, PollRepoError> {
        transitions::close_poll(self, trip_id, actor, poll_id, decided_at, application_ids).await
    }
}

fn record_error(_: itinera_core::ports::trip::TripRepoError) -> PollRepoError {
    PollRepoError::CorruptData
}

fn proposal_error(error: itinera_core::ports::proposal::ProposalRepoError) -> PollRepoError {
    use itinera_core::ports::proposal::ProposalRepoError;

    match error {
        ProposalRepoError::Unavailable => PollRepoError::Unavailable,
        ProposalRepoError::CorruptData => PollRepoError::CorruptData,
        ProposalRepoError::NotFound => PollRepoError::NotFound,
        ProposalRepoError::Forbidden => PollRepoError::Forbidden,
        ProposalRepoError::Conflict | ProposalRepoError::InvalidChange => PollRepoError::Conflict,
        ProposalRepoError::SafetyLimitExceeded => PollRepoError::SafetyLimitExceeded,
    }
}

#[cfg(test)]
mod tests;
