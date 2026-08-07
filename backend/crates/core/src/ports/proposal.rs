use async_trait::async_trait;

use crate::domain::{proposal::Proposal, user::UserId};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProposalRepoError {
    #[error("proposal storage is unavailable")]
    Unavailable,
    #[error("proposal storage returned corrupt data")]
    CorruptData,
    #[error("resource not found")]
    NotFound,
    #[error("the actor does not have permission for this operation")]
    Forbidden,
    #[error("the operation conflicts with current state")]
    Conflict,
    #[error("the structural change is invalid for the current plan")]
    InvalidChange,
    #[error("the proposal exceeds the repository's safe transaction limit")]
    SafetyLimitExceeded,
}

/// IDs reserved by the application layer for one potential plan publication.
///
/// The repository consumes only as many as the ChangeSet needs. Keeping ID
/// generation outside persistence makes retries deterministic without exposing
/// provider-specific revisions or keys through the core port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposalApplicationIds {
    pub plan_id: String,
    pub entity_ids: Vec<String>,
}

#[async_trait]
pub trait ProposalRepo: Send + Sync {
    async fn list_proposals(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Proposal>, ProposalRepoError>;

    async fn create_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal: Proposal,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, ProposalRepoError>;

    async fn approve_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal_id: &str,
        applied_at: &str,
        application_ids: ProposalApplicationIds,
    ) -> Result<Proposal, ProposalRepoError>;

    async fn reject_proposal(
        &self,
        trip_id: &str,
        actor: &UserId,
        proposal_id: &str,
        reason: &str,
    ) -> Result<Proposal, ProposalRepoError>;
}
