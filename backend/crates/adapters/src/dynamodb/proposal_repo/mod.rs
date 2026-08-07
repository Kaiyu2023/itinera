//! DynamoDB structural-proposal repository.
//!
//! Governance persistence is deliberately separate from `TripRepo`. This
//! capability owns proposal lifecycle validation, direct membership checks,
//! immutable plan cloning, and the complete compare-and-swap transaction that
//! publishes a proposal.

use async_trait::async_trait;
use itinera_core::{
    domain::{proposal::Proposal, user::UserId},
    ports::proposal::{ProposalApplicationIds, ProposalRepo, ProposalRepoError},
    services::proposals::ChangeApplicationError,
};

use super::DynamoUserRepo;

pub(in crate::dynamodb) mod access;
pub(in crate::dynamodb) mod application;
pub(in crate::dynamodb) mod operations;
pub(in crate::dynamodb) mod records;

#[async_trait]
impl ProposalRepo for DynamoUserRepo {
    async fn list_proposals(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Proposal>, ProposalRepoError> {
        operations::list_proposals(self, trip_id, actor).await
    }

    async fn create_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal: Proposal,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, ProposalRepoError> {
        operations::create_proposal(self, trip_id, actor, proposal, application_ids).await
    }

    async fn approve_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal_id: &str,
        applied_at: &str,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, ProposalRepoError> {
        operations::approve_proposal(
            self,
            trip_id,
            actor,
            proposal_id,
            applied_at,
            application_ids,
        )
        .await
    }

    async fn reject_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal_id: &str,
        reason: &str,
    ) -> Result<Proposal, ProposalRepoError> {
        operations::reject_proposal(self, trip_id, actor, proposal_id, reason).await
    }
}

fn record_error(_: itinera_core::ports::trip::TripRepoError) -> ProposalRepoError {
    ProposalRepoError::CorruptData
}

fn application_error(error: ChangeApplicationError) -> ProposalRepoError {
    match error {
        ChangeApplicationError::CorruptData => ProposalRepoError::CorruptData,
        ChangeApplicationError::NotFound => ProposalRepoError::NotFound,
        ChangeApplicationError::InvalidChange => ProposalRepoError::InvalidChange,
    }
}

#[cfg(test)]
mod tests;
