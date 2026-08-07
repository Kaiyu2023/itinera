use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;
use chrono::DateTime;
use itinera_core::{
    domain::notice::Notice,
    ports::notice::NoticeRepoError,
    services::notices::{
        MAX_NOTICE_RESPONSE_BYTES, checklist_toggle_request_hash, validate_stored_notice,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dynamodb::{
    SK,
    primitives::encoded_item_bytes,
    trip_repo::records::{DATA, decode_record, encode_record, string, trip_pk},
};

use super::record_error;

pub(in crate::dynamodb) const NOTICE_META_ENTITY: &str = "NOTICE_META";
pub(in crate::dynamodb) const NOTICE_ENTITY: &str = "NOTICE";
pub(in crate::dynamodb) const NOTICE_META_SK: &str = "NOTICE#META";
pub(in crate::dynamodb) const NOTICE_PREFIX: &str = "NOTICE_ITEM#";
pub(super) const NOTICE_OPERATION_ENTITY: &str = "NOTICE_OPERATION";
pub(super) const NOTICE_OPERATION_META_ENTITY: &str = "NOTICE_OPERATION_META";
pub(super) const NOTICE_OPERATION_META_SK: &str = "META";
pub(super) const NOTICE_OPERATION_PREFIX: &str = "OP#";
pub(super) const MAX_NOTICE_OPERATIONS_PER_ACTOR: usize = 32;
pub(in crate::dynamodb) const MAX_NOTICE_ITEM_BYTES: usize = 350 * 1_024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::dynamodb) struct NoticeMetaRecord {
    pub(in crate::dynamodb) trip_id: String,
    pub(in crate::dynamodb) notice_count: u32,
    pub(in crate::dynamodb) notice_bytes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NoticeOperationMetaRecord {
    pub(super) trip_id: String,
    pub(super) actor_id: String,
    pub(super) operation_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "snake_case",
    tag = "kind",
    content = "value",
    deny_unknown_fields
)]
pub(super) enum NoticeOperationResult {
    Created { notice_id: String },
    ChecklistToggled { notice_id: String, item_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NoticeOperationRecord {
    pub(super) trip_id: String,
    pub(super) key_hash: String,
    pub(super) actor_id: String,
    pub(super) request_hash: String,
    pub(super) result: NoticeOperationResult,
    pub(super) recorded_at: String,
    pub(super) expires_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredNotice {
    notice: Notice,
    created_at: String,
}

#[derive(Debug, Clone)]
pub(in crate::dynamodb) struct Loaded<T> {
    pub(in crate::dynamodb) value: T,
    pub(in crate::dynamodb) revision: u64,
    pub(in crate::dynamodb) raw_data: String,
}

#[derive(Debug, Clone)]
pub(in crate::dynamodb) struct LoadedNotice {
    pub(in crate::dynamodb) notice: Notice,
    pub(in crate::dynamodb) created_at: String,
    pub(in crate::dynamodb) revision: u64,
    pub(in crate::dynamodb) raw_data: String,
    pub(in crate::dynamodb) encoded_bytes: usize,
}

pub(in crate::dynamodb) fn notice_sk(notice_id: &str) -> String {
    format!("{NOTICE_PREFIX}{notice_id}")
}

pub(super) fn notice_operation_key_hash(idempotency_key: &str) -> String {
    let digest = Sha256::digest(idempotency_key.as_bytes());
    format!("{digest:x}")
}

pub(super) fn notice_operation_partition_key(trip_id: &str, actor_id: &str) -> String {
    let actor_hash = Sha256::digest(actor_id.as_bytes());
    format!("{}#NOTICE_ACTOR#{actor_hash:x}", trip_pk(trip_id))
}

pub(super) fn notice_operation_sk(key_hash: &str) -> String {
    format!("{NOTICE_OPERATION_PREFIX}{key_hash}")
}

pub(super) fn encode_notice_operation_meta(
    meta: &NoticeOperationMetaRecord,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, NoticeRepoError> {
    if !valid_id(&meta.trip_id)
        || !valid_id(&meta.actor_id)
        || meta.operation_count == 0
        || meta.operation_count as usize > MAX_NOTICE_OPERATIONS_PER_ACTOR
        || revision == 0
    {
        return Err(NoticeRepoError::CorruptData);
    }
    bounded_item(
        encode_record(
            notice_operation_partition_key(&meta.trip_id, &meta.actor_id),
            NOTICE_OPERATION_META_SK.to_string(),
            NOTICE_OPERATION_META_ENTITY,
            meta,
            revision,
        )
        .map_err(record_error)?,
    )
}

pub(super) fn decode_notice_operation_meta(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
    expected_actor_id: &str,
) -> Result<Loaded<NoticeOperationMetaRecord>, NoticeRepoError> {
    let partition_key = notice_operation_partition_key(expected_trip_id, expected_actor_id);
    let stored = decode_record::<NoticeOperationMetaRecord>(
        item,
        &partition_key,
        NOTICE_OPERATION_META_SK,
        NOTICE_OPERATION_META_ENTITY,
    )
    .map_err(record_error)?;
    if stored.revision == 0
        || stored.value.trip_id != expected_trip_id
        || stored.value.actor_id != expected_actor_id
        || stored.value.operation_count == 0
        || stored.value.operation_count as usize > MAX_NOTICE_OPERATIONS_PER_ACTOR
    {
        return Err(NoticeRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
        raw_data: string(item, DATA).map_err(record_error)?,
    })
}

pub(in crate::dynamodb) fn encode_notice_meta(
    meta: &NoticeMetaRecord,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, NoticeRepoError> {
    if meta.notice_count == 0
        || meta.notice_count > 1_000
        || meta.notice_bytes == 0
        || meta.notice_bytes as usize > MAX_NOTICE_RESPONSE_BYTES
        || revision == 0
    {
        return Err(NoticeRepoError::CorruptData);
    }
    encode_record(
        trip_pk(&meta.trip_id),
        NOTICE_META_SK.to_string(),
        NOTICE_META_ENTITY,
        meta,
        revision,
    )
    .map_err(record_error)
}

pub(in crate::dynamodb) fn decode_notice_meta(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<Loaded<NoticeMetaRecord>, NoticeRepoError> {
    let pk = trip_pk(expected_trip_id);
    let stored = decode_record::<NoticeMetaRecord>(item, &pk, NOTICE_META_SK, NOTICE_META_ENTITY)
        .map_err(record_error)?;
    if stored.revision == 0
        || stored.value.trip_id != expected_trip_id
        || stored.value.notice_count == 0
        || stored.value.notice_count > 1_000
        || stored.value.notice_bytes == 0
        || stored.value.notice_bytes as usize > MAX_NOTICE_RESPONSE_BYTES
    {
        return Err(NoticeRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
        raw_data: string(item, DATA).map_err(record_error)?,
    })
}

pub(in crate::dynamodb) fn encode_notice(
    notice: &Notice,
    created_at: &str,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, NoticeRepoError> {
    validate_stored_notice(&notice.trip_id, notice).map_err(|_| NoticeRepoError::CorruptData)?;
    utc(created_at)?;
    if revision == 0 {
        return Err(NoticeRepoError::CorruptData);
    }
    bounded_item(
        encode_record(
            trip_pk(&notice.trip_id),
            notice_sk(&notice.id),
            NOTICE_ENTITY,
            &StoredNotice {
                notice: notice.clone(),
                created_at: created_at.to_string(),
            },
            revision,
        )
        .map_err(record_error)?,
    )
}

pub(in crate::dynamodb) fn decode_notice(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<LoadedNotice, NoticeRepoError> {
    let encoded_bytes = encoded_item_bytes(item).ok_or(NoticeRepoError::SafetyLimitExceeded)?;
    if encoded_bytes > MAX_NOTICE_ITEM_BYTES {
        return Err(NoticeRepoError::SafetyLimitExceeded);
    }
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored =
        decode_record::<StoredNotice>(item, &pk, &sk, NOTICE_ENTITY).map_err(record_error)?;
    if stored.revision == 0
        || sk != notice_sk(&stored.value.notice.id)
        || validate_stored_notice(expected_trip_id, &stored.value.notice).is_err()
        || utc(&stored.value.created_at).is_err()
    {
        return Err(NoticeRepoError::CorruptData);
    }
    Ok(LoadedNotice {
        notice: stored.value.notice,
        created_at: stored.value.created_at,
        revision: stored.revision,
        raw_data: string(item, DATA).map_err(record_error)?,
        encoded_bytes,
    })
}

pub(super) fn encode_notice_operation(
    operation: &NoticeOperationRecord,
) -> Result<HashMap<String, AttributeValue>, NoticeRepoError> {
    validate_notice_operation(operation)?;
    bounded_item(
        encode_record(
            notice_operation_partition_key(&operation.trip_id, &operation.actor_id),
            notice_operation_sk(&operation.key_hash),
            NOTICE_OPERATION_ENTITY,
            operation,
            1,
        )
        .map_err(record_error)?,
    )
}

pub(super) fn decode_notice_operation(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
    expected_actor_id: &str,
) -> Result<Loaded<NoticeOperationRecord>, NoticeRepoError> {
    if encoded_item_bytes(item).is_none_or(|bytes| bytes > MAX_NOTICE_ITEM_BYTES) {
        return Err(NoticeRepoError::SafetyLimitExceeded);
    }
    let pk = notice_operation_partition_key(expected_trip_id, expected_actor_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored = decode_record::<NoticeOperationRecord>(item, &pk, &sk, NOTICE_OPERATION_ENTITY)
        .map_err(record_error)?;
    if stored.revision != 1
        || stored.value.trip_id != expected_trip_id
        || stored.value.actor_id != expected_actor_id
        || sk != notice_operation_sk(&stored.value.key_hash)
        || validate_notice_operation(&stored.value).is_err()
    {
        return Err(NoticeRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
        raw_data: string(item, DATA).map_err(record_error)?,
    })
}

fn validate_notice_operation(operation: &NoticeOperationRecord) -> Result<(), NoticeRepoError> {
    if !valid_id(&operation.trip_id)
        || !valid_id(&operation.actor_id)
        || !valid_sha256(&operation.key_hash)
        || !valid_sha256(&operation.request_hash)
        || utc(&operation.recorded_at).is_err()
        || utc(&operation.expires_at).is_err()
    {
        return Err(NoticeRepoError::CorruptData);
    }
    let recorded_at = utc(&operation.recorded_at)?;
    let expires_at = utc(&operation.expires_at)?;
    if recorded_at
        .checked_add_signed(chrono::Duration::hours(24))
        .is_none_or(|expected| expires_at != expected)
    {
        return Err(NoticeRepoError::CorruptData);
    }
    match &operation.result {
        NoticeOperationResult::Created { notice_id } => {
            if !valid_id(notice_id) {
                return Err(NoticeRepoError::CorruptData);
            }
        }
        NoticeOperationResult::ChecklistToggled { notice_id, item_id } => {
            if !valid_id(notice_id)
                || !valid_id(item_id)
                || operation.request_hash
                    != checklist_toggle_request_hash(notice_id, item_id)
                        .map_err(|_| NoticeRepoError::CorruptData)?
            {
                return Err(NoticeRepoError::CorruptData);
            }
        }
    }
    Ok(())
}

fn bounded_item(
    item: HashMap<String, AttributeValue>,
) -> Result<HashMap<String, AttributeValue>, NoticeRepoError> {
    if encoded_item_bytes(&item).is_none_or(|bytes| bytes > MAX_NOTICE_ITEM_BYTES) {
        Err(NoticeRepoError::SafetyLimitExceeded)
    } else {
        Ok(item)
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && value.chars().count() <= 200
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn utc(value: &str) -> Result<DateTime<chrono::FixedOffset>, NoticeRepoError> {
    let value = DateTime::parse_from_rfc3339(value).map_err(|_| NoticeRepoError::CorruptData)?;
    if value.offset().local_minus_utc() != 0 {
        return Err(NoticeRepoError::CorruptData);
    }
    Ok(value)
}
