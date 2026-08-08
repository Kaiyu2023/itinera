use async_trait::async_trait;

use crate::domain::{
    poll::{Poll, PollOption},
    proposal::Proposal,
};

use super::{authorization::TripAuthorizationContext, proposal::ProposalApplicationIds};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PollRepoError {
    #[error("poll storage is unavailable")]
    Unavailable,
    #[error("poll storage returned corrupt data")]
    CorruptData,
    #[error("resource not found")]
    NotFound,
    #[error("the actor does not have permission for this operation")]
    Forbidden,
    #[error("the operation conflicts with current state")]
    Conflict,
    #[error("the ballot is invalid for this poll")]
    InvalidVote,
    #[error("the poll exceeds the safe processing limit")]
    SafetyLimitExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDecisionPoll {
    pub id: String,
    pub title: String,
    pub description: String,
    pub options: Vec<PollOption>,
    pub closes_at: String,
    pub allow_multi: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlanChangePoll {
    pub poll_id: String,
    pub adopt_option_id: String,
    pub keep_option_id: String,
    pub created_at: String,
    pub closes_at: String,
}

#[async_trait]
pub trait PollRepo: Send + Sync {
    async fn list_polls(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<Poll>, PollRepoError>;

    async fn create_decision_poll(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        poll: NewDecisionPoll,
    ) -> Result<Poll, PollRepoError>;

    async fn create_proposal_poll(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        proposal: Proposal,
        poll: NewPlanChangePoll,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, PollRepoError>;

    async fn route_proposal_to_poll(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        proposal_id: &str,
        poll: NewPlanChangePoll,
        application_ids: ProposalApplicationIds,
    ) -> Result<Poll, PollRepoError>;

    async fn open_poll(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        poll_id: &str,
        opened_at: &str,
    ) -> Result<Poll, PollRepoError>;

    async fn cast_vote(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        poll_id: &str,
        option_ids: &[String],
        voted_at: &str,
    ) -> Result<Poll, PollRepoError>;

    async fn close_poll(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        poll_id: &str,
        decided_at: &str,
        application_ids: ProposalApplicationIds,
    ) -> Result<Poll, PollRepoError>;
}
