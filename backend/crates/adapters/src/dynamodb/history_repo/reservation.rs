//! Shared, bounded reservations for appending ordinary content-audit rows.

use std::collections::HashSet;

use aws_sdk_dynamodb::types::{AttributeValue, TransactWriteItem};
use itinera_core::{
    domain::content_history::EditStatus, ports::content_history::ContentHistoryRepoError,
};
use serde::Serialize;

use crate::dynamodb::{
    DynamoUserRepo,
    primitives::{encoded_item_bytes, put_action},
    trip_repo::records::{encode_record, trip_pk},
};

use super::{
    access::{MAX_HISTORY_BYTES, MAX_HISTORY_RECORDS},
    audit::{decode_audit_item, load_audit_records},
    record_error,
};

pub(super) const HISTORY_SLOT_ENTITY: &str = "CONTENT_HISTORY_SLOT";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HistorySlot {
    record_count: u32,
}

pub(in crate::dynamodb) async fn reserve_history_append(
    repo: &DynamoUserRepo,
    trip_id: &str,
    new_items: &[std::collections::HashMap<String, AttributeValue>],
) -> Result<Vec<TransactWriteItem>, ContentHistoryRepoError> {
    if new_items.is_empty() {
        return Ok(Vec::new());
    }
    let existing = load_audit_records(repo, trip_id).await?;
    let next_count = existing
        .len()
        .checked_add(new_items.len())
        .filter(|count| *count <= MAX_HISTORY_RECORDS)
        .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)?;
    let mut bytes = existing.iter().try_fold(0_usize, |total, record| {
        total
            .checked_add(record.encoded_bytes)
            .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)
    })?;
    let mut ids = existing
        .iter()
        .map(|record| record.value.id.clone())
        .collect::<HashSet<_>>();
    let mut sort_keys = existing
        .iter()
        .map(|record| record.sort_key.clone())
        .collect::<HashSet<_>>();
    for item in new_items {
        let record = decode_audit_item(item, trip_id)?;
        if record.value.status != EditStatus::Applied
            || record.value.reverted_by.is_some()
            || record.value.reverted_at.is_some()
            || record.value.revert_edit_id.is_some()
            || record.value.reverts_edit_id.is_some()
            || !ids.insert(record.value.id)
            || !sort_keys.insert(record.sort_key)
        {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        bytes = bytes
            .checked_add(
                encoded_item_bytes(item).ok_or(ContentHistoryRepoError::SafetyLimitExceeded)?,
            )
            .filter(|bytes| *bytes <= MAX_HISTORY_BYTES)
            .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)?;
    }

    let first_count = existing.len() + 1;
    (first_count..=next_count)
        .map(|record_count| {
            let item = history_slot_item(trip_id, record_count)?;
            Ok(put_action(repo.create_only_put(item)))
        })
        .collect()
}

pub(super) fn history_slot_item(
    trip_id: &str,
    record_count: usize,
) -> Result<std::collections::HashMap<String, AttributeValue>, ContentHistoryRepoError> {
    encode_record(
        trip_pk(trip_id),
        history_slot_sk(record_count),
        HISTORY_SLOT_ENTITY,
        &HistorySlot {
            record_count: u32::try_from(record_count)
                .map_err(|_| ContentHistoryRepoError::SafetyLimitExceeded)?,
        },
        1,
    )
    .map_err(record_error)
}

pub(super) fn history_slot_sk(record_count: usize) -> String {
    format!("HISTORY#SLOT#{record_count:010}")
}
