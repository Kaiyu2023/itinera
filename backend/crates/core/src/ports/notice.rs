use async_trait::async_trait;

use crate::domain::{notice::Notice, user::UserId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotice {
    pub notice: Notice,
    pub created_at: String,
    pub idempotency_key: String,
    pub request_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChecklistToggle {
    pub idempotency_key: String,
    pub request_hash: String,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoticePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub pinned: Option<bool>,
    pub source_url: Option<Option<String>>,
    pub status: Option<crate::domain::notice::NoticeStatus>,
    pub audience: Option<Option<Vec<String>>>,
}

impl NoticePatch {
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.body.is_none()
            && self.pinned.is_none()
            && self.source_url.is_none()
            && self.status.is_none()
            && self.audience.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeUpdate {
    pub patch: NoticePatch,
    pub changed_at: String,
    pub change_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NoticeRepoError {
    #[error("notice storage is unavailable")]
    Unavailable,
    #[error("notice storage contains corrupt data")]
    CorruptData,
    #[error("notice resource not found")]
    NotFound,
    #[error("notice operation is forbidden")]
    Forbidden,
    #[error("notice operation conflicts with current state")]
    Conflict,
    #[error("notice safety limit exceeded")]
    SafetyLimitExceeded,
}

#[async_trait]
pub trait NoticeRepo: Send + Sync {
    async fn list_notices(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<Vec<Notice>, NoticeRepoError>;

    async fn create_notice(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewNotice,
    ) -> Result<Notice, NoticeRepoError>;

    async fn replay_notice_creation(
        &self,
        trip_id: &str,
        actor: &UserId,
        idempotency_key: &str,
        request_hash: &str,
        now: &str,
    ) -> Result<Option<Notice>, NoticeRepoError>;

    async fn replay_checklist_toggle(
        &self,
        trip_id: &str,
        actor: &UserId,
        idempotency_key: &str,
        request_hash: &str,
        now: &str,
    ) -> Result<Option<Notice>, NoticeRepoError>;

    async fn update_notice(
        &self,
        trip_id: &str,
        actor: &UserId,
        notice_id: &str,
        update: NoticeUpdate,
    ) -> Result<Notice, NoticeRepoError>;

    async fn toggle_checklist_item(
        &self,
        trip_id: &str,
        actor: &UserId,
        notice_id: &str,
        item_id: &str,
        toggle: ChecklistToggle,
    ) -> Result<Notice, NoticeRepoError>;
}
