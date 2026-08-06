//! DynamoDB content-history repository.
//!
//! Audit reads and safe revert are a separate repository capability. They
//! share the one-table record codec with trip persistence, but own their
//! authorization reads, audit validation, allowlist, and complete revert
//! transaction here.

use async_trait::async_trait;
use itinera_core::{
    domain::{content_history::Edit, user::UserId},
    ports::{
        content_history::{ContentHistoryRepo, ContentHistoryRepoError},
        trip::TripRepoError,
    },
};

use super::DynamoUserRepo;

mod access;
mod audit;
mod revert;

#[async_trait]
impl ContentHistoryRepo for DynamoUserRepo {
    async fn list_history(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Edit>, ContentHistoryRepoError> {
        audit::list_history(self, trip_id, actor).await
    }

    async fn revert_edit(
        &self,
        trip_id: &str,
        actor: &UserId,
        edit_id: &str,
        reverted_at: &str,
        compensating_edit_id: &str,
    ) -> Result<(), ContentHistoryRepoError> {
        revert::revert_edit(
            self,
            trip_id,
            actor,
            edit_id,
            reverted_at,
            compensating_edit_id,
        )
        .await
    }
}

fn record_error(error: TripRepoError) -> ContentHistoryRepoError {
    match error {
        TripRepoError::Unavailable => ContentHistoryRepoError::Unavailable,
        _ => ContentHistoryRepoError::CorruptData,
    }
}

#[cfg(test)]
mod tests;
