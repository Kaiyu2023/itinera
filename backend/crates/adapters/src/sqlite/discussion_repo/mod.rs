//! Capability-scoped SQLite discussion repository.

use async_trait::async_trait;
use itinera_core::{
    domain::discussion::{Comment, DiscussionThread},
    ports::{
        authorization::TripAuthorizationContext,
        discussion::{DiscussionRepo, DiscussionRepoError, NewComment, NewThread},
    },
};

use super::{SqliteDb, codec::validate_id};

pub(in crate::sqlite) mod operations;
mod records;

#[derive(Clone)]
pub struct SqliteDiscussionRepo {
    db: SqliteDb,
}

impl SqliteDiscussionRepo {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &SqliteDb {
        &self.db
    }
}

#[async_trait]
impl DiscussionRepo for SqliteDiscussionRepo {
    async fn list_threads(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<DiscussionThread>, DiscussionRepoError> {
        validate_requested_id(trip_id)?;
        operations::list_threads(&self.db, trip_id, authorization).await
    }

    async fn create_thread(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        new: NewThread,
    ) -> Result<DiscussionThread, DiscussionRepoError> {
        validate_requested_id(trip_id)?;
        operations::create_thread(&self.db, trip_id, authorization, new).await
    }

    async fn get_comments(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        thread_id: &str,
    ) -> Result<Vec<Comment>, DiscussionRepoError> {
        validate_requested_id(trip_id)?;
        validate_requested_id(thread_id)?;
        operations::get_comments(&self.db, trip_id, authorization, thread_id).await
    }

    async fn add_comment(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        thread_id: &str,
        new: NewComment,
    ) -> Result<Comment, DiscussionRepoError> {
        validate_requested_id(trip_id)?;
        validate_requested_id(thread_id)?;
        operations::add_comment(&self.db, trip_id, authorization, thread_id, new).await
    }

    async fn set_reaction(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        thread_id: &str,
        comment_id: &str,
        emoji: &str,
        active: bool,
    ) -> Result<Comment, DiscussionRepoError> {
        validate_requested_id(trip_id)?;
        validate_requested_id(thread_id)?;
        validate_requested_id(comment_id)?;
        operations::set_reaction(
            &self.db,
            trip_id,
            authorization,
            thread_id,
            comment_id,
            emoji,
            active,
        )
        .await
    }
}

fn validate_requested_id(value: &str) -> Result<(), DiscussionRepoError> {
    validate_id(value).map_err(|_| DiscussionRepoError::NotFound)
}
