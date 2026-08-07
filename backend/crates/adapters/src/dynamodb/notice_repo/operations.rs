use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;
use chrono::{Duration, SecondsFormat};
use itinera_core::{
    domain::{
        notice::{ChecklistMode, Notice},
        trip::TripRole,
        user::UserId,
    },
    ports::{
        content_history::ContentHistoryRepoError,
        notice::{ChecklistToggle, NewNotice, NoticeRepoError, NoticeUpdate},
    },
    services::notices::{
        CreateNoticeInput, MAX_CHECKLIST_COMPLETIONS, MAX_NOTICES, notice_creation_request_hash,
        retain_audience_completions, validate_stored_notice,
    },
};

use crate::dynamodb::{
    DynamoUserRepo, SK,
    history_repo::reservation::reserve_history_append,
    primitives::{
        condition_action, delete_action, encoded_item_bytes, put_action,
        transaction_condition_failed,
    },
    trip_repo::{
        audit::{AuditChange, audit, suffixed_id},
        records::{AUDIT_ENTITY, audit_sk, encode_record, string, trip_pk},
    },
};

use super::{
    access::{MAX_NOTICE_BYTES, NoticeMembershipSnapshot, RequiredNoticeRole},
    records::{
        Loaded, LoadedNotice, MAX_NOTICE_OPERATIONS_PER_ACTOR, NOTICE_ENTITY, NOTICE_META_ENTITY,
        NOTICE_META_SK, NOTICE_OPERATION_ENTITY, NOTICE_OPERATION_META_ENTITY,
        NOTICE_OPERATION_META_SK, NOTICE_OPERATION_PREFIX, NOTICE_PREFIX, NoticeMetaRecord,
        NoticeOperationMetaRecord, NoticeOperationRecord, NoticeOperationResult, decode_notice,
        decode_notice_meta, decode_notice_operation, decode_notice_operation_meta, encode_notice,
        encode_notice_meta, encode_notice_operation, encode_notice_operation_meta,
        notice_operation_key_hash, notice_operation_partition_key, notice_operation_sk, utc,
    },
};

const MAX_TRANSACTION_ACTIONS: usize = 100;

struct NoticeState {
    meta: Option<Loaded<NoticeMetaRecord>>,
    notices: HashMap<String, LoadedNotice>,
    encoded_bytes: usize,
}

struct OperationState {
    meta: Option<Loaded<NoticeOperationMetaRecord>>,
    operations: HashMap<String, Loaded<NoticeOperationRecord>>,
}

pub(in crate::dynamodb) struct NoticeAggregateGuard {
    pub(in crate::dynamodb) meta: Loaded<NoticeMetaRecord>,
    pub(in crate::dynamodb) encoded_bytes: usize,
}

pub(in crate::dynamodb) async fn load_notice_aggregate_guard(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<NoticeAggregateGuard, NoticeRepoError> {
    let state = load_notice_state_with_retry(repo, trip_id).await?;
    Ok(NoticeAggregateGuard {
        meta: state.meta.ok_or(NoticeRepoError::CorruptData)?,
        encoded_bytes: state.encoded_bytes,
    })
}

pub(super) async fn replay_notice_creation(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
    now: &str,
) -> Result<Option<Notice>, NoticeRepoError> {
    repo.notice_authorize(trip_id, actor, RequiredNoticeRole::Editor)
        .await?;
    let Some(result) =
        replay_operation(repo, trip_id, actor, idempotency_key, request_hash, now).await?
    else {
        return Ok(None);
    };
    let state = load_notice_state_with_retry(repo, trip_id).await?;
    match result {
        NoticeOperationResult::Created { notice_id } => Ok(Some(
            resolve_operation_notice(&state, actor, &notice_id, None)?.clone(),
        )),
        NoticeOperationResult::ChecklistToggled { .. } => Err(NoticeRepoError::Conflict),
    }
}

pub(super) async fn replay_checklist_toggle(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
    now: &str,
) -> Result<Option<Notice>, NoticeRepoError> {
    repo.notice_authorize(trip_id, actor, RequiredNoticeRole::Any)
        .await?;
    let Some(result) =
        replay_operation(repo, trip_id, actor, idempotency_key, request_hash, now).await?
    else {
        return Ok(None);
    };
    let state = load_notice_state_with_retry(repo, trip_id).await?;
    match result {
        NoticeOperationResult::ChecklistToggled { notice_id, item_id } => Ok(Some(
            resolve_operation_notice(&state, actor, &notice_id, Some(&item_id))?.clone(),
        )),
        NoticeOperationResult::Created { .. } => Err(NoticeRepoError::Conflict),
    }
}

pub(super) async fn list_notices(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Vec<Notice>, NoticeRepoError> {
    repo.notice_authorize(trip_id, actor, RequiredNoticeRole::Any)
        .await?;
    let state = load_notice_state_with_retry(repo, trip_id).await?;
    let mut notices = state.notices.into_values().collect::<Vec<_>>();
    notices.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.notice.id.cmp(&left.notice.id))
    });
    Ok(notices.into_iter().map(|loaded| loaded.notice).collect())
}

pub(super) async fn create_notice(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    new: NewNotice,
) -> Result<Notice, NoticeRepoError> {
    let expected_request_hash = notice_creation_request_hash(&CreateNoticeInput {
        category: new.notice.category,
        title: new.notice.title.clone(),
        body: new.notice.body.clone(),
        source_url: new.notice.source_url.clone(),
        checklist_items: new
            .notice
            .checklist_items
            .iter()
            .map(|item| item.text.clone())
            .collect(),
        audience: new.notice.audience.clone(),
    })
    .map_err(|_| NoticeRepoError::CorruptData)?;
    if new.notice.trip_id != trip_id
        || new.notice.created_by != actor.0
        || new.notice.pinned
        || new.notice.status != itinera_core::domain::notice::NoticeStatus::Active
        || new.notice.checklist_items.iter().any(|item| {
            !item.done_by.is_empty() || item.due_date.is_some() || item.mode != ChecklistMode::Each
        })
        || new.request_hash != expected_request_hash
    {
        return Err(NoticeRepoError::CorruptData);
    }
    let operation = notice_operation(
        trip_id,
        actor,
        &new.idempotency_key,
        &new.request_hash,
        NoticeOperationResult::Created {
            notice_id: new.notice.id.clone(),
        },
        &new.created_at,
    )?;
    let notice_item = encode_notice(&new.notice, &new.created_at, 1)?;
    let notice_item_bytes =
        encoded_item_bytes(&notice_item).ok_or(NoticeRepoError::SafetyLimitExceeded)?;
    let operation_item = encode_notice_operation(&operation)?;
    repo.notice_authorize(trip_id, actor, RequiredNoticeRole::Editor)
        .await?;
    if let Some(result) = replay_operation(
        repo,
        trip_id,
        actor,
        &new.idempotency_key,
        &new.request_hash,
        &new.created_at,
    )
    .await?
    {
        let state = load_notice_state_with_retry(repo, trip_id).await?;
        return match result {
            NoticeOperationResult::Created { notice_id } => {
                Ok(resolve_operation_notice(&state, actor, &notice_id, None)?.clone())
            }
            NoticeOperationResult::ChecklistToggled { .. } => Err(NoticeRepoError::Conflict),
        };
    }
    let state = load_notice_state_with_retry(repo, trip_id).await?;
    let operation_state = load_operation_state_with_retry(repo, trip_id, actor).await?;
    if let Some(result) = replay_operation_from_state(
        &operation_state,
        actor,
        &new.idempotency_key,
        &new.request_hash,
        &new.created_at,
    )? {
        return match result {
            NoticeOperationResult::Created { notice_id } => {
                Ok(resolve_operation_notice(&state, actor, &notice_id, None)?.clone())
            }
            NoticeOperationResult::ChecklistToggled { .. } => Err(NoticeRepoError::Conflict),
        };
    }
    if state.notices.len() >= MAX_NOTICES {
        return Err(NoticeRepoError::SafetyLimitExceeded);
    }
    let next_notice_bytes = state
        .encoded_bytes
        .checked_add(notice_item_bytes)
        .filter(|bytes| *bytes <= MAX_NOTICE_BYTES)
        .ok_or(NoticeRepoError::SafetyLimitExceeded)?;
    if state.notices.contains_key(&new.notice.id) {
        return Err(NoticeRepoError::Conflict);
    }
    let membership_snapshot = if let Some(audience) = new.notice.audience.as_deref() {
        let snapshot = repo.notice_membership_snapshot(trip_id).await?;
        require_current_audience(&snapshot, audience)?;
        Some(snapshot)
    } else {
        None
    };
    let next_count =
        u32::try_from(state.notices.len() + 1).map_err(|_| NoticeRepoError::SafetyLimitExceeded)?;
    let next_meta = NoticeMetaRecord {
        trip_id: trip_id.to_string(),
        notice_count: next_count,
        notice_bytes: u32::try_from(next_notice_bytes)
            .map_err(|_| NoticeRepoError::SafetyLimitExceeded)?,
    };
    let next_meta_revision = match &state.meta {
        Some(meta) => meta
            .revision
            .checked_add(1)
            .ok_or(NoticeRepoError::CorruptData)?,
        None => 1,
    };
    let meta_item = encode_notice_meta(&next_meta, next_meta_revision)?;
    let meta_put = match &state.meta {
        Some(meta) => {
            repo.entity_snapshot_put(meta_item, NOTICE_META_ENTITY, meta.revision, &meta.raw_data)
        }
        None => repo.create_only_put(meta_item),
    };
    let mut actions = vec![condition_action(repo.notice_membership_condition(
        trip_id,
        actor,
        RequiredNoticeRole::Editor,
    ))];
    if let Some(snapshot) = &membership_snapshot {
        actions.push(condition_action(
            repo.notice_membership_snapshot_condition(trip_id, snapshot),
        ));
    }
    actions.extend([
        put_action(meta_put),
        put_action(repo.create_only_put(notice_item)),
    ]);
    actions.extend(operation_claim_actions(
        repo,
        &operation_state,
        &operation,
        operation_item,
        &new.created_at,
    )?);
    if actions.len() > MAX_TRANSACTION_ACTIONS {
        return Err(NoticeRepoError::SafetyLimitExceeded);
    }
    let result = repo
        .transaction()
        .set_transact_items(Some(actions))
        .send()
        .await;
    if result.is_ok() {
        return Ok(new.notice);
    }
    repo.notice_authorize(trip_id, actor, RequiredNoticeRole::Editor)
        .await?;
    if let Some(notice) = replay_notice_creation(
        repo,
        trip_id,
        actor,
        &new.idempotency_key,
        &new.request_hash,
        &new.created_at,
    )
    .await?
    {
        return Ok(notice);
    }
    if result
        .as_ref()
        .err()
        .is_some_and(|error| transaction_condition_failed(error.as_service_error()))
    {
        Err(NoticeRepoError::Conflict)
    } else {
        Err(NoticeRepoError::Unavailable)
    }
}

pub(super) async fn update_notice(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    notice_id: &str,
    update: NoticeUpdate,
) -> Result<Notice, NoticeRepoError> {
    let role = repo
        .notice_authorize(trip_id, actor, RequiredNoticeRole::Any)
        .await?;
    let state = load_notice_state_with_retry(repo, trip_id).await?;
    let loaded = state
        .notices
        .get(notice_id)
        .ok_or(NoticeRepoError::NotFound)?;
    let is_author = loaded.notice.created_by == actor.0;
    if role != TripRole::Leader && !(role == TripRole::Member && is_author) {
        return Err(NoticeRepoError::Forbidden);
    }
    let NoticeUpdate {
        patch,
        changed_at,
        change_id,
    } = update;
    validate_change_metadata(&changed_at, &change_id)?;
    let membership_snapshot = if patch.audience.is_some() {
        Some(repo.notice_membership_snapshot(trip_id).await?)
    } else {
        None
    };
    let mut updated = loaded.notice.clone();
    let mut changes = Vec::new();
    if let Some(value) = patch.title {
        record_change(&mut changes, "title", &updated.title, &value)?;
        updated.title = value;
    }
    if let Some(value) = patch.body {
        record_change(&mut changes, "body", &updated.body, &value)?;
        updated.body = value;
    }
    if let Some(value) = patch.pinned {
        record_change(&mut changes, "pinned", &updated.pinned, &value)?;
        updated.pinned = value;
    }
    if let Some(value) = patch.source_url {
        record_change(&mut changes, "sourceUrl", &updated.source_url, &value)?;
        updated.source_url = value;
    }
    if let Some(value) = patch.status {
        record_change(&mut changes, "status", &updated.status, &value)?;
        updated.status = value;
    }
    if let Some(value) = patch.audience {
        let snapshot = membership_snapshot
            .as_ref()
            .ok_or(NoticeRepoError::CorruptData)?;
        require_current_audience(snapshot, value.as_deref().unwrap_or_default())?;
        record_change(&mut changes, "audience", &updated.audience, &value)?;
        updated.audience = value;
        retain_audience_completions(&mut updated, &snapshot.member_ids);
    }
    validate_stored_notice(trip_id, &updated).map_err(|_| NoticeRepoError::Conflict)?;
    let completion_cleanup_changed = updated.checklist_items != loaded.notice.checklist_items;
    if changes.is_empty() && !completion_cleanup_changed {
        return Ok(updated);
    }
    let required = if is_author {
        RequiredNoticeRole::Editor
    } else {
        RequiredNoticeRole::Leader
    };
    let next_revision = loaded
        .revision
        .checked_add(1)
        .ok_or(NoticeRepoError::CorruptData)?;
    let updated_item = encode_notice(&updated, &loaded.created_at, next_revision)?;
    let updated_bytes =
        encoded_item_bytes(&updated_item).ok_or(NoticeRepoError::SafetyLimitExceeded)?;
    let next_notice_bytes = replacement_bytes(&state, loaded.encoded_bytes, updated_bytes)?;
    let meta = state.meta.as_ref().ok_or(NoticeRepoError::CorruptData)?;
    let next_meta = NoticeMetaRecord {
        trip_id: trip_id.to_string(),
        notice_count: meta.value.notice_count,
        notice_bytes: u32::try_from(next_notice_bytes)
            .map_err(|_| NoticeRepoError::SafetyLimitExceeded)?,
    };
    let mut actions = vec![
        condition_action(repo.notice_membership_condition(trip_id, actor, required)),
        put_action(
            repo.entity_snapshot_put(
                encode_notice_meta(
                    &next_meta,
                    meta.revision
                        .checked_add(1)
                        .ok_or(NoticeRepoError::CorruptData)?,
                )?,
                NOTICE_META_ENTITY,
                meta.revision,
                &meta.raw_data,
            ),
        ),
    ];
    if let Some(snapshot) = &membership_snapshot {
        actions.push(condition_action(
            repo.notice_membership_snapshot_condition(trip_id, snapshot),
        ));
    }
    actions.push(put_action(repo.entity_snapshot_put(
        updated_item,
        NOTICE_ENTITY,
        loaded.revision,
        &loaded.raw_data,
    )));
    let mut audit_items = Vec::with_capacity(changes.len());
    for (index, (field, old_value, new_value)) in changes.into_iter().enumerate() {
        let event_id = suffixed_id(&change_id, index);
        if !valid_id(&event_id) {
            return Err(NoticeRepoError::CorruptData);
        }
        let change = audit(
            trip_id,
            actor,
            &changed_at,
            &event_id,
            AuditChange {
                entity: "notice",
                entity_id: notice_id,
                field,
                old_value,
                new_value,
            },
        );
        audit_items.push(
            encode_record(
                trip_pk(trip_id),
                audit_sk(&changed_at, &event_id),
                AUDIT_ENTITY,
                &change,
                1,
            )
            .map_err(super::record_error)?,
        );
    }
    let reservation_actions = reserve_history_append(repo, trip_id, &audit_items)
        .await
        .map_err(history_reservation_error)?;
    if actions.len() + reservation_actions.len() + audit_items.len() > MAX_TRANSACTION_ACTIONS {
        return Err(NoticeRepoError::SafetyLimitExceeded);
    }
    actions.extend(reservation_actions);
    for item in &audit_items {
        actions.push(put_action(repo.create_only_put(item.clone())));
    }
    let result = repo
        .transaction()
        .set_transact_items(Some(actions))
        .send()
        .await;
    if result.is_ok() {
        return Ok(updated);
    }
    let latest_role = repo
        .notice_authorize(trip_id, actor, RequiredNoticeRole::Any)
        .await?;
    let latest = load_notice_state_with_retry(repo, trip_id).await?;
    let latest = latest
        .notices
        .get(notice_id)
        .ok_or(NoticeRepoError::NotFound)?;
    let latest_authorized = latest_role == TripRole::Leader
        || (latest_role == TripRole::Member && latest.notice.created_by == actor.0);
    if !latest_authorized {
        return Err(NoticeRepoError::Forbidden);
    }
    if latest.notice == updated && audit_items_match(repo, trip_id, &audit_items).await? {
        return Ok(latest.notice.clone());
    }
    if result
        .as_ref()
        .err()
        .is_some_and(|error| transaction_condition_failed(error.as_service_error()))
    {
        Err(NoticeRepoError::Conflict)
    } else {
        Err(NoticeRepoError::Unavailable)
    }
}

pub(super) async fn toggle_checklist_item(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    notice_id: &str,
    item_id: &str,
    toggle: ChecklistToggle,
) -> Result<Notice, NoticeRepoError> {
    if !valid_operation_input(&toggle.idempotency_key, &toggle.request_hash)
        || utc(&toggle.recorded_at).is_err()
    {
        return Err(NoticeRepoError::CorruptData);
    }
    repo.notice_authorize(trip_id, actor, RequiredNoticeRole::Any)
        .await?;
    if let Some(result) = replay_operation(
        repo,
        trip_id,
        actor,
        &toggle.idempotency_key,
        &toggle.request_hash,
        &toggle.recorded_at,
    )
    .await?
    {
        let state = load_notice_state_with_retry(repo, trip_id).await?;
        return match result {
            NoticeOperationResult::ChecklistToggled { notice_id, item_id } => {
                Ok(resolve_operation_notice(&state, actor, &notice_id, Some(&item_id))?.clone())
            }
            NoticeOperationResult::Created { .. } => Err(NoticeRepoError::Conflict),
        };
    }
    let state = load_notice_state_with_retry(repo, trip_id).await?;
    let operation_state = load_operation_state_with_retry(repo, trip_id, actor).await?;
    if let Some(result) = replay_operation_from_state(
        &operation_state,
        actor,
        &toggle.idempotency_key,
        &toggle.request_hash,
        &toggle.recorded_at,
    )? {
        return match result {
            NoticeOperationResult::ChecklistToggled { notice_id, item_id } => {
                Ok(resolve_operation_notice(&state, actor, &notice_id, Some(&item_id))?.clone())
            }
            NoticeOperationResult::Created { .. } => Err(NoticeRepoError::Conflict),
        };
    }
    let loaded = state
        .notices
        .get(notice_id)
        .ok_or(NoticeRepoError::NotFound)?;
    if loaded
        .notice
        .audience
        .as_ref()
        .is_some_and(|audience| !audience.contains(&actor.0))
    {
        return Err(NoticeRepoError::Forbidden);
    }
    let mut updated = loaded.notice.clone();
    let item = updated
        .checklist_items
        .iter_mut()
        .find(|item| item.id == item_id)
        .ok_or(NoticeRepoError::NotFound)?;
    match item.mode {
        ChecklistMode::Each => {
            if let Some(index) = item.done_by.iter().position(|user_id| user_id == &actor.0) {
                item.done_by.remove(index);
            } else {
                if item.done_by.len() >= MAX_CHECKLIST_COMPLETIONS {
                    return Err(NoticeRepoError::SafetyLimitExceeded);
                }
                item.done_by.push(actor.0.clone());
            }
        }
        ChecklistMode::Group => {
            if item.done_by.is_empty() {
                item.done_by.push(actor.0.clone());
            } else if item.done_by == [actor.0.clone()] {
                item.done_by.clear();
            } else {
                return Err(NoticeRepoError::Conflict);
            }
        }
    }
    validate_stored_notice(trip_id, &updated).map_err(|_| NoticeRepoError::CorruptData)?;
    let next_revision = loaded
        .revision
        .checked_add(1)
        .ok_or(NoticeRepoError::CorruptData)?;
    let updated_item = encode_notice(&updated, &loaded.created_at, next_revision)?;
    let updated_bytes =
        encoded_item_bytes(&updated_item).ok_or(NoticeRepoError::SafetyLimitExceeded)?;
    let next_notice_bytes = replacement_bytes(&state, loaded.encoded_bytes, updated_bytes)?;
    let meta = state.meta.as_ref().ok_or(NoticeRepoError::CorruptData)?;
    let next_meta = NoticeMetaRecord {
        trip_id: trip_id.to_string(),
        notice_count: meta.value.notice_count,
        notice_bytes: u32::try_from(next_notice_bytes)
            .map_err(|_| NoticeRepoError::SafetyLimitExceeded)?,
    };
    let operation = notice_operation(
        trip_id,
        actor,
        &toggle.idempotency_key,
        &toggle.request_hash,
        NoticeOperationResult::ChecklistToggled {
            notice_id: notice_id.to_string(),
            item_id: item_id.to_string(),
        },
        &toggle.recorded_at,
    )?;
    let operation_item = encode_notice_operation(&operation)?;
    let mut actions = vec![
        condition_action(repo.notice_membership_condition(trip_id, actor, RequiredNoticeRole::Any)),
        put_action(
            repo.entity_snapshot_put(
                encode_notice_meta(
                    &next_meta,
                    meta.revision
                        .checked_add(1)
                        .ok_or(NoticeRepoError::CorruptData)?,
                )?,
                NOTICE_META_ENTITY,
                meta.revision,
                &meta.raw_data,
            ),
        ),
        put_action(repo.entity_snapshot_put(
            updated_item,
            NOTICE_ENTITY,
            loaded.revision,
            &loaded.raw_data,
        )),
    ];
    actions.extend(operation_claim_actions(
        repo,
        &operation_state,
        &operation,
        operation_item,
        &toggle.recorded_at,
    )?);
    if actions.len() > MAX_TRANSACTION_ACTIONS {
        return Err(NoticeRepoError::SafetyLimitExceeded);
    }
    let result = repo
        .transaction()
        .set_transact_items(Some(actions))
        .send()
        .await;
    if result.is_ok() {
        return Ok(updated);
    }
    if let Some(notice) = replay_checklist_toggle(
        repo,
        trip_id,
        actor,
        &toggle.idempotency_key,
        &toggle.request_hash,
        &toggle.recorded_at,
    )
    .await?
    {
        return Ok(notice);
    }
    if result
        .as_ref()
        .err()
        .is_some_and(|error| transaction_condition_failed(error.as_service_error()))
    {
        Err(NoticeRepoError::Conflict)
    } else {
        Err(NoticeRepoError::Unavailable)
    }
}

fn record_change<T: serde::Serialize + PartialEq>(
    changes: &mut Vec<(&'static str, serde_json::Value, serde_json::Value)>,
    field: &'static str,
    old_value: &T,
    new_value: &T,
) -> Result<(), NoticeRepoError> {
    if old_value != new_value {
        changes.push((
            field,
            serde_json::to_value(old_value).map_err(|_| NoticeRepoError::CorruptData)?,
            serde_json::to_value(new_value).map_err(|_| NoticeRepoError::CorruptData)?,
        ));
    }
    Ok(())
}

fn notice_operation(
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
    result: NoticeOperationResult,
    recorded_at: &str,
) -> Result<NoticeOperationRecord, NoticeRepoError> {
    if !valid_operation_input(idempotency_key, request_hash) {
        return Err(NoticeRepoError::CorruptData);
    }
    let recorded = utc(recorded_at)?;
    let expires = recorded
        .checked_add_signed(Duration::hours(24))
        .ok_or(NoticeRepoError::CorruptData)?;
    Ok(NoticeOperationRecord {
        trip_id: trip_id.to_string(),
        key_hash: notice_operation_key_hash(idempotency_key),
        actor_id: actor.0.clone(),
        request_hash: request_hash.to_string(),
        result,
        recorded_at: recorded_at.to_string(),
        expires_at: expires.to_rfc3339_opts(SecondsFormat::AutoSi, true),
    })
}

async fn replay_operation(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
    now: &str,
) -> Result<Option<NoticeOperationResult>, NoticeRepoError> {
    if !valid_operation_input(idempotency_key, request_hash) || utc(now).is_err() {
        return Err(NoticeRepoError::CorruptData);
    }
    let key_hash = notice_operation_key_hash(idempotency_key);
    let partition_key = notice_operation_partition_key(trip_id, &actor.0);
    let Some(item) = repo
        .notice_get(&partition_key, &notice_operation_sk(&key_hash))
        .await?
    else {
        return Ok(None);
    };
    let operation = decode_notice_operation(&item, trip_id, &actor.0)?;
    replay_loaded_operation(&operation, actor, request_hash, now)
}

fn replay_operation_from_state(
    state: &OperationState,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
    now: &str,
) -> Result<Option<NoticeOperationResult>, NoticeRepoError> {
    if !valid_operation_input(idempotency_key, request_hash) || utc(now).is_err() {
        return Err(NoticeRepoError::CorruptData);
    }
    let key_hash = notice_operation_key_hash(idempotency_key);
    state
        .operations
        .get(&key_hash)
        .map_or(Ok(None), |operation| {
            replay_loaded_operation(operation, actor, request_hash, now)
        })
}

fn replay_loaded_operation(
    operation: &Loaded<NoticeOperationRecord>,
    actor: &UserId,
    request_hash: &str,
    now: &str,
) -> Result<Option<NoticeOperationResult>, NoticeRepoError> {
    if operation.value.actor_id != actor.0 {
        return Err(NoticeRepoError::CorruptData);
    }
    if operation_expired(&operation.value, now)? {
        return Ok(None);
    }
    if operation.value.request_hash != request_hash {
        return Err(NoticeRepoError::Conflict);
    }
    Ok(Some(operation.value.result.clone()))
}

fn resolve_operation_notice<'a>(
    state: &'a NoticeState,
    actor: &UserId,
    notice_id: &str,
    item_id: Option<&str>,
) -> Result<&'a Notice, NoticeRepoError> {
    let notice = &state
        .notices
        .get(notice_id)
        .ok_or(NoticeRepoError::CorruptData)?
        .notice;
    if item_id.is_none() && notice.created_by != actor.0 {
        return Err(NoticeRepoError::CorruptData);
    }
    if item_id.is_some_and(|item_id| !notice.checklist_items.iter().any(|item| item.id == item_id))
    {
        return Err(NoticeRepoError::CorruptData);
    }
    Ok(notice)
}

fn operation_claim_actions(
    repo: &DynamoUserRepo,
    state: &OperationState,
    operation: &NoticeOperationRecord,
    operation_item: HashMap<String, AttributeValue>,
    now: &str,
) -> Result<Vec<aws_sdk_dynamodb::types::TransactWriteItem>, NoticeRepoError> {
    if operation_expired(operation, now)? {
        return Err(NoticeRepoError::CorruptData);
    }
    let partition_key = notice_operation_partition_key(&operation.trip_id, &operation.actor_id);
    let existing = state.operations.get(&operation.key_hash);
    if let Some(existing) = existing
        && !operation_expired(&existing.value, now)?
    {
        return Err(NoticeRepoError::Conflict);
    }

    let reclaimed = if existing.is_some() {
        None
    } else if state.operations.len() >= MAX_NOTICE_OPERATIONS_PER_ACTOR {
        let mut expired = Vec::new();
        for candidate in state.operations.values() {
            if operation_expired(&candidate.value, now)? {
                expired.push(candidate);
            }
        }
        Some(
            expired
                .into_iter()
                .min_by(|left, right| {
                    left.value
                        .expires_at
                        .cmp(&right.value.expires_at)
                        .then_with(|| left.value.key_hash.cmp(&right.value.key_hash))
                })
                .ok_or(NoticeRepoError::SafetyLimitExceeded)?,
        )
    } else {
        None
    };

    let next_count = if state.meta.is_none() {
        1
    } else if existing.is_some() || reclaimed.is_some() {
        u32::try_from(state.operations.len()).map_err(|_| NoticeRepoError::CorruptData)?
    } else {
        u32::try_from(state.operations.len() + 1).map_err(|_| NoticeRepoError::CorruptData)?
    };
    let next_meta = NoticeOperationMetaRecord {
        trip_id: operation.trip_id.clone(),
        actor_id: operation.actor_id.clone(),
        operation_count: next_count,
    };
    let meta_put = match &state.meta {
        Some(meta) => repo.entity_snapshot_put(
            encode_notice_operation_meta(
                &next_meta,
                meta.revision
                    .checked_add(1)
                    .ok_or(NoticeRepoError::CorruptData)?,
            )?,
            NOTICE_OPERATION_META_ENTITY,
            meta.revision,
            &meta.raw_data,
        ),
        None => repo.create_only_put(encode_notice_operation_meta(&next_meta, 1)?),
    };
    let mut actions = vec![put_action(meta_put)];
    if let Some(existing) = existing {
        actions.push(put_action(repo.entity_snapshot_put(
            operation_item,
            NOTICE_OPERATION_ENTITY,
            existing.revision,
            &existing.raw_data,
        )));
    } else {
        if let Some(reclaimed) = reclaimed {
            actions.push(delete_action(repo.entity_snapshot_delete(
                &partition_key,
                notice_operation_sk(&reclaimed.value.key_hash),
                NOTICE_OPERATION_ENTITY,
                reclaimed.revision,
                &reclaimed.raw_data,
            )));
        }
        actions.push(put_action(repo.create_only_put(operation_item)));
    }
    Ok(actions)
}

fn operation_expired(
    operation: &NoticeOperationRecord,
    now: &str,
) -> Result<bool, NoticeRepoError> {
    Ok(utc(&operation.expires_at)? <= utc(now)?)
}

fn history_reservation_error(error: ContentHistoryRepoError) -> NoticeRepoError {
    match error {
        ContentHistoryRepoError::Unavailable => NoticeRepoError::Unavailable,
        ContentHistoryRepoError::SafetyLimitExceeded => NoticeRepoError::SafetyLimitExceeded,
        ContentHistoryRepoError::CorruptData
        | ContentHistoryRepoError::NotFound
        | ContentHistoryRepoError::Forbidden
        | ContentHistoryRepoError::Conflict
        | ContentHistoryRepoError::Unsupported => NoticeRepoError::CorruptData,
    }
}

fn valid_operation_input(idempotency_key: &str, request_hash: &str) -> bool {
    !idempotency_key.is_empty()
        && idempotency_key.len() <= 128
        && idempotency_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
        && request_hash.len() == 64
        && request_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_change_metadata(changed_at: &str, change_id: &str) -> Result<(), NoticeRepoError> {
    if changed_at.len() > 64
        || !changed_at.ends_with('Z')
        || utc(changed_at).is_err()
        || change_id.chars().count() > 197
        || !valid_id(change_id)
    {
        return Err(NoticeRepoError::CorruptData);
    }
    Ok(())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && value.chars().count() <= 200
}

async fn audit_items_match(
    repo: &DynamoUserRepo,
    trip_id: &str,
    expected: &[HashMap<String, AttributeValue>],
) -> Result<bool, NoticeRepoError> {
    for expected_item in expected {
        let sort_key = string(expected_item, SK).map_err(super::record_error)?;
        let Some(stored) = repo.notice_get(&trip_pk(trip_id), &sort_key).await? else {
            return Ok(false);
        };
        if stored != *expected_item {
            return Ok(false);
        }
    }
    Ok(true)
}

fn require_current_audience(
    snapshot: &NoticeMembershipSnapshot,
    audience: &[String],
) -> Result<(), NoticeRepoError> {
    if audience
        .iter()
        .any(|user_id| !snapshot.member_ids.contains(user_id))
    {
        return Err(NoticeRepoError::Conflict);
    }
    Ok(())
}

fn replacement_bytes(
    state: &NoticeState,
    previous_bytes: usize,
    next_bytes: usize,
) -> Result<usize, NoticeRepoError> {
    state
        .encoded_bytes
        .checked_sub(previous_bytes)
        .and_then(|bytes| bytes.checked_add(next_bytes))
        .filter(|bytes| *bytes <= MAX_NOTICE_BYTES)
        .ok_or(NoticeRepoError::SafetyLimitExceeded)
}

async fn load_notice_state_with_retry(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<NoticeState, NoticeRepoError> {
    match load_notice_state(repo, trip_id).await {
        Err(NoticeRepoError::CorruptData) => load_notice_state(repo, trip_id).await,
        result => result,
    }
}

async fn load_notice_state(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<NoticeState, NoticeRepoError> {
    let before = load_meta(repo, trip_id).await?;
    let items = repo
        .notice_query(&trip_pk(trip_id), NOTICE_PREFIX, MAX_NOTICES)
        .await?;
    let after = load_meta(repo, trip_id).await?;
    if !same_meta(&before, &after) {
        return Err(NoticeRepoError::CorruptData);
    }
    let Some(meta) = before else {
        return if items.is_empty() {
            Ok(NoticeState {
                meta: None,
                notices: HashMap::new(),
                encoded_bytes: 0,
            })
        } else {
            Err(NoticeRepoError::CorruptData)
        };
    };
    if items.len() != meta.value.notice_count as usize {
        return Err(NoticeRepoError::CorruptData);
    }
    let mut notices = HashMap::with_capacity(items.len());
    let mut encoded_bytes = 0_usize;
    for item in items {
        let loaded = decode_notice(&item, trip_id)?;
        encoded_bytes = encoded_bytes
            .checked_add(loaded.encoded_bytes)
            .filter(|bytes| *bytes <= MAX_NOTICE_BYTES)
            .ok_or(NoticeRepoError::SafetyLimitExceeded)?;
        if notices.insert(loaded.notice.id.clone(), loaded).is_some() {
            return Err(NoticeRepoError::CorruptData);
        }
    }
    if encoded_bytes != meta.value.notice_bytes as usize {
        return Err(NoticeRepoError::CorruptData);
    }
    Ok(NoticeState {
        meta: Some(meta),
        notices,
        encoded_bytes,
    })
}

async fn load_operation_state_with_retry(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<OperationState, NoticeRepoError> {
    match load_operation_state(repo, trip_id, actor).await {
        Err(NoticeRepoError::CorruptData) => load_operation_state(repo, trip_id, actor).await,
        result => result,
    }
}

async fn load_operation_state(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<OperationState, NoticeRepoError> {
    let before = load_operation_meta(repo, trip_id, actor).await?;
    let partition_key = notice_operation_partition_key(trip_id, &actor.0);
    let items = repo
        .notice_query(
            &partition_key,
            NOTICE_OPERATION_PREFIX,
            MAX_NOTICE_OPERATIONS_PER_ACTOR,
        )
        .await?;
    let after = load_operation_meta(repo, trip_id, actor).await?;
    if !same_operation_meta(&before, &after) {
        return Err(NoticeRepoError::CorruptData);
    }
    let Some(meta) = before else {
        return if items.is_empty() {
            Ok(OperationState {
                meta: None,
                operations: HashMap::new(),
            })
        } else {
            Err(NoticeRepoError::CorruptData)
        };
    };
    if items.len() != meta.value.operation_count as usize {
        return Err(NoticeRepoError::CorruptData);
    }
    let mut operations = HashMap::with_capacity(items.len());
    for item in items {
        let loaded = decode_notice_operation(&item, trip_id, &actor.0)?;
        if operations
            .insert(loaded.value.key_hash.clone(), loaded)
            .is_some()
        {
            return Err(NoticeRepoError::CorruptData);
        }
    }
    Ok(OperationState {
        meta: Some(meta),
        operations,
    })
}

async fn load_operation_meta(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Option<Loaded<NoticeOperationMetaRecord>>, NoticeRepoError> {
    repo.notice_get(
        &notice_operation_partition_key(trip_id, &actor.0),
        NOTICE_OPERATION_META_SK,
    )
    .await?
    .map(|item| decode_notice_operation_meta(&item, trip_id, &actor.0))
    .transpose()
}

async fn load_meta(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<Option<Loaded<NoticeMetaRecord>>, NoticeRepoError> {
    repo.notice_get(&trip_pk(trip_id), NOTICE_META_SK)
        .await?
        .map(|item| decode_notice_meta(&item, trip_id))
        .transpose()
}

fn same_meta(
    left: &Option<Loaded<NoticeMetaRecord>>,
    right: &Option<Loaded<NoticeMetaRecord>>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.revision == right.revision && left.raw_data == right.raw_data
        }
        _ => false,
    }
}

fn same_operation_meta(
    left: &Option<Loaded<NoticeOperationMetaRecord>>,
    right: &Option<Loaded<NoticeOperationMetaRecord>>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.revision == right.revision && left.raw_data == right.raw_data
        }
        _ => false,
    }
}
