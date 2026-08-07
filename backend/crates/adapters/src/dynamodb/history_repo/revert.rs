//! Explicitly allowlisted, atomic, stale-safe content reverts.

use std::collections::HashSet;

use aws_sdk_dynamodb::types::TransactWriteItem;
use chrono::DateTime;
use itinera_core::{
    domain::{
        content_history::{Edit, EditEntity, EditStatus},
        notice::NoticeStatus,
        trip::{Booking, Candidate, CandidateStatus, Day, Place, Stop, TripRole, TripStatus},
        user::UserId,
    },
    ports::{content_history::ContentHistoryRepoError, notice::NoticeRepoError},
    services::{
        notices::{retain_audience_completions, validate_stored_notice},
        validation::{
            duration_min as canonical_duration_min,
            exact_bounded_strings as canonical_bounded_strings,
            exact_required_text as canonical_required_text, http_url as canonical_http_url,
            local_time as canonical_local_time, text_len as canonical_text_len,
            time_window as canonical_time_window, validate_booking as canonical_booking,
            validate_place_snapshot as canonical_place,
        },
    },
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::dynamodb::{
    DynamoUserRepo, ENTITY_TYPE, SK,
    ledger_repo::records::{STOP_LINK_ENTITY, decode_stop_link, stop_link_sk},
    notice_repo::{
        access::NoticeMembershipSnapshot,
        operations::load_notice_aggregate_guard,
        records::{
            LoadedNotice, NOTICE_ENTITY, NOTICE_META_ENTITY, NoticeMetaRecord, decode_notice,
            encode_notice, encode_notice_meta, notice_sk,
        },
    },
    primitives::{condition_action, put_action, transaction_condition_failed},
    trip_repo::records::{
        AUDIT_ENTITY, CANDIDATE_ENTITY, DAY_ENTITY, META_SK, PLACE_ENTITY, STOP_ENTITY,
        TRIP_COLLECTION_PAGE_SIZE, TRIP_ENTITY, audit_sk, candidate_sk, encode_record,
        encode_trip_meta, place_sk, plan_prefix, string, trip_pk,
    },
};

use super::{
    access::{
        Loaded, MAX_HISTORY_BYTES, MAX_HISTORY_RECORDS, MAX_REVERT_PLAN_BYTES,
        MAX_REVERT_PLAN_RECORDS, RequiredHistoryRole, decode_loaded, encoded_item_bytes,
    },
    audit::{
        compensating_edit, find_audit_record, reverted_original, valid_edit_id, valid_utc_timestamp,
    },
    record_error,
    reservation::history_slot_item,
};

struct TargetPlan {
    actions: Vec<TransactWriteItem>,
    compensation_values: Option<(Value, Value)>,
    required_role: RequiredHistoryRole,
}

pub(super) async fn revert_edit(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    edit_id: &str,
    reverted_at: &str,
    compensating_edit_id: &str,
) -> Result<(), ContentHistoryRepoError> {
    let actor_role = repo
        .history_authorize(trip_id, actor, RequiredHistoryRole::Editor)
        .await?;
    if !valid_edit_id(edit_id)
        || !valid_edit_id(compensating_edit_id)
        || edit_id == compensating_edit_id
        || !valid_utc_timestamp(reverted_at)
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }

    let lookup = find_audit_record(repo, trip_id, edit_id).await?;
    let original = lookup.record.ok_or(ContentHistoryRepoError::NotFound)?;
    validate_revert_semantics(trip_id, &original.value)?;
    match original.value.status {
        // Repeating the same server-owned command is a successful no-op. The
        // first transaction's actor and compensation remain authoritative.
        EditStatus::Reverted => {
            if original.value.entity == EditEntity::Notice {
                let (_, required_role) =
                    load_notice_target(repo, trip_id, actor, actor_role, &original.value).await?;
                repo.history_authorize(trip_id, actor, required_role)
                    .await?;
            }
            return Ok(());
        }
        EditStatus::Applied => {}
        EditStatus::PendingReview | EditStatus::Rejected => {
            return Err(ContentHistoryRepoError::Conflict);
        }
    }
    let reverted_time = DateTime::parse_from_rfc3339(reverted_at)
        .map_err(|_| ContentHistoryRepoError::CorruptData)?;
    let original_time = DateTime::parse_from_rfc3339(&original.value.created_at)
        .map_err(|_| ContentHistoryRepoError::CorruptData)?;
    if reverted_time < original_time || lookup.edit_ids.contains(compensating_edit_id) {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    if lookup.record_count >= MAX_HISTORY_RECORDS {
        return Err(ContentHistoryRepoError::SafetyLimitExceeded);
    }
    let target = build_target_plan(repo, trip_id, actor, actor_role, &original.value).await?;
    let reverted = reverted_original(&original.value, actor, reverted_at, compensating_edit_id);
    let mut compensation =
        compensating_edit(&original.value, actor, reverted_at, compensating_edit_id);
    if let Some((before, after)) = target.compensation_values.clone() {
        compensation.old_value = before;
        compensation.new_value = after;
    }
    let pk = trip_pk(trip_id);
    let reverted_item = encode_record(
        pk.clone(),
        original.sort_key.clone(),
        AUDIT_ENTITY,
        &reverted,
        next_revision(original.revision)?,
    )
    .map_err(record_error)?;
    let compensation_item = encode_record(
        pk,
        audit_sk(reverted_at, compensating_edit_id),
        AUDIT_ENTITY,
        &compensation,
        1,
    )
    .map_err(record_error)?;
    let reserved_count = lookup
        .record_count
        .checked_add(1)
        .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)?;
    let reservation_item = history_slot_item(trip_id, reserved_count)?;
    let reverted_bytes = encoded_item_bytes(&reverted_item)?;
    let compensation_bytes = encoded_item_bytes(&compensation_item)?;
    let projected_bytes = lookup
        .encoded_bytes
        .checked_sub(original.encoded_bytes)
        .and_then(|bytes| bytes.checked_add(reverted_bytes))
        .and_then(|bytes| bytes.checked_add(compensation_bytes))
        .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)?;
    if projected_bytes > MAX_HISTORY_BYTES {
        return Err(ContentHistoryRepoError::SafetyLimitExceeded);
    }

    let mut transaction =
        repo.transaction()
            .transact_items(condition_action(repo.history_membership_condition(
                trip_id,
                actor,
                target.required_role,
            )));
    for action in target.actions {
        transaction = transaction.transact_items(action);
    }
    transaction = transaction
        .transact_items(put_action(repo.entity_snapshot_put(
            reverted_item,
            AUDIT_ENTITY,
            original.revision,
            &original.raw_data,
        )))
        .transact_items(put_action(repo.create_only_put(compensation_item)))
        // Reverts that observed the same history length compete for the same
        // permanent create-only slot. Only one may append; a loser reloads the
        // bounded history before a caller retries, so distinct concurrent
        // reverts cannot race past the row or byte ceiling.
        .transact_items(put_action(repo.create_only_put(reservation_item)));

    let result = transaction.send().await;
    let Err(error) = result else {
        return Ok(());
    };

    // A cancellation can mean role revocation, a stale entity, a concurrent
    // revert, or an extremely unlikely generated-id collision. Re-read the
    // two authoritative records rather than translating all of those into one
    // misleading outcome. This also recovers an ambiguous SDK response after
    // DynamoDB committed the transaction.
    repo.history_authorize(trip_id, actor, target.required_role)
        .await?;
    if find_audit_record(repo, trip_id, edit_id)
        .await?
        .record
        .is_some_and(|stored| stored.value.status == EditStatus::Reverted)
    {
        return Ok(());
    }
    if transaction_condition_failed(error.as_service_error()) {
        Err(ContentHistoryRepoError::Conflict)
    } else {
        Err(ContentHistoryRepoError::Unavailable)
    }
}

fn validate_revert_semantics(trip_id: &str, edit: &Edit) -> Result<(), ContentHistoryRepoError> {
    if edit.old_value == edit.new_value {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    match (edit.entity, edit.field.as_str()) {
        (EditEntity::Trip, "status") => {
            if edit.entity_id != trip_id {
                return Err(ContentHistoryRepoError::CorruptData);
            }
            let _: TripStatus = parse_exact(&edit.old_value)?;
            let _: TripStatus = parse_exact(&edit.new_value)?;
        }
        (EditEntity::Candidate, "status") => {
            let old: CandidateStatus = parse_exact(&edit.old_value)?;
            let new: CandidateStatus = parse_exact(&edit.new_value)?;
            if old == CandidateStatus::InPlan || new == CandidateStatus::InPlan {
                return Err(ContentHistoryRepoError::Unsupported);
            }
        }
        (EditEntity::Candidate, "pitch") => {
            validate_required_text(&parse_exact::<String>(&edit.old_value)?, 2_000)?;
            validate_required_text(&parse_exact::<String>(&edit.new_value)?, 2_000)?;
        }
        (EditEntity::Candidate, "tags") => {
            validate_tags(&parse_exact::<Vec<String>>(&edit.old_value)?)?;
            validate_tags(&parse_exact::<Vec<String>>(&edit.new_value)?)?;
        }
        (EditEntity::Candidate, "place") => {
            let old: Place = parse_exact(&edit.old_value)?;
            let new: Place = parse_exact(&edit.new_value)?;
            validate_place(&old)?;
            validate_place(&new)?;
            if old.id == new.id {
                return Err(ContentHistoryRepoError::CorruptData);
            }
        }
        (EditEntity::Day, "windowStart" | "windowEnd") => {
            validate_local_time(&parse_exact::<String>(&edit.old_value)?)?;
            validate_local_time(&parse_exact::<String>(&edit.new_value)?)?;
        }
        (EditEntity::Day, "cityHint") => {
            validate_required_text(&parse_exact::<String>(&edit.old_value)?, 120)?;
            validate_required_text(&parse_exact::<String>(&edit.new_value)?, 120)?;
        }
        (EditEntity::Stop, "plannedArrival") => {
            validate_local_time(&parse_exact::<String>(&edit.old_value)?)?;
            validate_local_time(&parse_exact::<String>(&edit.new_value)?)?;
        }
        (EditEntity::Stop, "durationMin") => {
            validate_duration(parse_exact::<u32>(&edit.old_value)?)?;
            validate_duration(parse_exact::<u32>(&edit.new_value)?)?;
        }
        (EditEntity::Stop, "notes") => {
            validate_text_len(&parse_exact::<String>(&edit.old_value)?, 10_000)?;
            validate_text_len(&parse_exact::<String>(&edit.new_value)?, 10_000)?;
        }
        (EditEntity::Stop, "booking") => {
            let old: Option<Booking> = parse_exact(&edit.old_value)?;
            let new: Option<Booking> = parse_exact(&edit.new_value)?;
            validate_booking(old.as_ref())?;
            validate_booking(new.as_ref())?;
            if booking_ledger_entry_id(old.as_ref()) != booking_ledger_entry_id(new.as_ref()) {
                return Err(ContentHistoryRepoError::CorruptData);
            }
        }
        (EditEntity::Notice, "title") => {
            validate_required_text(
                &parse_exact::<String>(&edit.old_value)?,
                itinera_core::services::notices::MAX_NOTICE_TITLE_CHARS,
            )?;
            validate_required_text(
                &parse_exact::<String>(&edit.new_value)?,
                itinera_core::services::notices::MAX_NOTICE_TITLE_CHARS,
            )?;
        }
        (EditEntity::Notice, "body") => {
            validate_required_text(
                &parse_exact::<String>(&edit.old_value)?,
                itinera_core::services::notices::MAX_NOTICE_BODY_CHARS,
            )?;
            validate_required_text(
                &parse_exact::<String>(&edit.new_value)?,
                itinera_core::services::notices::MAX_NOTICE_BODY_CHARS,
            )?;
        }
        (EditEntity::Notice, "pinned") => {
            let _: bool = parse_exact(&edit.old_value)?;
            let _: bool = parse_exact(&edit.new_value)?;
        }
        (EditEntity::Notice, "sourceUrl") => {
            for value in [&edit.old_value, &edit.new_value] {
                let parsed: Option<String> = parse_exact(value)?;
                if canonical_http_url(parsed.clone())
                    .map_err(|_| ContentHistoryRepoError::CorruptData)?
                    != parsed
                {
                    return Err(ContentHistoryRepoError::CorruptData);
                }
            }
        }
        (EditEntity::Notice, "status") => {
            let _: NoticeStatus = parse_exact(&edit.old_value)?;
            let _: NoticeStatus = parse_exact(&edit.new_value)?;
        }
        (EditEntity::Notice, "audience") => {
            validate_notice_audience(&parse_exact::<Option<Vec<String>>>(&edit.old_value)?)?;
            validate_notice_audience(&parse_exact::<Option<Vec<String>>>(&edit.new_value)?)?;
        }
        _ => return Err(ContentHistoryRepoError::Unsupported),
    }
    Ok(())
}

fn validate_notice_audience(audience: &Option<Vec<String>>) -> Result<(), ContentHistoryRepoError> {
    let Some(audience) = audience else {
        return Ok(());
    };
    if audience.is_empty() || audience.len() > itinera_core::services::notices::MAX_NOTICE_AUDIENCE
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let mut unique = HashSet::new();
    if audience.iter().any(|user_id| {
        !valid_edit_id(user_id) || user_id.trim() != user_id || !unique.insert(user_id.as_str())
    }) {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    Ok(())
}

async fn build_target_plan(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    actor_role: TripRole,
    edit: &Edit,
) -> Result<TargetPlan, ContentHistoryRepoError> {
    match edit.entity {
        EditEntity::Trip if edit.field == "status" => revert_trip_status(repo, trip_id, edit).await,
        EditEntity::Candidate => revert_candidate(repo, trip_id, edit).await,
        EditEntity::Day => revert_day(repo, trip_id, edit).await,
        EditEntity::Stop => revert_stop(repo, trip_id, edit).await,
        EditEntity::Notice => revert_notice(repo, trip_id, actor, actor_role, edit).await,
        EditEntity::Trip => Err(ContentHistoryRepoError::Unsupported),
    }
}

async fn revert_trip_status(
    repo: &DynamoUserRepo,
    trip_id: &str,
    edit: &Edit,
) -> Result<TargetPlan, ContentHistoryRepoError> {
    if edit.entity_id != trip_id {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let old: TripStatus = parse_exact(&edit.old_value)?;
    let new: TripStatus = parse_exact(&edit.new_value)?;
    let mut meta = repo.history_trip_meta(trip_id).await?;
    if meta.value.status != new {
        return Err(ContentHistoryRepoError::Conflict);
    }
    meta.value.status = old;
    let item =
        encode_trip_meta(&meta.value, next_revision(meta.revision)?).map_err(record_error)?;
    Ok(TargetPlan {
        actions: vec![put_action(repo.entity_snapshot_put(
            item,
            TRIP_ENTITY,
            meta.revision,
            &meta.raw_data,
        ))],
        compensation_values: None,
        required_role: RequiredHistoryRole::Editor,
    })
}

async fn revert_candidate(
    repo: &DynamoUserRepo,
    trip_id: &str,
    edit: &Edit,
) -> Result<TargetPlan, ContentHistoryRepoError> {
    let pk = trip_pk(trip_id);
    let sk = candidate_sk(&edit.entity_id);
    let item = repo
        .history_get(&pk, &sk)
        .await?
        .ok_or(ContentHistoryRepoError::Conflict)?;
    let mut candidate: Loaded<Candidate> = decode_loaded(&item, &pk, &sk, CANDIDATE_ENTITY)?;
    if candidate.value.id != edit.entity_id || candidate.value.trip_id != trip_id {
        return Err(ContentHistoryRepoError::CorruptData);
    }

    let mut guards = Vec::new();
    match edit.field.as_str() {
        "status" => {
            let old: CandidateStatus = parse_exact(&edit.old_value)?;
            let new: CandidateStatus = parse_exact(&edit.new_value)?;
            if old == CandidateStatus::InPlan || new == CandidateStatus::InPlan {
                return Err(ContentHistoryRepoError::Unsupported);
            }
            if candidate.value.status != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            candidate.value.status = old;
        }
        "pitch" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_required_text(&old, 2_000)?;
            validate_required_text(&new, 2_000)?;
            if candidate.value.pitch != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            candidate.value.pitch = old;
        }
        "tags" => {
            let old: Vec<String> = parse_exact(&edit.old_value)?;
            let new: Vec<String> = parse_exact(&edit.new_value)?;
            validate_tags(&old)?;
            validate_tags(&new)?;
            if candidate.value.tags != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            candidate.value.tags = old;
        }
        "place" => {
            let old: Place = parse_exact(&edit.old_value)?;
            let new: Place = parse_exact(&edit.new_value)?;
            validate_place(&old)?;
            validate_place(&new)?;
            if old.id == new.id || candidate.value.place_id != new.id {
                return Err(ContentHistoryRepoError::CorruptData);
            }
            let current = load_place(repo, trip_id, &new.id).await?;
            let previous = load_place(repo, trip_id, &old.id).await?;
            if current.value != new || previous.value != old {
                return Err(ContentHistoryRepoError::CorruptData);
            }
            guards.push(condition_action(repo.entity_revision_data_condition(
                trip_pk(trip_id),
                current.sort_key,
                PLACE_ENTITY,
                current.revision,
                &current.raw_data,
            )));
            guards.push(condition_action(repo.entity_revision_data_condition(
                trip_pk(trip_id),
                previous.sort_key,
                PLACE_ENTITY,
                previous.revision,
                &previous.raw_data,
            )));
            candidate.value.place_id = old.id;
        }
        _ => return Err(ContentHistoryRepoError::Unsupported),
    }

    let encoded = encode_record(
        pk,
        candidate.sort_key,
        CANDIDATE_ENTITY,
        &candidate.value,
        next_revision(candidate.revision)?,
    )
    .map_err(record_error)?;
    let mut actions = vec![put_action(repo.entity_snapshot_put(
        encoded,
        CANDIDATE_ENTITY,
        candidate.revision,
        &candidate.raw_data,
    ))];
    actions.extend(guards);
    Ok(TargetPlan {
        actions,
        compensation_values: None,
        required_role: RequiredHistoryRole::Editor,
    })
}

async fn load_place(
    repo: &DynamoUserRepo,
    trip_id: &str,
    place_id: &str,
) -> Result<Loaded<Place>, ContentHistoryRepoError> {
    let pk = trip_pk(trip_id);
    let sk = place_sk(place_id);
    let item = repo
        .history_get(&pk, &sk)
        .await?
        .ok_or(ContentHistoryRepoError::CorruptData)?;
    let place = decode_loaded::<Place>(&item, &pk, &sk, PLACE_ENTITY)?;
    if place.value.id != place_id {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    Ok(place)
}

async fn load_notice_target(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    actor_role: TripRole,
    edit: &Edit,
) -> Result<(LoadedNotice, RequiredHistoryRole), ContentHistoryRepoError> {
    if !matches!(
        edit.field.as_str(),
        "title" | "body" | "pinned" | "sourceUrl" | "status" | "audience"
    ) {
        return Err(ContentHistoryRepoError::Unsupported);
    }
    let item = repo
        .history_get(&trip_pk(trip_id), &notice_sk(&edit.entity_id))
        .await?
        .ok_or(ContentHistoryRepoError::Conflict)?;
    let loaded = decode_notice(&item, trip_id).map_err(notice_record_error)?;
    let required_role = if loaded.notice.created_by == actor.0 {
        RequiredHistoryRole::Editor
    } else if actor_role == TripRole::Leader {
        RequiredHistoryRole::Leader
    } else {
        return Err(ContentHistoryRepoError::Forbidden);
    };
    Ok((loaded, required_role))
}

async fn revert_notice(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    actor_role: TripRole,
    edit: &Edit,
) -> Result<TargetPlan, ContentHistoryRepoError> {
    let (mut loaded, required_role) =
        load_notice_target(repo, trip_id, actor, actor_role, edit).await?;
    let membership_snapshot = if edit.field == "audience" {
        Some(
            repo.notice_membership_snapshot(trip_id)
                .await
                .map_err(notice_record_error)?,
        )
    } else {
        None
    };
    match edit.field.as_str() {
        "title" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            if loaded.notice.title != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            loaded.notice.title = old;
        }
        "body" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            if loaded.notice.body != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            loaded.notice.body = old;
        }
        "pinned" => {
            let old: bool = parse_exact(&edit.old_value)?;
            let new: bool = parse_exact(&edit.new_value)?;
            if loaded.notice.pinned != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            loaded.notice.pinned = old;
        }
        "sourceUrl" => {
            let old: Option<String> = parse_exact(&edit.old_value)?;
            let new: Option<String> = parse_exact(&edit.new_value)?;
            if loaded.notice.source_url != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            loaded.notice.source_url = old;
        }
        "status" => {
            let old: NoticeStatus = parse_exact(&edit.old_value)?;
            let new: NoticeStatus = parse_exact(&edit.new_value)?;
            if loaded.notice.status != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            loaded.notice.status = old;
        }
        "audience" => {
            let old: Option<Vec<String>> = parse_exact(&edit.old_value)?;
            let new: Option<Vec<String>> = parse_exact(&edit.new_value)?;
            if loaded.notice.audience != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            let snapshot = membership_snapshot
                .as_ref()
                .ok_or(ContentHistoryRepoError::CorruptData)?;
            require_current_notice_audience(snapshot, old.as_deref().unwrap_or_default())?;
            loaded.notice.audience = old;
            retain_audience_completions(&mut loaded.notice, &snapshot.member_ids);
        }
        _ => return Err(ContentHistoryRepoError::Unsupported),
    }
    validate_stored_notice(trip_id, &loaded.notice).map_err(|_| {
        if edit.field == "audience" {
            ContentHistoryRepoError::Conflict
        } else {
            ContentHistoryRepoError::CorruptData
        }
    })?;

    let next_notice_revision = next_revision(loaded.revision)?;
    let notice_item = encode_notice(&loaded.notice, &loaded.created_at, next_notice_revision)
        .map_err(notice_record_error)?;
    let next_item_bytes = encoded_item_bytes(&notice_item)?;
    let guard = load_notice_aggregate_guard(repo, trip_id)
        .await
        .map_err(notice_record_error)?;
    let next_aggregate_bytes = guard
        .encoded_bytes
        .checked_sub(loaded.encoded_bytes)
        .and_then(|bytes| bytes.checked_add(next_item_bytes))
        .filter(|bytes| *bytes <= itinera_core::services::notices::MAX_NOTICE_RESPONSE_BYTES)
        .ok_or(ContentHistoryRepoError::SafetyLimitExceeded)?;
    let next_meta = NoticeMetaRecord {
        trip_id: trip_id.to_string(),
        notice_count: guard.meta.value.notice_count,
        notice_bytes: u32::try_from(next_aggregate_bytes)
            .map_err(|_| ContentHistoryRepoError::SafetyLimitExceeded)?,
    };
    let mut actions = vec![
        put_action(repo.entity_snapshot_put(
            notice_item,
            NOTICE_ENTITY,
            loaded.revision,
            &loaded.raw_data,
        )),
        put_action(
            repo.entity_snapshot_put(
                encode_notice_meta(&next_meta, next_revision(guard.meta.revision)?)
                    .map_err(notice_record_error)?,
                NOTICE_META_ENTITY,
                guard.meta.revision,
                &guard.meta.raw_data,
            ),
        ),
    ];
    if let Some(snapshot) = &membership_snapshot {
        actions.push(condition_action(
            repo.notice_membership_snapshot_condition(trip_id, snapshot),
        ));
    }
    Ok(TargetPlan {
        actions,
        compensation_values: None,
        required_role,
    })
}

fn require_current_notice_audience(
    snapshot: &NoticeMembershipSnapshot,
    audience: &[String],
) -> Result<(), ContentHistoryRepoError> {
    if audience
        .iter()
        .any(|user_id| !snapshot.member_ids.contains(user_id))
    {
        return Err(ContentHistoryRepoError::Conflict);
    }
    Ok(())
}

fn notice_record_error(error: NoticeRepoError) -> ContentHistoryRepoError {
    match error {
        NoticeRepoError::Unavailable => ContentHistoryRepoError::Unavailable,
        NoticeRepoError::Forbidden => ContentHistoryRepoError::Forbidden,
        NoticeRepoError::NotFound => ContentHistoryRepoError::NotFound,
        NoticeRepoError::Conflict => ContentHistoryRepoError::Conflict,
        NoticeRepoError::SafetyLimitExceeded => ContentHistoryRepoError::SafetyLimitExceeded,
        NoticeRepoError::CorruptData => ContentHistoryRepoError::CorruptData,
    }
}

async fn revert_day(
    repo: &DynamoUserRepo,
    trip_id: &str,
    edit: &Edit,
) -> Result<TargetPlan, ContentHistoryRepoError> {
    let mut meta = repo.history_trip_meta(trip_id).await?;
    let version = meta
        .value
        .current_plan_version
        .ok_or(ContentHistoryRepoError::Conflict)?;
    let plan_id = meta
        .value
        .current_plan_id
        .clone()
        .ok_or(ContentHistoryRepoError::CorruptData)?;
    let pk = trip_pk(trip_id);
    let mut days = Vec::new();
    let mut day_ids = HashSet::new();
    let mut nested_stop_day_ids = Vec::new();
    let mut target_index = None;
    for item in repo
        .history_query(
            &pk,
            &format!("{}#DAY#", plan_prefix(version)),
            TRIP_COLLECTION_PAGE_SIZE,
            false,
            MAX_REVERT_PLAN_RECORDS,
            MAX_REVERT_PLAN_BYTES,
        )
        .await?
    {
        let sk = string(&item, SK).map_err(record_error)?;
        match string(&item, ENTITY_TYPE).map_err(record_error)?.as_str() {
            DAY_ENTITY => {
                let day = decode_loaded::<Day>(&item, &pk, &sk, DAY_ENTITY)?;
                if day.value.plan_id != plan_id || !day_ids.insert(day.value.id.clone()) {
                    return Err(ContentHistoryRepoError::CorruptData);
                }
                if day.value.id == edit.entity_id {
                    target_index = Some(days.len());
                }
                days.push(day);
            }
            STOP_ENTITY => {
                // Stop keys are nested below the DAY prefix. Validate their
                // envelope rather than mistaking them for day rows, then keep
                // them out of the city/window aggregate.
                let stop = decode_loaded::<Stop>(&item, &pk, &sk, STOP_ENTITY)?;
                if stop.value.id.is_empty() || stop.value.day_id.is_empty() {
                    return Err(ContentHistoryRepoError::CorruptData);
                }
                nested_stop_day_ids.push(stop.value.day_id);
            }
            _ => {
                return Err(ContentHistoryRepoError::CorruptData);
            }
        }
    }
    if nested_stop_day_ids
        .iter()
        .any(|day_id| !day_ids.contains(day_id))
    {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let index = target_index.ok_or(ContentHistoryRepoError::Conflict)?;
    let target = &mut days[index];
    match edit.field.as_str() {
        "windowStart" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_local_time(&old)?;
            validate_local_time(&new)?;
            if target.value.window_start != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            target.value.window_start = old;
        }
        "windowEnd" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_local_time(&old)?;
            validate_local_time(&new)?;
            if target.value.window_end != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            target.value.window_end = old;
        }
        "cityHint" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_required_text(&old, 120)?;
            validate_required_text(&new, 120)?;
            if target.value.city_hint != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            target.value.city_hint = old;
        }
        _ => return Err(ContentHistoryRepoError::Unsupported),
    }
    validate_local_time(&target.value.window_start)?;
    validate_local_time(&target.value.window_end)?;
    if canonical_time_window(&target.value.window_start, &target.value.window_end).is_err() {
        return Err(ContentHistoryRepoError::Conflict);
    }

    let day_item = encode_record(
        pk.clone(),
        target.sort_key.clone(),
        DAY_ENTITY,
        &target.value,
        next_revision(target.revision)?,
    )
    .map_err(record_error)?;
    let mut actions = vec![put_action(repo.entity_snapshot_put(
        day_item,
        DAY_ENTITY,
        target.revision,
        &target.raw_data,
    ))];
    if edit.field == "cityHint" {
        days.sort_by(|left, right| left.value.date.cmp(&right.value.date));
        let mut seen = HashSet::new();
        meta.value.cities = days
            .into_iter()
            .map(|day| day.value.city_hint)
            .filter(|city| seen.insert(city.clone()))
            .collect();
        let meta_item =
            encode_trip_meta(&meta.value, next_revision(meta.revision)?).map_err(record_error)?;
        actions.push(put_action(repo.entity_snapshot_put(
            meta_item,
            TRIP_ENTITY,
            meta.revision,
            &meta.raw_data,
        )));
    } else {
        actions.push(condition_action(repo.entity_revision_data_condition(
            pk,
            META_SK,
            TRIP_ENTITY,
            meta.revision,
            &meta.raw_data,
        )));
    }
    Ok(TargetPlan {
        actions,
        compensation_values: None,
        required_role: RequiredHistoryRole::Editor,
    })
}

async fn revert_stop(
    repo: &DynamoUserRepo,
    trip_id: &str,
    edit: &Edit,
) -> Result<TargetPlan, ContentHistoryRepoError> {
    let meta = repo.history_trip_meta(trip_id).await?;
    let version = meta
        .value
        .current_plan_version
        .ok_or(ContentHistoryRepoError::Conflict)?;
    let plan_id = meta
        .value
        .current_plan_id
        .as_ref()
        .ok_or(ContentHistoryRepoError::CorruptData)?;
    let pk = trip_pk(trip_id);
    let items = repo
        .history_query(
            &pk,
            &format!("{}#", plan_prefix(version)),
            TRIP_COLLECTION_PAGE_SIZE,
            false,
            MAX_REVERT_PLAN_RECORDS,
            MAX_REVERT_PLAN_BYTES,
        )
        .await?;
    let mut day_ids = HashSet::new();
    let mut target = None;
    for item in items {
        let sk = string(&item, SK).map_err(record_error)?;
        match string(&item, ENTITY_TYPE).map_err(record_error)?.as_str() {
            DAY_ENTITY => {
                let day = decode_loaded::<Day>(&item, &pk, &sk, DAY_ENTITY)?;
                if &day.value.plan_id != plan_id || !day_ids.insert(day.value.id) {
                    return Err(ContentHistoryRepoError::CorruptData);
                }
            }
            STOP_ENTITY => {
                let stop = decode_loaded::<Stop>(&item, &pk, &sk, STOP_ENTITY)?;
                if stop.value.id == edit.entity_id {
                    if target.is_some() {
                        return Err(ContentHistoryRepoError::CorruptData);
                    }
                    target = Some(stop);
                }
            }
            "PLAN" => {}
            _ => return Err(ContentHistoryRepoError::CorruptData),
        }
    }
    let mut stop = target.ok_or(ContentHistoryRepoError::Conflict)?;
    if !day_ids.contains(&stop.value.day_id) {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let mut compensation_values = None;
    match edit.field.as_str() {
        "plannedArrival" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_local_time(&old)?;
            validate_local_time(&new)?;
            if stop.value.planned_arrival != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            stop.value.planned_arrival = old;
        }
        "durationMin" => {
            let old: u32 = parse_exact(&edit.old_value)?;
            let new: u32 = parse_exact(&edit.new_value)?;
            validate_duration(old)?;
            validate_duration(new)?;
            if stop.value.duration_min != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            stop.value.duration_min = old;
        }
        "notes" => {
            let old: String = parse_exact(&edit.old_value)?;
            let new: String = parse_exact(&edit.new_value)?;
            validate_text_len(&old, 10_000)?;
            validate_text_len(&new, 10_000)?;
            if stop.value.notes != new {
                return Err(ContentHistoryRepoError::Conflict);
            }
            stop.value.notes = old;
        }
        "booking" => {
            let (booking, values) =
                reverted_booking(&stop.value.booking, &edit.old_value, &edit.new_value)?;
            stop.value.booking = booking;
            compensation_values = Some(values);
        }
        _ => return Err(ContentHistoryRepoError::Unsupported),
    }
    validate_local_time(&stop.value.planned_arrival)?;
    validate_duration(stop.value.duration_min)?;
    validate_text_len(&stop.value.notes, 10_000)?;
    validate_booking(stop.value.booking.as_ref())?;

    let link_claim = repo
        .history_get(&pk, &stop_link_sk(&stop.value.id))
        .await?
        .map(|item| {
            decode_stop_link(&item, trip_id).map_err(|_| ContentHistoryRepoError::CorruptData)
        })
        .transpose()?;
    match (
        booking_ledger_entry_id(stop.value.booking.as_ref()),
        link_claim.as_ref(),
    ) {
        (Some(expense_id), Some(claim)) if claim.value.expense_id == expense_id => {}
        (None, None) => {}
        _ => return Err(ContentHistoryRepoError::CorruptData),
    }

    let stop_item = encode_record(
        pk.clone(),
        stop.sort_key,
        STOP_ENTITY,
        &stop.value,
        next_revision(stop.revision)?,
    )
    .map_err(record_error)?;
    let mut actions = vec![
        put_action(repo.entity_snapshot_put(stop_item, STOP_ENTITY, stop.revision, &stop.raw_data)),
        condition_action(repo.entity_revision_data_condition(
            pk,
            META_SK,
            TRIP_ENTITY,
            meta.revision,
            &meta.raw_data,
        )),
    ];
    if let Some(claim) = link_claim {
        actions.push(condition_action(repo.entity_revision_data_condition(
            trip_pk(trip_id),
            stop_link_sk(&stop.value.id),
            STOP_LINK_ENTITY,
            claim.revision,
            &claim.raw_data,
        )));
    }
    Ok(TargetPlan {
        actions,
        compensation_values,
        required_role: RequiredHistoryRole::Editor,
    })
}

fn booking_ledger_entry_id(booking: Option<&Booking>) -> Option<&str> {
    booking.and_then(|booking| booking.ledger_entry_id.as_deref())
}

fn clear_booking_ledger_entry_id(booking: &mut Option<Booking>) {
    if let Some(booking) = booking {
        booking.ledger_entry_id = None;
    }
}

pub(super) fn reverted_booking(
    current: &Option<Booking>,
    old_value: &Value,
    new_value: &Value,
) -> Result<(Option<Booking>, (Value, Value)), ContentHistoryRepoError> {
    let actual_before =
        serde_json::to_value(current).map_err(|_| ContentHistoryRepoError::CorruptData)?;
    let mut old: Option<Booking> = parse_exact(old_value)?;
    let mut new: Option<Booking> = parse_exact(new_value)?;
    validate_booking(old.as_ref())?;
    validate_booking(new.as_ref())?;
    if booking_ledger_entry_id(old.as_ref()) != booking_ledger_entry_id(new.as_ref()) {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    let current_link = booking_ledger_entry_id(current.as_ref()).map(str::to_string);
    clear_booking_ledger_entry_id(&mut new);
    let mut comparable_current = current.clone();
    clear_booking_ledger_entry_id(&mut comparable_current);
    if comparable_current != new {
        return Err(ContentHistoryRepoError::Conflict);
    }
    if old.is_none() && current_link.is_some() {
        return Err(ContentHistoryRepoError::Conflict);
    }
    if let Some(booking) = old.as_mut() {
        booking.ledger_entry_id = current_link;
    }
    let actual_after =
        serde_json::to_value(&old).map_err(|_| ContentHistoryRepoError::CorruptData)?;
    Ok((old, (actual_before, actual_after)))
}

fn parse_exact<T>(value: &Value) -> Result<T, ContentHistoryRepoError>
where
    T: DeserializeOwned + Serialize,
{
    let parsed = serde_json::from_value::<T>(value.clone())
        .map_err(|_| ContentHistoryRepoError::CorruptData)?;
    let canonical =
        serde_json::to_value(&parsed).map_err(|_| ContentHistoryRepoError::CorruptData)?;
    if canonical != *value {
        return Err(ContentHistoryRepoError::CorruptData);
    }
    Ok(parsed)
}

pub(super) fn validate_required_text(
    value: &str,
    max: usize,
) -> Result<(), ContentHistoryRepoError> {
    canonical_required_text(value, "stored text must be normalized", max)
        .map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_text_len(value: &str, max: usize) -> Result<(), ContentHistoryRepoError> {
    canonical_text_len(value, max).map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_tags(tags: &[String]) -> Result<(), ContentHistoryRepoError> {
    canonical_bounded_strings(tags, 20, 60).map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_local_time(value: &str) -> Result<(), ContentHistoryRepoError> {
    canonical_local_time(value).map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_duration(value: u32) -> Result<(), ContentHistoryRepoError> {
    canonical_duration_min(value).map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_booking(booking: Option<&Booking>) -> Result<(), ContentHistoryRepoError> {
    canonical_booking(booking).map_err(|_| ContentHistoryRepoError::CorruptData)
}

pub(super) fn validate_place(place: &Place) -> Result<(), ContentHistoryRepoError> {
    canonical_place(place).map_err(|_| ContentHistoryRepoError::CorruptData)
}

fn next_revision(current: u64) -> Result<u64, ContentHistoryRepoError> {
    current
        .checked_add(1)
        .ok_or(ContentHistoryRepoError::CorruptData)
}
