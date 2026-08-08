//! Query-shaped codecs for discussion threads, comments, and reactions.

use chrono::{DateTime, FixedOffset, SecondsFormat};
use itinera_core::{
    domain::discussion::{Comment, DiscussionThread, Reaction, ThreadAnchor},
    ports::discussion::DiscussionRepoError,
    services::discussions::{validate_stored_comment, validate_stored_thread},
};
use sqlx::FromRow;

use crate::sqlite::codec::{checked_revision, ensure_encoded_size, validate_id};

pub(super) const MAX_THREADS: usize = 1_000;
pub(super) const MAX_COMMENTS: usize = 1_000;
pub(super) const MAX_REACTIONS: usize = 1_000;
pub(super) const COLLECTION_QUERY_LIMIT: i64 = 1_001;
pub(super) const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, FromRow)]
pub(super) struct ThreadRow {
    thread_trip_id: String,
    thread_id: String,
    anchor_kind: String,
    anchor_id: Option<String>,
    anchor_key: String,
    thread_title: String,
    thread_created_at: String,
    thread_last_activity_at: String,
    thread_revision: i64,
    comment_count: i64,
    earliest_comment_at: Option<String>,
    latest_comment_at: Option<String>,
}

#[derive(Debug, Clone)]
pub(super) struct StoredThread {
    pub(super) value: DiscussionThread,
    pub(super) revision: i64,
}

#[derive(Debug, FromRow)]
pub(super) struct CommentRow {
    comment_trip_id: String,
    comment_thread_id: String,
    comment_id: String,
    comment_author_id: String,
    comment_body: String,
    comment_created_at: String,
    comment_revision: i64,
}

#[derive(Debug, Clone)]
pub(super) struct StoredComment {
    pub(super) value: Comment,
    pub(super) revision: i64,
}

#[derive(Debug, FromRow)]
pub(super) struct ReactionRow {
    reaction_trip_id: String,
    reaction_thread_id: String,
    reaction_comment_id: String,
    reaction_emoji: String,
    reaction_user_id: String,
}

impl ThreadRow {
    pub(super) fn into_thread(
        self,
        expected_trip_id: &str,
    ) -> Result<StoredThread, DiscussionRepoError> {
        if self.thread_trip_id != expected_trip_id {
            return Err(DiscussionRepoError::CorruptData);
        }
        let revision = checked_revision(self.thread_revision).map_err(corrupt)?;
        let count = u32::try_from(self.comment_count)
            .ok()
            .filter(|count| (1..=MAX_COMMENTS as u32).contains(count))
            .ok_or(DiscussionRepoError::CorruptData)?;
        let earliest_comment_at = self
            .earliest_comment_at
            .ok_or(DiscussionRepoError::CorruptData)?;
        let latest_comment_at = self
            .latest_comment_at
            .ok_or(DiscussionRepoError::CorruptData)?;
        let created = parse_stored_time(&self.thread_created_at)?;
        let activity = parse_stored_time(&self.thread_last_activity_at)?;
        parse_stored_time(&earliest_comment_at)?;
        parse_stored_time(&latest_comment_at)?;
        if activity < created
            || earliest_comment_at != self.thread_created_at
            || latest_comment_at != self.thread_last_activity_at
        {
            return Err(DiscussionRepoError::CorruptData);
        }
        let anchor = decode_anchor(&self.anchor_kind, self.anchor_id, &self.anchor_key)?;
        let value = DiscussionThread {
            id: self.thread_id,
            trip_id: self.thread_trip_id,
            anchor,
            title: self.thread_title,
            comment_count: count,
            last_activity_at: self.thread_last_activity_at,
        };
        validate_stored_thread(expected_trip_id, &value).map_err(corrupt)?;
        Ok(StoredThread { value, revision })
    }
}

impl CommentRow {
    pub(super) fn id(&self) -> &str {
        &self.comment_id
    }

    pub(super) fn into_comment(
        self,
        expected_trip_id: &str,
        expected_thread_id: &str,
        reactions: Vec<Reaction>,
    ) -> Result<StoredComment, DiscussionRepoError> {
        if self.comment_trip_id != expected_trip_id || self.comment_thread_id != expected_thread_id
        {
            return Err(DiscussionRepoError::CorruptData);
        }
        parse_stored_time(&self.comment_created_at)?;
        let revision = checked_revision(self.comment_revision).map_err(corrupt)?;
        let value = Comment {
            id: self.comment_id,
            thread_id: self.comment_thread_id,
            author: self.comment_author_id,
            body: self.comment_body,
            created_at: self.comment_created_at,
            reactions,
        };
        validate_stored_comment(expected_thread_id, &value).map_err(corrupt)?;
        Ok(StoredComment { value, revision })
    }
}

impl ReactionRow {
    pub(super) fn comment_id(&self) -> &str {
        &self.reaction_comment_id
    }

    pub(super) fn into_parts(
        self,
        expected_trip_id: &str,
        expected_thread_id: &str,
        expected_comment_id: &str,
    ) -> Result<(String, String), DiscussionRepoError> {
        if self.reaction_trip_id != expected_trip_id
            || self.reaction_thread_id != expected_thread_id
            || self.reaction_comment_id != expected_comment_id
        {
            return Err(DiscussionRepoError::CorruptData);
        }
        validate_id(&self.reaction_user_id).map_err(corrupt)?;
        Ok((self.reaction_emoji, self.reaction_user_id))
    }
}

pub(super) fn encode_anchor(anchor: &ThreadAnchor) -> (&'static str, Option<&str>, String) {
    match anchor {
        ThreadAnchor::Trip => ("trip", None, "trip".to_string()),
        ThreadAnchor::Day { day_id } => ("day", Some(day_id), format!("day:{day_id}")),
        ThreadAnchor::Stop { stop_id } => ("stop", Some(stop_id), format!("stop:{stop_id}")),
        ThreadAnchor::Poll { poll_id } => ("poll", Some(poll_id), format!("poll:{poll_id}")),
        ThreadAnchor::Candidate { candidate_id } => (
            "candidate",
            Some(candidate_id),
            format!("candidate:{candidate_id}"),
        ),
    }
}

pub(super) fn canonical_command_time(value: &str) -> Result<String, DiscussionRepoError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(corrupt)?;
    if parsed.offset().local_minus_utc() != 0 {
        return Err(DiscussionRepoError::CorruptData);
    }
    Ok(parsed.to_rfc3339_opts(SecondsFormat::Nanos, true))
}

pub(super) fn parse_stored_time(value: &str) -> Result<DateTime<FixedOffset>, DiscussionRepoError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(corrupt)?;
    if parsed.offset().local_minus_utc() != 0
        || parsed.to_rfc3339_opts(SecondsFormat::Nanos, true) != value
    {
        return Err(DiscussionRepoError::CorruptData);
    }
    Ok(parsed)
}

pub(super) fn ensure_response_size<T: serde::Serialize>(
    value: &T,
) -> Result<(), DiscussionRepoError> {
    ensure_encoded_size(value, MAX_RESPONSE_BYTES).map_err(|error| match error {
        crate::sqlite::codec::CodecError::Invalid => DiscussionRepoError::CorruptData,
        crate::sqlite::codec::CodecError::LimitExceeded => DiscussionRepoError::CorruptData,
    })
}

fn decode_anchor(
    kind: &str,
    id: Option<String>,
    key: &str,
) -> Result<ThreadAnchor, DiscussionRepoError> {
    let anchor = match (kind, id) {
        ("trip", None) => ThreadAnchor::Trip,
        ("day", Some(day_id)) => ThreadAnchor::Day { day_id },
        ("stop", Some(stop_id)) => ThreadAnchor::Stop { stop_id },
        ("poll", Some(poll_id)) => ThreadAnchor::Poll { poll_id },
        ("candidate", Some(candidate_id)) => ThreadAnchor::Candidate { candidate_id },
        _ => return Err(DiscussionRepoError::CorruptData),
    };
    let (_, _, expected_key) = encode_anchor(&anchor);
    if key != expected_key {
        return Err(DiscussionRepoError::CorruptData);
    }
    Ok(anchor)
}

fn corrupt<T>(_error: T) -> DiscussionRepoError {
    DiscussionRepoError::CorruptData
}
