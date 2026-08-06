use std::{collections::HashMap, fmt::Write};

use aws_sdk_dynamodb::types::AttributeValue;
use chrono::DateTime;
use itinera_core::{
    domain::discussion::{Comment, DiscussionThread, ThreadAnchor},
    ports::discussion::DiscussionRepoError,
    services::discussions::{
        validate_stored_comment, validate_stored_thread, validate_thread_anchor,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dynamodb::{
    SK,
    trip_repo::records::{decode_record, encode_record, string, trip_pk},
};

use super::record_error;

pub(super) const DISCUSSION_META_ENTITY: &str = "DISCUSSION_META";
pub(super) const THREAD_ENTITY: &str = "DISCUSSION_THREAD";
pub(super) const THREAD_ANCHOR_ENTITY: &str = "DISCUSSION_THREAD_ANCHOR";
pub(super) const COMMENT_ENTITY: &str = "DISCUSSION_COMMENT";
pub(super) const DISCUSSION_META_SK: &str = "DISCUSSION#META";
pub(super) const THREAD_PREFIX: &str = "THREAD_META#";
pub(super) const THREAD_ANCHOR_PREFIX: &str = "THREAD_ANCHOR#";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DiscussionMetaRecord {
    pub(super) trip_id: String,
    pub(super) thread_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ThreadRecord {
    id: String,
    trip_id: String,
    anchor: ThreadAnchor,
    title: String,
    comment_count: u32,
    last_activity_at: String,
    created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AnchorClaimRecord {
    pub(super) trip_id: String,
    pub(super) anchor: ThreadAnchor,
    pub(super) thread_id: String,
}

#[derive(Debug, Clone)]
pub(super) struct Loaded<T> {
    pub(super) value: T,
    pub(super) revision: u64,
}

#[derive(Debug, Clone)]
pub(super) struct LoadedThread {
    pub(super) thread: DiscussionThread,
    pub(super) revision: u64,
    pub(super) created_at: String,
}

pub(super) fn thread_sk(thread_id: &str) -> String {
    format!("{THREAD_PREFIX}{thread_id}")
}

pub(super) fn anchor_claim_sk(anchor: &ThreadAnchor) -> Result<String, DiscussionRepoError> {
    validate_thread_anchor(anchor).map_err(|_| DiscussionRepoError::CorruptData)?;
    let encoded = serde_json::to_vec(anchor).map_err(|_| DiscussionRepoError::CorruptData)?;
    let digest = Sha256::digest(encoded);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut hex, "{byte:02x}").expect("writing into a String cannot fail");
    }
    Ok(format!("{THREAD_ANCHOR_PREFIX}{hex}"))
}

pub(super) fn comment_prefix(thread_id: &str) -> String {
    format!("THREAD#{thread_id}#COMMENT#")
}

pub(super) fn comment_sk(thread_id: &str, comment_id: &str) -> String {
    format!("{}{comment_id}", comment_prefix(thread_id))
}

pub(super) fn encode_discussion_meta(
    meta: &DiscussionMetaRecord,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, DiscussionRepoError> {
    if meta.thread_count == 0 || meta.thread_count > 1_000 {
        return Err(DiscussionRepoError::CorruptData);
    }
    encode_record(
        trip_pk(&meta.trip_id),
        DISCUSSION_META_SK.to_string(),
        DISCUSSION_META_ENTITY,
        meta,
        revision,
    )
    .map_err(record_error)
}

pub(super) fn decode_discussion_meta(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<Loaded<DiscussionMetaRecord>, DiscussionRepoError> {
    let pk = trip_pk(expected_trip_id);
    let stored = decode_record::<DiscussionMetaRecord>(
        item,
        &pk,
        DISCUSSION_META_SK,
        DISCUSSION_META_ENTITY,
    )
    .map_err(record_error)?;
    if stored.revision == 0
        || stored.value.trip_id != expected_trip_id
        || stored.value.thread_count == 0
        || stored.value.thread_count > 1_000
    {
        return Err(DiscussionRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
    })
}

pub(super) fn encode_thread(
    thread: &DiscussionThread,
    created_at: &str,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, DiscussionRepoError> {
    validate_stored_thread(&thread.trip_id, thread)
        .map_err(|_| DiscussionRepoError::CorruptData)?;
    let created = utc(created_at)?;
    let activity = utc(&thread.last_activity_at)?;
    if activity < created {
        return Err(DiscussionRepoError::CorruptData);
    }
    let record = ThreadRecord {
        id: thread.id.clone(),
        trip_id: thread.trip_id.clone(),
        anchor: thread.anchor.clone(),
        title: thread.title.clone(),
        comment_count: thread.comment_count,
        last_activity_at: thread.last_activity_at.clone(),
        created_at: created_at.to_string(),
    };
    encode_record(
        trip_pk(&thread.trip_id),
        thread_sk(&thread.id),
        THREAD_ENTITY,
        &record,
        revision,
    )
    .map_err(record_error)
}

pub(super) fn decode_thread(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<LoadedThread, DiscussionRepoError> {
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored =
        decode_record::<ThreadRecord>(item, &pk, &sk, THREAD_ENTITY).map_err(record_error)?;
    let record = stored.value;
    let thread = DiscussionThread {
        id: record.id,
        trip_id: record.trip_id,
        anchor: record.anchor,
        title: record.title,
        comment_count: record.comment_count,
        last_activity_at: record.last_activity_at,
    };
    if stored.revision == 0
        || sk != thread_sk(&thread.id)
        || validate_stored_thread(expected_trip_id, &thread).is_err()
        || utc(&record.created_at).is_err()
    {
        return Err(DiscussionRepoError::CorruptData);
    }
    let created = utc(&record.created_at)?;
    if utc(&thread.last_activity_at)? < created {
        return Err(DiscussionRepoError::CorruptData);
    }
    Ok(LoadedThread {
        thread,
        revision: stored.revision,
        created_at: record.created_at,
    })
}

pub(super) fn encode_anchor_claim(
    claim: &AnchorClaimRecord,
) -> Result<HashMap<String, AttributeValue>, DiscussionRepoError> {
    validate_thread_anchor(&claim.anchor).map_err(|_| DiscussionRepoError::CorruptData)?;
    if !valid_id(&claim.trip_id) || !valid_id(&claim.thread_id) {
        return Err(DiscussionRepoError::CorruptData);
    }
    encode_record(
        trip_pk(&claim.trip_id),
        anchor_claim_sk(&claim.anchor)?,
        THREAD_ANCHOR_ENTITY,
        claim,
        1,
    )
    .map_err(record_error)
}

pub(super) fn decode_anchor_claim(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<Loaded<AnchorClaimRecord>, DiscussionRepoError> {
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored = decode_record::<AnchorClaimRecord>(item, &pk, &sk, THREAD_ANCHOR_ENTITY)
        .map_err(record_error)?;
    if stored.revision != 1
        || stored.value.trip_id != expected_trip_id
        || !valid_id(&stored.value.thread_id)
        || anchor_claim_sk(&stored.value.anchor)? != sk
    {
        return Err(DiscussionRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
    })
}

pub(super) fn encode_comment(
    trip_id: &str,
    comment: &Comment,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, DiscussionRepoError> {
    validate_stored_comment(&comment.thread_id, comment)
        .map_err(|_| DiscussionRepoError::CorruptData)?;
    encode_record(
        trip_pk(trip_id),
        comment_sk(&comment.thread_id, &comment.id),
        COMMENT_ENTITY,
        comment,
        revision,
    )
    .map_err(record_error)
}

pub(super) fn decode_comment(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
    expected_thread_id: &str,
) -> Result<Loaded<Comment>, DiscussionRepoError> {
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored = decode_record::<Comment>(item, &pk, &sk, COMMENT_ENTITY).map_err(record_error)?;
    if stored.revision == 0
        || sk != comment_sk(expected_thread_id, &stored.value.id)
        || validate_stored_comment(expected_thread_id, &stored.value).is_err()
    {
        return Err(DiscussionRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
    })
}

fn utc(value: &str) -> Result<DateTime<chrono::FixedOffset>, DiscussionRepoError> {
    let timestamp =
        DateTime::parse_from_rfc3339(value).map_err(|_| DiscussionRepoError::CorruptData)?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err(DiscussionRepoError::CorruptData);
    }
    Ok(timestamp)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && value.chars().count() <= 200
}
