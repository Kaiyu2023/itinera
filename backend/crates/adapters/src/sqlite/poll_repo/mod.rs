//! SQLite decision/plan-change poll repository.

mod records;

use async_trait::async_trait;
use itinera_core::{
    domain::{poll::Poll, proposal::Proposal},
    ports::{
        authorization::TripAuthorizationContext,
        clock::Clock,
        poll::{NewDecisionPoll, NewPlanChangePoll, PollRepo, PollRepoError},
        proposal::ProposalApplicationIds,
    },
};

use super::SqliteDb;

#[derive(Clone)]
pub struct SqlitePollRepo {
    db: SqliteDb,
}

impl SqlitePollRepo {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &SqliteDb {
        &self.db
    }
}

#[async_trait]
impl PollRepo for SqlitePollRepo {
    async fn list_polls(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<Poll>, PollRepoError> {
        operations::list_polls(&self.db, trip_id, authorization).await
    }

    async fn create_decision_poll(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        poll: NewDecisionPoll,
    ) -> Result<Poll, PollRepoError> {
        operations::create_decision_poll(&self.db, trip_id, authorization, poll).await
    }

    async fn create_proposal_poll(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        proposal: Proposal,
        poll: NewPlanChangePoll,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, PollRepoError> {
        operations::create_proposal_poll(
            &self.db,
            trip_id,
            authorization,
            proposal,
            poll,
            application_ids,
        )
        .await
    }

    async fn route_proposal_to_poll(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        proposal_id: &str,
        poll: NewPlanChangePoll,
        application_ids: ProposalApplicationIds,
    ) -> Result<Poll, PollRepoError> {
        operations::route_proposal_to_poll(
            &self.db,
            trip_id,
            authorization,
            proposal_id,
            poll,
            application_ids,
        )
        .await
    }

    async fn open_poll(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        poll_id: &str,
        clock: &dyn Clock,
    ) -> Result<Poll, PollRepoError> {
        operations::open_poll(&self.db, trip_id, authorization, poll_id, clock).await
    }

    async fn cast_vote(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        poll_id: &str,
        option_ids: &[String],
        clock: &dyn Clock,
    ) -> Result<Poll, PollRepoError> {
        operations::cast_vote(&self.db, trip_id, authorization, poll_id, option_ids, clock).await
    }

    async fn close_poll(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        poll_id: &str,
        decided_at: &str,
        application_ids: ProposalApplicationIds,
    ) -> Result<Poll, PollRepoError> {
        operations::close_poll(
            &self.db,
            trip_id,
            authorization,
            poll_id,
            decided_at,
            application_ids,
        )
        .await
    }
}

pub(in crate::sqlite) mod operations;
