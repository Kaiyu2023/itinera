//! Query-shaped codecs for immutable content-edit rows.

use itinera_core::{
    domain::content_history::{ChangeSource, Edit, EditEntity, EditStatus},
    ports::content_history::ContentHistoryRepoError,
    services::content_history::validate_stored_edit,
};
use serde_json::Value;
use sqlx::FromRow;

use crate::sqlite::codec::{checked_revision, validate_id};

pub(super) const MAX_HISTORY_RECORDS: usize = 1_000;
pub(super) const HISTORY_QUERY_LIMIT: i64 = 1_001;
pub(super) const MAX_HISTORY_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_STORED_VALUE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, FromRow)]
pub(super) struct EditRow {
    edit_trip_id: String,
    edit_id: String,
    edit_entity: String,
    edit_entity_id: String,
    edit_field: String,
    old_value_json: String,
    new_value_json: String,
    author_id: String,
    source_kind: String,
    source_service_id: Option<String>,
    source_service_name: Option<String>,
    edit_status: String,
    edit_created_at: String,
    reverted_by: Option<String>,
    reverted_at: Option<String>,
    revert_edit_id: Option<String>,
    reverts_edit_id: Option<String>,
    edit_revision: i64,
}

#[derive(Debug, Clone)]
pub(super) struct StoredEdit {
    pub(super) value: Edit,
    pub(super) revision: i64,
}

impl EditRow {
    pub(super) fn into_edit(
        self,
        expected_trip_id: &str,
    ) -> Result<StoredEdit, ContentHistoryRepoError> {
        let revision = checked_revision(self.edit_revision).map_err(corrupt)?;
        let source = match (
            self.source_kind.as_str(),
            self.source_service_id,
            self.source_service_name,
        ) {
            ("web", None, None) => ChangeSource::Web {},
            ("service", Some(service_identity_id), Some(service_identity_name)) => {
                ChangeSource::Service {
                    service_identity_id,
                    service_identity_name,
                }
            }
            _ => return Err(ContentHistoryRepoError::CorruptData),
        };
        let edit = Edit {
            id: self.edit_id,
            trip_id: self.edit_trip_id,
            entity: decode_entity(&self.edit_entity)?,
            entity_id: self.edit_entity_id,
            field: self.edit_field,
            old_value: decode_canonical_value(self.old_value_json)?,
            new_value: decode_canonical_value(self.new_value_json)?,
            author: self.author_id,
            source,
            status: decode_status(&self.edit_status)?,
            created_at: self.edit_created_at,
            reverted_by: self.reverted_by,
            reverted_at: self.reverted_at,
            revert_edit_id: self.revert_edit_id,
            reverts_edit_id: self.reverts_edit_id,
        };
        validate_stored_edit(expected_trip_id, &edit).map_err(corrupt)?;
        Ok(StoredEdit {
            value: edit,
            revision,
        })
    }
}

pub(super) fn encode_entity(entity: EditEntity) -> &'static str {
    match entity {
        EditEntity::Stop => "stop",
        EditEntity::Day => "day",
        EditEntity::Candidate => "candidate",
        EditEntity::Notice => "notice",
        EditEntity::Trip => "trip",
    }
}

pub(super) fn encode_status(status: EditStatus) -> Result<&'static str, ContentHistoryRepoError> {
    match status {
        EditStatus::Applied => Ok("applied"),
        EditStatus::Reverted => Ok("reverted"),
        EditStatus::PendingReview | EditStatus::Rejected => {
            Err(ContentHistoryRepoError::CorruptData)
        }
    }
}

pub(super) fn encode_value(value: &Value) -> Result<String, ContentHistoryRepoError> {
    let encoded = serde_json::to_string(value).map_err(corrupt)?;
    if encoded.len() > MAX_STORED_VALUE_BYTES {
        return Err(ContentHistoryRepoError::SafetyLimitExceeded);
    }
    Ok(encoded)
}

fn decode_entity(value: &str) -> Result<EditEntity, ContentHistoryRepoError> {
    match value {
        "stop" => Ok(EditEntity::Stop),
        "day" => Ok(EditEntity::Day),
        "candidate" => Ok(EditEntity::Candidate),
        "notice" => Ok(EditEntity::Notice),
        "trip" => Ok(EditEntity::Trip),
        _ => Err(ContentHistoryRepoError::CorruptData),
    }
}

fn decode_status(value: &str) -> Result<EditStatus, ContentHistoryRepoError> {
    match value {
        "applied" => Ok(EditStatus::Applied),
        "reverted" => Ok(EditStatus::Reverted),
        _ => Err(ContentHistoryRepoError::CorruptData),
    }
}

fn decode_canonical_value(value: String) -> Result<Value, ContentHistoryRepoError> {
    if value.len() > MAX_STORED_VALUE_BYTES {
        return Err(ContentHistoryRepoError::SafetyLimitExceeded);
    }
    let decoded = serde_json::from_str::<Value>(&value).map_err(corrupt)?;
    if serde_json::to_string(&decoded).map_err(corrupt)? != value {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    Ok(decoded)
}

pub(super) fn validate_actor_id(value: &str) -> Result<(), ContentHistoryRepoError> {
    validate_id(value).map_err(corrupt)
}

fn corrupt<T>(_error: T) -> ContentHistoryRepoError {
    ContentHistoryRepoError::CorruptData
}
