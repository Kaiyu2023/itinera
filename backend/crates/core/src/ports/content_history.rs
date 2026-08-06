use async_trait::async_trait;

use crate::domain::{content_history::Edit, user::UserId};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContentHistoryRepoError {
    #[error("content history storage is unavailable")]
    Unavailable,
    #[error("content history storage returned corrupt data")]
    CorruptData,
    #[error("resource not found")]
    NotFound,
    #[error("the actor does not have permission for this operation")]
    Forbidden,
    #[error("the operation conflicts with current state")]
    Conflict,
    #[error("this edit target cannot be reverted safely")]
    Unsupported,
    #[error("the history operation exceeds its safe processing limit")]
    SafetyLimitExceeded,
}

#[async_trait]
pub trait ContentHistoryRepo: Send + Sync {
    async fn list_history(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Edit>, ContentHistoryRepoError>;

    async fn revert_edit(
        &self,
        trip_id: &str,
        actor: &UserId,
        edit_id: &str,
        reverted_at: &str,
        compensating_edit_id: &str,
    ) -> Result<(), ContentHistoryRepoError>;
}
