//! SQLite structural-proposal repository.

pub(in crate::sqlite) mod records;

use async_trait::async_trait;
use itinera_core::{
    domain::proposal::Proposal,
    ports::{
        authorization::TripAuthorizationContext,
        proposal::{ProposalApplicationIds, ProposalRepo, ProposalRepoError},
    },
};

use super::SqliteDb;

#[derive(Clone)]
pub struct SqliteProposalRepo {
    db: SqliteDb,
}

impl SqliteProposalRepo {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &SqliteDb {
        &self.db
    }
}

#[async_trait]
impl ProposalRepo for SqliteProposalRepo {
    async fn list_proposals(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<Proposal>, ProposalRepoError> {
        operations::list_proposals(&self.db, trip_id, authorization).await
    }

    async fn create_proposal(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        proposal: Proposal,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, ProposalRepoError> {
        operations::create_proposal(&self.db, trip_id, authorization, proposal, application_ids)
            .await
    }

    async fn approve_proposal(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        proposal_id: &str,
        applied_at: &str,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, ProposalRepoError> {
        operations::approve_proposal(
            &self.db,
            trip_id,
            authorization,
            proposal_id,
            applied_at,
            application_ids,
        )
        .await
    }

    async fn reject_proposal(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        proposal_id: &str,
        reason: &str,
    ) -> Result<Proposal, ProposalRepoError> {
        operations::reject_proposal(&self.db, trip_id, authorization, proposal_id, reason).await
    }
}

pub(in crate::sqlite) mod operations;
pub(in crate::sqlite) mod publication;
