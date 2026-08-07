use async_trait::async_trait;

use crate::domain::{
    discussion::{Comment, DiscussionThread, ThreadAnchor},
    user::UserId,
};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscussionRepoError {
    #[error("discussion storage is unavailable")]
    Unavailable,
    #[error("discussion storage returned corrupt data")]
    CorruptData,
    #[error("resource not found")]
    NotFound,
    #[error("the actor does not have permission for this operation")]
    Forbidden,
    #[error("the operation conflicts with current state")]
    Conflict,
    #[error("the discussion exceeds the safe processing limit")]
    SafetyLimitExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThread {
    pub id: String,
    pub first_comment_id: String,
    pub anchor: ThreadAnchor,
    pub title: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewComment {
    pub id: String,
    pub body: String,
    pub created_at: String,
}

#[async_trait]
pub trait DiscussionRepo: Send + Sync {
    async fn list_threads(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<DiscussionThread>, DiscussionRepoError>;

    async fn create_thread(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewThread,
    ) -> Result<DiscussionThread, DiscussionRepoError>;

    async fn get_comments(
        &self,
        trip_id: &str,
        actor: &UserId,
        thread_id: &str,
    ) -> Result<Vec<Comment>, DiscussionRepoError>;

    async fn add_comment(
        &self,
        trip_id: &str,
        actor: &UserId,
        thread_id: &str,
        new: NewComment,
    ) -> Result<Comment, DiscussionRepoError>;

    async fn set_reaction(
        &self,
        trip_id: &str,
        actor: &UserId,
        thread_id: &str,
        comment_id: &str,
        emoji: &str,
        active: bool,
    ) -> Result<Comment, DiscussionRepoError>;
}
