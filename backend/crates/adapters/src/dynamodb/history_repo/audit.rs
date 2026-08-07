//! Strict audit decoding, listing, and revert provenance construction.

use std::collections::{HashMap, HashSet};

use chrono::DateTime;
use itinera_core::{
    domain::{
        content_history::{ChangeSource, Edit, EditEntity, EditStatus},
        trip::Booking,
        user::UserId,
    },
    ports::content_history::ContentHistoryRepoError,
};

use crate::dynamodb::{
    DynamoUserRepo, SK,
    trip_repo::records::{AUDIT_ENTITY, audit_sk, string, trip_pk},
};

use super::{
    access::{
        HISTORY_PAGE_SIZE, Loaded, MAX_HISTORY_BYTES, MAX_HISTORY_RECORDS,
        MAX_HISTORY_RESPONSE_BYTES, RequiredHistoryRole, decode_loaded,
    },
    record_error,
};

pub(super) async fn list_history(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Vec<Edit>, ContentHistoryRepoError> {
    repo.history_authorize(trip_id, actor, RequiredHistoryRole::Any)
        .await?;
    // A dangling membership must not make an orphaned audit partition look
    // like a valid trip.
    repo.history_trip_meta(trip_id).await?;
    let mut edits = load_audit_records(repo, trip_id)
        .await?
        .into_iter()
        .filter(|record| is_content_history_status(record.value.status))
        .map(|record| record.value)
        .collect::<Vec<_>>();
    edits.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    ensure_history_response_budget(&edits)?;
    Ok(edits)
}

pub(super) struct AuditLookup {
    pub(super) record: Option<Loaded<Edit>>,
    pub(super) record_count: usize,
    pub(super) encoded_bytes: usize,
}

pub(super) async fn find_audit_record(
    repo: &DynamoUserRepo,
    trip_id: &str,
    edit_id: &str,
) -> Result<AuditLookup, ContentHistoryRepoError> {
    let mut found = None;
    let records = load_audit_records(repo, trip_id).await?;
    let record_count = records.len();
    let encoded_bytes = records.iter().try_fold(0_usize, |total, record| {
        total
            .checked_add(record.encoded_bytes)
            .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)
    })?;
    for record in records {
        if record.value.id != edit_id || !is_content_history_status(record.value.status) {
            continue;
        }
        if found.is_some() {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        found = Some(record);
    }
    Ok(AuditLookup {
        record: found,
        record_count,
        encoded_bytes,
    })
}

async fn load_audit_records(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<Vec<Loaded<Edit>>, ContentHistoryRepoError> {
    let records = read_audit_records(repo, trip_id).await?;
    if validate_provenance_graph(&records).is_ok() {
        return Ok(records);
    }

    // Strongly consistent DynamoDB queries guarantee each page is current,
    // but not that multiple pages form one snapshot. A revert can therefore
    // commit between pages and temporarily expose only one side of its
    // reciprocal provenance pair. Retry the complete bounded read once before
    // classifying the graph as corrupt. Structural decoding errors above and
    // below still fail closed without being hidden by this retry.
    let records = read_audit_records(repo, trip_id).await?;
    validate_provenance_graph(&records)?;
    Ok(records)
}

async fn read_audit_records(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<Vec<Loaded<Edit>>, ContentHistoryRepoError> {
    let pk = trip_pk(trip_id);
    repo.history_query(
        &pk,
        "AUDIT#",
        HISTORY_PAGE_SIZE,
        true,
        MAX_HISTORY_RECORDS,
        MAX_HISTORY_BYTES,
    )
    .await?
    .into_iter()
    .map(|item| {
        let sk = string(&item, SK).map_err(record_error)?;
        let record = decode_loaded::<Edit>(&item, &pk, &sk, AUDIT_ENTITY)?;
        validate_edit(trip_id, &record)?;
        Ok(record)
    })
    .collect::<Result<Vec<_>, _>>()
}

fn is_content_history_status(status: EditStatus) -> bool {
    matches!(status, EditStatus::Applied | EditStatus::Reverted)
}

pub(super) fn ensure_history_response_budget(
    edits: &[Edit],
) -> Result<(), ContentHistoryRepoError> {
    let mut bytes = 2_usize; // JSON array brackets.
    for (index, edit) in edits.iter().enumerate() {
        let encoded = serde_json::to_vec(edit).map_err(|_| ContentHistoryRepoError::CorruptData)?;
        bytes = bytes
            .checked_add(encoded.len())
            .and_then(|total| total.checked_add(usize::from(index > 0)))
            .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)?;
        if bytes > MAX_HISTORY_RESPONSE_BYTES {
            return Err(ContentHistoryRepoError::SafetyLimitExceeded);
        }
    }
    Ok(())
}

fn validate_edit(
    expected_trip_id: &str,
    record: &Loaded<Edit>,
) -> Result<(), ContentHistoryRepoError> {
    let edit = &record.value;
    if edit.id.is_empty()
        || edit.id.chars().count() > 200
        || edit.trip_id != expected_trip_id
        || edit.entity_id.is_empty()
        || edit.field.is_empty()
        || edit.author.is_empty()
        || !valid_utc_timestamp(&edit.created_at)
        || record.sort_key != audit_sk(&edit.created_at, &edit.id)
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    if let ChangeSource::Token {
        token_id,
        token_name,
    } = &edit.source
        && (token_id.is_empty() || token_name.is_empty())
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }

    let has_complete_revert = edit.reverted_by.as_ref().is_some_and(|id| !id.is_empty())
        && edit.reverted_at.as_deref().is_some_and(valid_utc_timestamp)
        && edit
            .revert_edit_id
            .as_ref()
            .is_some_and(|id| valid_edit_id(id));
    match edit.status {
        EditStatus::Reverted if !has_complete_revert => {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        EditStatus::Reverted => {}
        _ if edit.reverted_by.is_some()
            || edit.reverted_at.is_some()
            || edit.revert_edit_id.is_some() =>
        {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        _ => {}
    }
    if edit
        .reverts_edit_id
        .as_ref()
        .is_some_and(|id| !valid_edit_id(id) || id == &edit.id)
        || edit
            .revert_edit_id
            .as_ref()
            .is_some_and(|id| !valid_edit_id(id) || id == &edit.id)
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    Ok(())
}

pub(super) fn valid_edit_id(value: &str) -> bool {
    !value.is_empty() && value.chars().count() <= 200
}

pub(super) fn valid_utc_timestamp(value: &str) -> bool {
    value.len() <= 64 && value.ends_with('Z') && DateTime::parse_from_rfc3339(value).is_ok()
}

fn validate_provenance_graph(records: &[Loaded<Edit>]) -> Result<(), ContentHistoryRepoError> {
    let mut by_id = HashMap::with_capacity(records.len());
    for record in records {
        if by_id
            .insert(record.value.id.as_str(), &record.value)
            .is_some()
        {
            return Err(ContentHistoryRepoError::CorruptData);
        }
    }

    for record in records {
        let edit = &record.value;
        if let Some(compensation_id) = edit.revert_edit_id.as_deref() {
            let compensation = by_id
                .get(compensation_id)
                .ok_or(ContentHistoryRepoError::CorruptData)?;
            validate_revert_pair(edit, compensation)?;
        }
        if let Some(original_id) = edit.reverts_edit_id.as_deref() {
            let original = by_id
                .get(original_id)
                .ok_or(ContentHistoryRepoError::CorruptData)?;
            validate_revert_pair(original, edit)?;
        }
    }
    validate_acyclic_reverts(&by_id)?;
    Ok(())
}

fn validate_acyclic_reverts(by_id: &HashMap<&str, &Edit>) -> Result<(), ContentHistoryRepoError> {
    let mut completed = HashSet::with_capacity(by_id.len());
    for edit_id in by_id.keys().copied() {
        if completed.contains(edit_id) {
            continue;
        }
        let mut path = HashSet::new();
        let mut current_id = edit_id;
        loop {
            if completed.contains(current_id) {
                break;
            }
            if !path.insert(current_id) {
                return Err(ContentHistoryRepoError::CorruptData);
            }
            let current = by_id
                .get(current_id)
                .ok_or(ContentHistoryRepoError::CorruptData)?;
            let Some(next_id) = current.revert_edit_id.as_deref() else {
                break;
            };
            current_id = next_id;
        }
        completed.extend(path);
    }
    Ok(())
}

fn validate_revert_pair(
    original: &Edit,
    compensation: &Edit,
) -> Result<(), ContentHistoryRepoError> {
    let original_created = DateTime::parse_from_rfc3339(&original.created_at)
        .map_err(|_| ContentHistoryRepoError::CorruptData)?;
    let compensation_created = DateTime::parse_from_rfc3339(&compensation.created_at)
        .map_err(|_| ContentHistoryRepoError::CorruptData)?;
    let pair_matches = original.status == EditStatus::Reverted
        && matches!(
            compensation.status,
            EditStatus::Applied | EditStatus::Reverted
        )
        && original.revert_edit_id.as_deref() == Some(compensation.id.as_str())
        && compensation.reverts_edit_id.as_deref() == Some(original.id.as_str())
        && original.trip_id == compensation.trip_id
        && original.entity == compensation.entity
        && original.entity_id == compensation.entity_id
        && original.field == compensation.field
        && revert_values_match(original, compensation)
        && original.reverted_by.as_deref() == Some(compensation.author.as_str())
        && original.reverted_at.as_deref() == Some(compensation.created_at.as_str())
        && original_created <= compensation_created;
    if pair_matches {
        Ok(())
    } else {
        Err(ContentHistoryRepoError::CorruptData)
    }
}

pub(super) fn revert_values_match(original: &Edit, compensation: &Edit) -> bool {
    if original.entity != EditEntity::Stop || original.field != "booking" {
        return original.old_value == compensation.new_value
            && original.new_value == compensation.old_value;
    }
    let (Some(mut original_old), Some(mut original_new)) = (
        exact_booking(&original.old_value),
        exact_booking(&original.new_value),
    ) else {
        return false;
    };
    let (Some(mut compensation_old), Some(mut compensation_new)) = (
        exact_booking(&compensation.old_value),
        exact_booking(&compensation.new_value),
    ) else {
        return false;
    };
    if booking_link(&original_old) != booking_link(&original_new)
        || booking_link(&compensation_old) != booking_link(&compensation_new)
    {
        return false;
    }
    clear_booking_link(&mut original_old);
    clear_booking_link(&mut original_new);
    clear_booking_link(&mut compensation_old);
    clear_booking_link(&mut compensation_new);
    original_old == compensation_new && original_new == compensation_old
}

fn exact_booking(value: &serde_json::Value) -> Option<Option<Booking>> {
    let parsed = serde_json::from_value::<Option<Booking>>(value.clone()).ok()?;
    (serde_json::to_value(&parsed).ok()? == *value).then_some(parsed)
}

fn booking_link(booking: &Option<Booking>) -> Option<&str> {
    booking
        .as_ref()
        .and_then(|booking| booking.ledger_entry_id.as_deref())
}

fn clear_booking_link(booking: &mut Option<Booking>) {
    if let Some(booking) = booking {
        booking.ledger_entry_id = None;
    }
}

pub(super) fn reverted_original(
    original: &Edit,
    actor: &UserId,
    reverted_at: &str,
    compensating_edit_id: &str,
) -> Edit {
    let mut reverted = original.clone();
    reverted.status = EditStatus::Reverted;
    reverted.reverted_by = Some(actor.0.clone());
    reverted.reverted_at = Some(reverted_at.to_string());
    reverted.revert_edit_id = Some(compensating_edit_id.to_string());
    reverted
}

pub(super) fn compensating_edit(
    original: &Edit,
    actor: &UserId,
    reverted_at: &str,
    compensating_edit_id: &str,
) -> Edit {
    Edit {
        id: compensating_edit_id.to_string(),
        trip_id: original.trip_id.clone(),
        entity: original.entity,
        entity_id: original.entity_id.clone(),
        field: original.field.clone(),
        old_value: original.new_value.clone(),
        new_value: original.old_value.clone(),
        author: actor.0.clone(),
        source: ChangeSource::Web,
        status: EditStatus::Applied,
        created_at: reverted_at.to_string(),
        reverted_by: None,
        reverted_at: None,
        revert_edit_id: None,
        reverts_edit_id: Some(original.id.clone()),
    }
}
