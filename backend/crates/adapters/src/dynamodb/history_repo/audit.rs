//! Strict audit decoding, listing, and revert provenance construction.

use chrono::DateTime;

use super::{access::*, *};

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
        .map(|record| record.value)
        .collect::<Vec<_>>();
    edits.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(edits)
}

pub(super) async fn find_audit_record(
    repo: &DynamoUserRepo,
    trip_id: &str,
    edit_id: &str,
) -> Result<Option<Loaded<Edit>>, ContentHistoryRepoError> {
    let mut found = None;
    for record in load_audit_records(repo, trip_id).await? {
        if record.value.id != edit_id {
            continue;
        }
        if found.is_some() {
            return Err(ContentHistoryRepoError::CorruptData);
        }
        found = Some(record);
    }
    Ok(found)
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
    repo.history_query(&pk, "AUDIT#", HISTORY_PAGE_SIZE, true, MAX_HISTORY_RECORDS)
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
    Ok(())
}

fn validate_revert_pair(
    original: &Edit,
    compensation: &Edit,
) -> Result<(), ContentHistoryRepoError> {
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
        && original.old_value == compensation.new_value
        && original.new_value == compensation.old_value
        && original.reverted_by.as_deref() == Some(compensation.author.as_str())
        && original.reverted_at.as_deref() == Some(compensation.created_at.as_str());
    if pair_matches {
        Ok(())
    } else {
        Err(ContentHistoryRepoError::CorruptData)
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
