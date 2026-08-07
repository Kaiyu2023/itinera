//! DynamoDB notice repository.
//!
//! Notice metadata and checklist state form a bounded trip-scoped capability;
//! they deliberately do not expand the core trip repository.

use async_trait::async_trait;
use itinera_core::{
    domain::{notice::Notice, user::UserId},
    ports::notice::{ChecklistToggle, NewNotice, NoticeRepo, NoticeRepoError, NoticeUpdate},
};

use super::DynamoUserRepo;

pub(in crate::dynamodb) mod access;
pub(in crate::dynamodb) mod operations;
pub(in crate::dynamodb) mod records;

#[cfg(test)]
mod tests;

#[async_trait]
impl NoticeRepo for DynamoUserRepo {
    async fn list_notices(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Notice>, NoticeRepoError> {
        operations::list_notices(self, trip_id, actor).await
    }

    async fn create_notice(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewNotice,
    ) -> Result<Notice, NoticeRepoError> {
        operations::create_notice(self, trip_id, actor, new).await
    }

    async fn replay_notice_creation(
        &self,
        trip_id: &str,
        actor: &UserId,
        idempotency_key: &str,
        request_hash: &str,
        now: &str,
    ) -> Result<Option<Notice>, NoticeRepoError> {
        operations::replay_notice_creation(self, trip_id, actor, idempotency_key, request_hash, now)
            .await
    }

    async fn replay_checklist_toggle(
        &self,
        trip_id: &str,
        actor: &UserId,
        idempotency_key: &str,
        request_hash: &str,
        now: &str,
    ) -> Result<Option<Notice>, NoticeRepoError> {
        operations::replay_checklist_toggle(
            self,
            trip_id,
            actor,
            idempotency_key,
            request_hash,
            now,
        )
        .await
    }

    async fn update_notice(
        &self,
        trip_id: &str,
        actor: &UserId,
        notice_id: &str,
        update: NoticeUpdate,
    ) -> Result<Notice, NoticeRepoError> {
        operations::update_notice(self, trip_id, actor, notice_id, update).await
    }

    async fn toggle_checklist_item(
        &self,
        trip_id: &str,
        actor: &UserId,
        notice_id: &str,
        item_id: &str,
        toggle: ChecklistToggle,
    ) -> Result<Notice, NoticeRepoError> {
        operations::toggle_checklist_item(self, trip_id, actor, notice_id, item_id, toggle).await
    }
}

fn record_error(_: itinera_core::ports::trip::TripRepoError) -> NoticeRepoError {
    NoticeRepoError::CorruptData
}
