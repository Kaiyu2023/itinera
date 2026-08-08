//! Bounded history loading, graph validation, and transaction-local appends.

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::DateTime;
use itinera_core::{
    domain::{
        content_history::{ChangeSource, Edit, EditEntity, EditStatus},
        trip::{Booking, Place},
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
    validate_schema_four_support(transaction, trip_id, &records).await?;
    validate_provenance_graph(&records)?;
    ensure_response_budget(&history_values(&records)?)?;
    Ok(records)
}

fn schema_four_base_supports(edit: &Edit) -> bool {
    if matches!(edit.source, ChangeSource::Service { .. }) || edit.entity == EditEntity::Notice {
        return false;
    }
    if edit.entity == EditEntity::Stop && edit.field == "booking" {
        return [&edit.old_value, &edit.new_value].into_iter().all(|value| {
            exact_booking(value).is_some_and(|booking| booking_link(&booking).is_none())
        });
    }
    true
}

async fn validate_schema_four_support(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    records: &[StoredEdit],
) -> Result<(), ContentHistoryRepoError> {
    if records
        .iter()
        .any(|record| !schema_four_base_supports(&record.value))
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let structural_ids = records
        .iter()
        .filter(|record| structural_candidate_status(&record.value))
        .map(|record| record.value.id.as_str())
        .collect::<HashSet<_>>();
    let expected =
        crate::sqlite::trip_repo::plans::load_plan_structural_audits(transaction, trip_id)
            .await
            .map_err(map_trip_error)?;
    let expected_by_id = expected
        .iter()
        .map(|(plan, binding)| (binding.edit.id.as_str(), (plan, binding)))
        .collect::<HashMap<_, _>>();
    if expected_by_id.len() != expected.len()
        || expected_by_id.len() != structural_ids.len()
        || structural_ids
            .iter()
            .any(|edit_id| !expected_by_id.contains_key(edit_id))
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let links = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            Option<String>,
            Option<i64>,
            Option<String>,
        ),
    >(
        "SELECT link.edit_id, link.proposal_id, link.candidate_id, \
                link.candidate_place_id, edit.proposal_candidate_place_id, \
                plan.version, plan.created_at \
         FROM proposal_content_edits AS link \
         LEFT JOIN content_edits AS edit \
           ON edit.trip_id = link.trip_id AND edit.id = link.edit_id \
         LEFT JOIN plans AS plan \
           ON plan.trip_id = link.trip_id \
          AND plan.created_from_proposal_id = link.proposal_id \
         WHERE link.trip_id = ? ORDER BY link.edit_id",
    )
    .bind(trip_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if links.len() != expected_by_id.len() {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let by_id = records
        .iter()
        .map(|record| (record.value.id.as_str(), &record.value))
        .collect::<HashMap<_, _>>();
    let candidate_place_snapshots =
        validate_candidate_place_snapshots(transaction, trip_id, records).await?;
    let mut linked_ids = HashSet::new();
    for (
        edit_id,
        proposal_id,
        candidate_id,
        place_id,
        audit_place_id,
        plan_version,
        plan_created_at,
    ) in links
    {
        if !linked_ids.insert(edit_id.clone()) || !structural_ids.contains(edit_id.as_str()) {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        let edit = by_id
            .get(edit_id.as_str())
            .ok_or(ContentHistoryRepoError::CorruptData)?;
        let (expected_plan, expected_binding) = expected_by_id
            .get(edit_id.as_str())
            .ok_or(ContentHistoryRepoError::CorruptData)?;
        let (Some(plan_version), Some(plan_created_at)) = (plan_version, plan_created_at) else {
            return Err(ContentHistoryRepoError::CorruptData);
        };
        let plan_version = u32::try_from(plan_version)
            .ok()
            .filter(|version| *version > 1)
            .ok_or(ContentHistoryRepoError::CorruptData)?;
        if *edit != &expected_binding.edit
            || edit.entity_id != candidate_id
            || proposal_id
                != expected_plan
                    .created_from_proposal_id
                    .as_deref()
                    .unwrap_or_default()
            || expected_binding.candidate_place_id != place_id
            || audit_place_id.as_deref() != Some(place_id.as_str())
            || !candidate_place_snapshots
                .get(candidate_id.as_str())
                .is_some_and(|place_ids| place_ids.contains(place_id.as_str()))
            || edit.created_at != plan_created_at
            || expected_plan.version != plan_version
            || expected_plan.created_at != plan_created_at
        {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        let (base_adopted, published_adopted): (i64, i64) = sqlx::query_as(
            "SELECT \
                 EXISTS(SELECT 1 FROM plan_stops \
                        WHERE trip_id = ? AND plan_version = ? AND place_id = ?), \
                 EXISTS(SELECT 1 FROM plan_stops \
                        WHERE trip_id = ? AND plan_version = ? AND place_id = ?)",
        )
        .bind(trip_id)
        .bind(i64::from(plan_version - 1))
        .bind(&place_id)
        .bind(trip_id)
        .bind(i64::from(plan_version))
        .bind(&place_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(unavailable)?;
        let transition_matches = matches!(
            (
                edit.old_value.as_str(),
                edit.new_value.as_str(),
                base_adopted > 0,
                published_adopted > 0,
            ),
            (Some("shortlisted"), Some("in_plan"), false, true)
                | (Some("in_plan"), Some("shortlisted"), true, false)
        );
        if !transition_matches {
            return Err(ContentHistoryRepoError::CorruptData);
        }
    }
    Ok(())
}

async fn validate_candidate_place_snapshots(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    records: &[StoredEdit],
) -> Result<HashMap<String, HashSet<String>>, ContentHistoryRepoError> {
    let current_rows = sqlx::query_as::<_, (String, String)>(
        "SELECT id, place_id FROM candidates WHERE trip_id = ? ORDER BY id LIMIT 1001",
    )
    .bind(trip_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if current_rows.len() > 1_000 {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let current_by_candidate = current_rows.into_iter().collect::<HashMap<_, _>>();
    let mut transitions = HashMap::<String, Vec<(String, String)>>::new();
    let mut stored_snapshots = HashMap::<String, Value>::new();
    for record in records {
        let edit = &record.value;
        if edit.entity != EditEntity::Candidate || edit.field != "place" {
            continue;
        }
        if !current_by_candidate.contains_key(&edit.entity_id) {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        let old = serde_json::from_value::<Place>(edit.old_value.clone()).map_err(corrupt)?;
        let new = serde_json::from_value::<Place>(edit.new_value.clone()).map_err(corrupt)?;
        if old.id == new.id {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        for (place, encoded) in [(&old, &edit.old_value), (&new, &edit.new_value)] {
            if stored_snapshots
                .insert(place.id.clone(), encoded.clone())
                .is_some_and(|previous| previous != *encoded)
            {
                return Err(ContentHistoryRepoError::CorruptData);
            }
        }
        transitions
            .entry(edit.entity_id.clone())
            .or_default()
            .push((old.id, new.id));
    }

    let mut owned = HashMap::with_capacity(current_by_candidate.len());
    for (candidate_id, current_place_id) in current_by_candidate {
        let candidate_transitions = transitions.remove(&candidate_id).unwrap_or_default();
        let mut adjacency = HashMap::<String, Vec<String>>::new();
        let mut degrees = HashMap::<String, (usize, usize)>::new();
        adjacency.entry(current_place_id.clone()).or_default();
        degrees.entry(current_place_id.clone()).or_default();
        for (old, new) in candidate_transitions {
            adjacency.entry(old.clone()).or_default().push(new.clone());
            adjacency.entry(new.clone()).or_default().push(old.clone());
            degrees.entry(old).or_default().0 += 1;
            degrees.entry(new).or_default().1 += 1;
        }

        let mut reached = HashSet::new();
        let mut pending = VecDeque::from([current_place_id.clone()]);
        while let Some(place_id) = pending.pop_front() {
            if !reached.insert(place_id.clone()) {
                continue;
            }
            if let Some(neighbours) = adjacency.get(&place_id) {
                pending.extend(neighbours.iter().cloned());
            }
        }
        if reached.len() != adjacency.len() {
            return Err(ContentHistoryRepoError::CorruptData);
        }

        let mut starts = 0;
        let mut ends = 0;
        for (place_id, (outgoing, incoming)) in degrees {
            let difference = i64::try_from(outgoing).map_err(corrupt)?
                - i64::try_from(incoming).map_err(corrupt)?;
            match difference {
                0 => {}
                1 => starts += 1,
                -1 if place_id == current_place_id => ends += 1,
                _ => return Err(ContentHistoryRepoError::CorruptData),
            }
        }
        if !((starts == 0 && ends == 0) || (starts == 1 && ends == 1)) {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        owned.insert(candidate_id, reached);
    }
    if !transitions.is_empty() {
        return Err(ContentHistoryRepoError::CorruptData);
    }

    let mut owner_by_place = HashMap::new();
    for (candidate_id, place_ids) in &owned {
        for place_id in place_ids {
            if owner_by_place
                .insert(place_id.as_str(), candidate_id.as_str())
                .is_some_and(|previous| previous != candidate_id)
            {
                return Err(ContentHistoryRepoError::CorruptData);
            }
        }
    }
    let place_ids = owner_by_place.keys().copied().collect::<HashSet<_>>();
    let encoded_ids = serde_json::to_string(&place_ids).map_err(corrupt)?;
    let stored_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trip_places WHERE trip_id = ? \
         AND id IN (SELECT CAST(value AS TEXT) FROM json_each(?))",
    )
    .bind(trip_id)
    .bind(encoded_ids)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if usize::try_from(stored_count).ok() != Some(place_ids.len()) {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    Ok(owned)
}

fn structural_candidate_status(edit: &Edit) -> bool {
    edit.entity == EditEntity::Candidate
        && edit.field == "status"
        && [&edit.old_value, &edit.new_value]
            .into_iter()
            .any(|value| value.as_str() == Some("in_plan"))
}

fn map_trip_error(error: itinera_core::ports::trip::TripRepoError) -> ContentHistoryRepoError {
    match error {
        itinera_core::ports::trip::TripRepoError::Unavailable => {
            ContentHistoryRepoError::Unavailable
        }
        itinera_core::ports::trip::TripRepoError::CorruptData
        | itinera_core::ports::trip::TripRepoError::NotFound
        | itinera_core::ports::trip::TripRepoError::Forbidden
        | itinera_core::ports::trip::TripRepoError::Conflict
        | itinera_core::ports::trip::TripRepoError::DuplicateInvite => {
            ContentHistoryRepoError::CorruptData
        }
    }
}

pub(in crate::sqlite) async fn append_edits(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    edits: &[Edit],
) -> Result<(), ContentHistoryRepoError> {
    validate_append(transaction, trip_id, edits).await?;
    for edit in edits {
        insert_edit_with_binding(transaction, edit, 1, None).await?;
    }
    Ok(())
}

pub(in crate::sqlite) async fn append_proposal_edits(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    edits: &[Edit],
    candidate_place_ids: &[String],
) -> Result<(), ContentHistoryRepoError> {
    if edits.len() != candidate_place_ids.len() {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    validate_append(transaction, trip_id, edits).await?;
    for (edit, place_id) in edits.iter().zip(candidate_place_ids) {
        insert_edit_with_binding(transaction, edit, 1, Some(place_id)).await?;
    }
    Ok(())
}

async fn validate_append(
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
    insert_edit_with_binding(transaction, edit, revision, None).await
}

async fn insert_edit_with_binding(
    transaction: &mut Transaction<'static, Sqlite>,
    edit: &Edit,
    revision: i64,
    proposal_candidate_place_id: Option<&str>,
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
             created_at, reverted_by, reverted_at, revert_edit_id, reverts_edit_id, \
             proposal_candidate_place_id, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
    .bind(proposal_candidate_place_id)
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
