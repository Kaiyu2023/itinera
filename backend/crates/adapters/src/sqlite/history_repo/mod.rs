//! Capability-scoped SQLite content-history repository.

use async_trait::async_trait;
use itinera_core::{
    domain::content_history::Edit,
    ports::{
        authorization::TripAuthorizationContext,
        content_history::{ContentHistoryRepo, ContentHistoryRepoError},
        trip::TripRepoError,
    },
};

use super::{SqliteDb, codec::validate_id};
use crate::sqlite::trip_repo::access::{RequiredRole, authorize, validate_trip_aggregate};

pub(crate) mod audit;
mod records;
mod revert;

#[derive(Clone)]
pub struct SqliteContentHistoryRepo {
    db: SqliteDb,
}

impl SqliteContentHistoryRepo {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &SqliteDb {
        &self.db
    }
}

#[async_trait]
impl ContentHistoryRepo for SqliteContentHistoryRepo {
    async fn list_history(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
    ) -> Result<Vec<Edit>, ContentHistoryRepoError> {
        validate_requested_id(trip_id)?;
        let mut transaction = self.db.pool().begin().await.map_err(unavailable)?;
        authorize(
            &mut transaction,
            trip_id,
            authorization,
            RequiredRole::AnyMember,
        )
        .await
        .map_err(map_trip_error)?;
        validate_trip_aggregate(&mut transaction, trip_id)
            .await
            .map_err(map_trip_error)?;
        let edits = audit::history_values(&audit::load_history(&mut transaction, trip_id).await?)?;
        self.db.commit(transaction).await.map_err(unavailable)?;
        Ok(edits)
    }

    async fn revert_edit(
        &self,
        trip_id: &str,
        authorization: &TripAuthorizationContext,
        edit_id: &str,
        reverted_at: &str,
        compensating_edit_id: &str,
    ) -> Result<(), ContentHistoryRepoError> {
        validate_requested_id(trip_id)?;
        validate_requested_id(edit_id)?;
        revert::revert_edit(
            &self.db,
            trip_id,
            authorization,
            edit_id,
            reverted_at,
            compensating_edit_id,
        )
        .await
    }
}

pub(crate) fn map_trip_error(error: TripRepoError) -> ContentHistoryRepoError {
    match error {
        TripRepoError::Unavailable => ContentHistoryRepoError::Unavailable,
        TripRepoError::CorruptData => ContentHistoryRepoError::CorruptData,
        TripRepoError::NotFound => ContentHistoryRepoError::NotFound,
        TripRepoError::Forbidden => ContentHistoryRepoError::Forbidden,
        TripRepoError::Conflict | TripRepoError::DuplicateInvite => {
            ContentHistoryRepoError::Conflict
        }
    }
}

fn validate_requested_id(value: &str) -> Result<(), ContentHistoryRepoError> {
    validate_id(value).map_err(|_| ContentHistoryRepoError::NotFound)
}

fn unavailable<T>(_error: T) -> ContentHistoryRepoError {
    ContentHistoryRepoError::Unavailable
}
