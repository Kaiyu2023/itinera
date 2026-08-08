//! Bounded history loading, graph validation, and transaction-local appends.

use std::collections::{HashMap, HashSet};

use chrono::DateTime;
use itinera_core::{
    domain::{
        content_history::{ChangeSource, Edit, EditEntity, EditStatus},
        trip::Booking,
        user::UserId,
    },
    ports::content_history::ContentHistoryRepoError,
    services::content_history::validate_stored_edit,
};
use serde_json::Value;
use sqlx::{Sqlite, Transaction};

use super::records::{
    EditRow, HISTORY_QUERY_LIMIT, MAX_HISTORY_RECORDS, MAX_HISTORY_RESPONSE_BYTES, StoredEdit,
    encode_entity, encode_status, encode_value,
};

pub(in crate::sqlite) struct AuditChange<'a> {
    pub(in crate::sqlite) entity: EditEntity,
    pub(in crate::sqlite) entity_id: &'a str,
    pub(in crate::sqlite) field: &'a str,
    pub(in crate::sqlite) old_value: Value,
    pub(in crate::sqlite) new_value: Value,
}

pub(in crate::sqlite) fn audit(
    trip_id: &str,
    actor: &UserId,
    changed_at: &str,
    change_id: &str,
    change: AuditChange<'_>,
) -> Edit {
    Edit {
        id: change_id.to_string(),
        trip_id: trip_id.to_string(),
        entity: change.entity,
        entity_id: change.entity_id.to_string(),
        field: change.field.to_string(),
        old_value: change.old_value,
        new_value: change.new_value,
        author: actor.0.clone(),
        source: ChangeSource::Web {},
        status: EditStatus::Applied,
        created_at: changed_at.to_string(),
        reverted_by: None,
        reverted_at: None,
        revert_edit_id: None,
        reverts_edit_id: None,
    }
}

pub(in crate::sqlite) fn suffixed_id(base: &str, index: usize) -> String {
    format!("{base}-{index:02}")
}

pub(super) async fn load_history(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<StoredEdit>, ContentHistoryRepoError> {
    // Bound raw UTF-8 payload before sqlx materializes any values. The exact
    // response-size check below remains authoritative because JSON field names
    // and escaping add bytes, but raw payload alone can already prove a result
    // cannot fit without allocating every row first.
    let (count, raw_payload_bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM( \
             length(CAST(trip_id AS BLOB)) \
             + length(CAST(id AS BLOB)) \
             + length(CAST(entity AS BLOB)) \
             + length(CAST(entity_id AS BLOB)) \
             + length(CAST(field AS BLOB)) \
             + length(CAST(old_value_json AS BLOB)) \
             + length(CAST(new_value_json AS BLOB)) \
             + length(CAST(author_id AS BLOB)) \
             + length(CAST(source_kind AS BLOB)) \
             + COALESCE(length(CAST(source_service_id AS BLOB)), 0) \
             + COALESCE(length(CAST(source_service_name AS BLOB)), 0) \
             + length(CAST(status AS BLOB)) \
             + length(CAST(created_at AS BLOB)) \
             + COALESCE(length(CAST(reverted_by AS BLOB)), 0) \
             + COALESCE(length(CAST(reverted_at AS BLOB)), 0) \
             + COALESCE(length(CAST(revert_edit_id AS BLOB)), 0) \
             + COALESCE(length(CAST(reverts_edit_id AS BLOB)), 0) \
         ), 0) \
         FROM content_edits WHERE trip_id = ?",
    )
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let count = usize::try_from(count).map_err(corrupt)?;
    let raw_payload_bytes = usize::try_from(raw_payload_bytes).map_err(corrupt)?;
    if count > MAX_HISTORY_RECORDS || raw_payload_bytes > MAX_HISTORY_RESPONSE_BYTES {
        return Err(ContentHistoryRepoError::SafetyLimitExceeded);
    }
    let rows = sqlx::query_as::<_, EditRow>(
        "SELECT trip_id AS edit_trip_id, id AS edit_id, entity AS edit_entity, \
                entity_id AS edit_entity_id, field AS edit_field, old_value_json, \
                new_value_json, author_id, source_kind, source_service_id, \
                source_service_name, status AS edit_status, \
                created_at AS edit_created_at, reverted_by, reverted_at, \
                revert_edit_id, reverts_edit_id, revision AS edit_revision \
         FROM content_edits WHERE trip_id = ? \
         ORDER BY created_at, id LIMIT ?",
    )
    .bind(trip_id)
    .bind(HISTORY_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() != count {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let records = rows
        .into_iter()
        .map(|row| row.into_edit(trip_id))
        .collect::<Result<Vec<_>, _>>()?;
    // Later migrations have schema-shaped values in this shared table, but
    // their reciprocal source/target records do not exist yet. Keep each
    // staged variant unreadable until its owning capability can validate it.
    if records
        .iter()
        .any(|record| !schema_three_supports(&record.value))
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    validate_provenance_graph(&records)?;
    ensure_response_budget(&history_values(&records)?)?;
    Ok(records)
}

fn schema_three_supports(edit: &Edit) -> bool {
    if matches!(edit.source, ChangeSource::Service { .. }) || edit.entity == EditEntity::Notice {
        return false;
    }
    if edit.entity == EditEntity::Candidate
        && edit.field == "status"
        && [&edit.old_value, &edit.new_value]
            .into_iter()
            .any(|value| value.as_str() == Some("in_plan"))
    {
        return false;
    }
    if edit.entity == EditEntity::Stop && edit.field == "booking" {
        return [&edit.old_value, &edit.new_value].into_iter().all(|value| {
            exact_booking(value).is_some_and(|booking| booking_link(&booking).is_none())
        });
    }
    true
}

pub(in crate::sqlite) async fn append_edits(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    edits: &[Edit],
) -> Result<(), ContentHistoryRepoError> {
    let records = load_history(transaction, trip_id).await?;
    let mut projected = history_values(&records)?;
    let mut ids = projected
        .iter()
        .map(|edit| edit.id.clone())
        .collect::<HashSet<_>>();
    for edit in edits {
        validate_stored_edit(trip_id, edit).map_err(corrupt)?;
        if !matches!(edit.source, ChangeSource::Web {}) || !ids.insert(edit.id.clone()) {
            return Err(ContentHistoryRepoError::Conflict);
        }
        projected.push(edit.clone());
    }
    validate_projected(&projected)?;
    for edit in edits {
        insert_edit(transaction, edit, 1).await?;
    }
    Ok(())
}

pub(super) fn validate_projected(edits: &[Edit]) -> Result<(), ContentHistoryRepoError> {
    if edits.len() > MAX_HISTORY_RECORDS {
        return Err(ContentHistoryRepoError::SafetyLimitExceeded);
    }
    let records = edits
        .iter()
        .cloned()
        .map(|value| StoredEdit { value, revision: 1 })
        .collect::<Vec<_>>();
    validate_provenance_graph(&records)?;
    ensure_response_budget(edits)
}

pub(super) async fn insert_edit(
    transaction: &mut Transaction<'static, Sqlite>,
    edit: &Edit,
    revision: i64,
) -> Result<(), ContentHistoryRepoError> {
    let (source_kind, source_service_id, source_service_name) = match &edit.source {
        ChangeSource::Web {} => ("web", None, None),
        ChangeSource::Service {
            service_identity_id,
            service_identity_name,
        } => (
            "service",
            Some(service_identity_id.as_str()),
            Some(service_identity_name.as_str()),
        ),
    };
    sqlx::query(
        "INSERT INTO content_edits ( \
             trip_id, id, entity, entity_id, field, old_value_json, new_value_json, \
             author_id, source_kind, source_service_id, source_service_name, status, \
             created_at, reverted_by, reverted_at, revert_edit_id, reverts_edit_id, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&edit.trip_id)
    .bind(&edit.id)
    .bind(encode_entity(edit.entity))
    .bind(&edit.entity_id)
    .bind(&edit.field)
    .bind(encode_value(&edit.old_value)?)
    .bind(encode_value(&edit.new_value)?)
    .bind(&edit.author)
    .bind(source_kind)
    .bind(source_service_id)
    .bind(source_service_name)
    .bind(encode_status(edit.status)?)
    .bind(&edit.created_at)
    .bind(&edit.reverted_by)
    .bind(&edit.reverted_at)
    .bind(&edit.revert_edit_id)
    .bind(&edit.reverts_edit_id)
    .bind(revision)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    Ok(())
}

pub(super) fn history_values(records: &[StoredEdit]) -> Result<Vec<Edit>, ContentHistoryRepoError> {
    let mut edits = records
        .iter()
        .map(|record| {
            DateTime::parse_from_rfc3339(&record.value.created_at)
                .map(|timestamp| (timestamp, record.value.clone()))
                .map_err(corrupt)
        })
        .collect::<Result<Vec<_>, _>>()?;
    edits.sort_by(|(left_time, left), (right_time, right)| {
        right_time
            .cmp(left_time)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(edits.into_iter().map(|(_, edit)| edit).collect())
}

fn ensure_response_budget(edits: &[Edit]) -> Result<(), ContentHistoryRepoError> {
    let encoded = serde_json::to_vec(edits).map_err(corrupt)?;
    if encoded.len() > MAX_HISTORY_RESPONSE_BYTES {
        Err(ContentHistoryRepoError::SafetyLimitExceeded)
    } else {
        Ok(())
    }
}

fn validate_provenance_graph(records: &[StoredEdit]) -> Result<(), ContentHistoryRepoError> {
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
    validate_acyclic(&by_id)
}

fn validate_revert_pair(
    original: &Edit,
    compensation: &Edit,
) -> Result<(), ContentHistoryRepoError> {
    let original_created = DateTime::parse_from_rfc3339(&original.created_at).map_err(corrupt)?;
    let compensation_created =
        DateTime::parse_from_rfc3339(&compensation.created_at).map_err(corrupt)?;
    let matches = original.status == EditStatus::Reverted
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
    if matches {
        Ok(())
    } else {
        Err(ContentHistoryRepoError::CorruptData)
    }
}

fn validate_acyclic(by_id: &HashMap<&str, &Edit>) -> Result<(), ContentHistoryRepoError> {
    let mut complete = HashSet::new();
    for edit_id in by_id.keys().copied() {
        if complete.contains(edit_id) {
            continue;
        }
        let mut path = HashSet::new();
        let mut current_id = edit_id;
        loop {
            if complete.contains(current_id) {
                break;
            }
            if !path.insert(current_id) {
                return Err(ContentHistoryRepoError::CorruptData);
            }
            let current = by_id
                .get(current_id)
                .ok_or(ContentHistoryRepoError::CorruptData)?;
            let Some(next) = current.revert_edit_id.as_deref() else {
                break;
            };
            current_id = next;
        }
        complete.extend(path);
    }
    Ok(())
}

fn revert_values_match(original: &Edit, compensation: &Edit) -> bool {
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

fn exact_booking(value: &Value) -> Option<Option<Booking>> {
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

fn unavailable<T>(_error: T) -> ContentHistoryRepoError {
    ContentHistoryRepoError::Unavailable
}

fn corrupt<T>(_error: T) -> ContentHistoryRepoError {
    ContentHistoryRepoError::CorruptData
}
