//! DynamoDB discussion repository.
//!
//! Thread metadata, unique anchor claims, comments, and reactions live in this
//! capability rather than expanding `TripRepo` into another monolith.

use async_trait::async_trait;
use itinera_core::{
    domain::{
        discussion::{Comment, DiscussionThread},
        user::UserId,
    },
    ports::discussion::{DiscussionRepo, DiscussionRepoError, NewComment, NewThread},
};

use super::DynamoUserRepo;

mod access;
mod operations;
mod records;

#[cfg(test)]
mod tests;

#[async_trait]
impl DiscussionRepo for DynamoUserRepo {
    async fn list_threads(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<DiscussionThread>, DiscussionRepoError> {
        operations::list_threads(self, trip_id, actor).await
    }

    async fn create_thread(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewThread,
    ) -> Result<DiscussionThread, DiscussionRepoError> {
        operations::create_thread(self, trip_id, actor, new).await
    }

    async fn get_comments(
        &self,
        trip_id: &str,
        actor: &UserId,
        thread_id: &str,
    ) -> Result<Vec<Comment>, DiscussionRepoError> {
        operations::get_comments(self, trip_id, actor, thread_id).await
    }

    async fn add_comment(
        &self,
        trip_id: &str,
        actor: &UserId,
        thread_id: &str,
        new: NewComment,
    ) -> Result<Comment, DiscussionRepoError> {
        operations::add_comment(self, trip_id, actor, thread_id, new).await
    }

    async fn set_reaction(
        &self,
        trip_id: &str,
        actor: &UserId,
        thread_id: &str,
        comment_id: &str,
        emoji: &str,
        active: bool,
    ) -> Result<Comment, DiscussionRepoError> {
        operations::set_reaction(self, trip_id, actor, thread_id, comment_id, emoji, active).await
    }
}

fn record_error(_: itinera_core::ports::trip::TripRepoError) -> DiscussionRepoError {
    DiscussionRepoError::CorruptData
}

fn poll_error(error: itinera_core::ports::poll::PollRepoError) -> DiscussionRepoError {
    use itinera_core::ports::poll::PollRepoError;

    match error {
        PollRepoError::Unavailable => DiscussionRepoError::Unavailable,
        PollRepoError::CorruptData => DiscussionRepoError::CorruptData,
        PollRepoError::NotFound => DiscussionRepoError::NotFound,
        PollRepoError::Forbidden => DiscussionRepoError::Forbidden,
        PollRepoError::Conflict | PollRepoError::InvalidVote => DiscussionRepoError::Conflict,
        PollRepoError::SafetyLimitExceeded => DiscussionRepoError::SafetyLimitExceeded,
    }
}
