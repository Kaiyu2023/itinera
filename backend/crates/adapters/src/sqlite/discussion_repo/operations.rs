//! Transaction-local discussion reads and mutations.

use std::collections::{BTreeMap, HashSet};

use itinera_core::{
    domain::{
        discussion::{Comment, DiscussionThread, Reaction, ThreadAnchor},
        trip::{Day, Stop},
    },
    ports::{
        authorization::TripAuthorizationContext,
        discussion::{DiscussionRepoError, NewComment, NewThread},
        trip::TripRepoError,
    },
    services::discussions::{
        MAX_COMMENT_BODY_CHARS, MAX_REACTION_CHARS, MAX_THREAD_TITLE_CHARS,
        validate_stored_comment, validate_stored_thread, validate_thread_anchor,
    },
};
use sqlx::{Sqlite, Transaction};

use crate::sqlite::{
    SqliteDb,
    codec::{next_revision, validate_id, validate_text},
    trip_repo::{
        access::{RequiredRole, authorize, validate_trip_aggregate},
        plans::load_plan_detail,
    },
};

use super::records::{
    COLLECTION_QUERY_LIMIT, CommentRow, MAX_COMMENTS, MAX_REACTIONS, MAX_RESPONSE_BYTES,
    MAX_THREADS, ReactionRow, StoredComment, StoredThread, ThreadRow, canonical_command_time,
    encode_anchor, ensure_response_size, parse_stored_time,
};

pub(super) async fn list_threads(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<DiscussionThread>, DiscussionRepoError> {
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    authorize_discussion(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::AnyMember,
    )
    .await?;
    let stored = load_threads(&mut transaction, trip_id).await?;
    validate_anchors(
        &mut transaction,
        trip_id,
        stored.iter().map(|thread| &thread.value.anchor),
        MissingAnchor::Corrupt,
    )
    .await?;
    let threads = stored
        .into_iter()
        .map(|thread| thread.value)
        .collect::<Vec<_>>();
    ensure_response_size(&threads)?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(threads)
}

pub(super) async fn create_thread(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    new: NewThread,
) -> Result<DiscussionThread, DiscussionRepoError> {
    validate_new_thread(trip_id, &new)?;
    let created_at = canonical_command_time(&new.created_at)?;
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    let actor = authorize_discussion(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Editor,
    )
    .await?;
    validate_anchors(
        &mut transaction,
        trip_id,
        std::iter::once(&new.anchor),
        MissingAnchor::NotFound,
    )
    .await?;
    let mut threads = load_threads(&mut transaction, trip_id).await?;
    validate_anchors(
        &mut transaction,
        trip_id,
        threads.iter().map(|thread| &thread.value.anchor),
        MissingAnchor::Corrupt,
    )
    .await?;
    if threads.len() >= MAX_THREADS {
        return Err(DiscussionRepoError::SafetyLimitExceeded);
    }
    let (_, anchor_id, anchor_key) = encode_anchor(&new.anchor);
    let collision: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM discussion_threads \
         WHERE trip_id = ? AND (id = ? OR anchor_key = ?))",
    )
    .bind(trip_id)
    .bind(&new.id)
    .bind(&anchor_key)
    .fetch_one(&mut *transaction)
    .await
    .map_err(unavailable)?;
    let comment_collision: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM discussion_comments \
         WHERE trip_id = ? AND thread_id = ? AND id = ?)",
    )
    .bind(trip_id)
    .bind(&new.id)
    .bind(&new.first_comment_id)
    .fetch_one(&mut *transaction)
    .await
    .map_err(unavailable)?;
    if collision != 0 || comment_collision != 0 {
        return Err(DiscussionRepoError::Conflict);
    }
    let thread = DiscussionThread {
        id: new.id.clone(),
        trip_id: trip_id.to_string(),
        anchor: new.anchor.clone(),
        title: new.title.clone(),
        comment_count: 1,
        last_activity_at: created_at.clone(),
    };
    validate_stored_thread(trip_id, &thread).map_err(corrupt)?;
    let comment = Comment {
        id: new.first_comment_id.clone(),
        thread_id: new.id.clone(),
        author: actor.clone(),
        body: new.body.clone(),
        created_at: created_at.clone(),
        reactions: Vec::new(),
    };
    validate_stored_comment(&new.id, &comment).map_err(corrupt)?;
    let mut projected = threads
        .drain(..)
        .map(|stored| stored.value)
        .collect::<Vec<_>>();
    projected.push(thread.clone());
    sort_threads(&mut projected)?;
    ensure_projected_size(&projected)?;

    let (anchor_kind, _, _) = encode_anchor(&new.anchor);
    sqlx::query(
        "INSERT INTO discussion_threads ( \
             trip_id, id, anchor_kind, anchor_id, anchor_key, title, created_at, \
             last_activity_at, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1)",
    )
    .bind(trip_id)
    .bind(&new.id)
    .bind(anchor_kind)
    .bind(anchor_id)
    .bind(anchor_key)
    .bind(&new.title)
    .bind(&created_at)
    .bind(&created_at)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    insert_comment(
        &mut transaction,
        trip_id,
        &new.id,
        &new.first_comment_id,
        &actor,
        &new.body,
        &created_at,
    )
    .await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(thread)
}

pub(super) async fn get_comments(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    thread_id: &str,
) -> Result<Vec<Comment>, DiscussionRepoError> {
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    authorize_discussion(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::AnyMember,
    )
    .await?;
    let thread = load_thread(&mut transaction, trip_id, thread_id).await?;
    validate_anchors(
        &mut transaction,
        trip_id,
        std::iter::once(&thread.value.anchor),
        MissingAnchor::Corrupt,
    )
    .await?;
    let comments = load_comments(&mut transaction, trip_id, &thread).await?;
    let values = comments
        .into_iter()
        .map(|comment| comment.value)
        .collect::<Vec<_>>();
    ensure_response_size(&values)?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(values)
}

pub(super) async fn add_comment(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    thread_id: &str,
    new: NewComment,
) -> Result<Comment, DiscussionRepoError> {
    validate_id(&new.id).map_err(corrupt)?;
    validate_text(&new.body, MAX_COMMENT_BODY_CHARS).map_err(corrupt)?;
    let created_at = canonical_command_time(&new.created_at)?;
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    let actor = authorize_discussion(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Editor,
    )
    .await?;
    let thread = load_thread(&mut transaction, trip_id, thread_id).await?;
    validate_anchors(
        &mut transaction,
        trip_id,
        std::iter::once(&thread.value.anchor),
        MissingAnchor::Corrupt,
    )
    .await?;
    let mut comments = load_comments(&mut transaction, trip_id, &thread).await?;
    let comment = Comment {
        id: new.id,
        thread_id: thread_id.to_string(),
        author: actor.clone(),
        body: new.body,
        created_at,
        reactions: Vec::new(),
    };
    validate_stored_comment(thread_id, &comment).map_err(corrupt)?;
    if let Some(existing) = comments.iter().find(|stored| stored.value.id == comment.id) {
        return if existing.value == comment {
            Ok(existing.value.clone())
        } else {
            Err(DiscussionRepoError::CorruptData)
        };
    }
    if comments.len() >= MAX_COMMENTS {
        return Err(DiscussionRepoError::SafetyLimitExceeded);
    }
    if parse_stored_time(&comment.created_at)? < parse_stored_time(&thread.value.last_activity_at)?
    {
        return Err(DiscussionRepoError::Conflict);
    }
    let mut projected = comments
        .drain(..)
        .map(|stored| stored.value)
        .collect::<Vec<_>>();
    projected.push(comment.clone());
    sort_comments(&mut projected)?;
    ensure_projected_size(&projected)?;

    insert_comment(
        &mut transaction,
        trip_id,
        thread_id,
        &comment.id,
        &actor,
        &comment.body,
        &comment.created_at,
    )
    .await?;
    let updated = sqlx::query(
        "UPDATE discussion_threads SET last_activity_at = ?, revision = ? \
         WHERE trip_id = ? AND id = ? AND revision = ? AND last_activity_at = ?",
    )
    .bind(&comment.created_at)
    .bind(next_revision(thread.revision).map_err(corrupt)?)
    .bind(trip_id)
    .bind(thread_id)
    .bind(thread.revision)
    .bind(&thread.value.last_activity_at)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    if updated.rows_affected() != 1 {
        return Err(DiscussionRepoError::Conflict);
    }
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(comment)
}

pub(super) async fn set_reaction(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    thread_id: &str,
    comment_id: &str,
    emoji: &str,
    active: bool,
) -> Result<Comment, DiscussionRepoError> {
    validate_text(emoji, MAX_REACTION_CHARS).map_err(corrupt)?;
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    let actor = authorize_discussion(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Editor,
    )
    .await?;
    let thread = load_thread(&mut transaction, trip_id, thread_id).await?;
    validate_anchors(
        &mut transaction,
        trip_id,
        std::iter::once(&thread.value.anchor),
        MissingAnchor::Corrupt,
    )
    .await?;
    let mut comments = load_comments(&mut transaction, trip_id, &thread).await?;
    let (result, target_revision) = {
        let target = comments
            .iter_mut()
            .find(|comment| comment.value.id == comment_id)
            .ok_or(DiscussionRepoError::NotFound)?;
        let reaction = target
            .value
            .reactions
            .iter_mut()
            .find(|reaction| reaction.emoji == emoji);
        let currently_active = reaction
            .as_ref()
            .is_some_and(|reaction| reaction.user_ids.binary_search(&actor).is_ok());
        if currently_active == active {
            let result = target.value.clone();
            db.commit(transaction).await.map_err(unavailable)?;
            return Ok(result);
        }
        match (reaction, active) {
            (Some(reaction), true) => {
                if reaction.user_ids.len() >= MAX_REACTIONS {
                    return Err(DiscussionRepoError::SafetyLimitExceeded);
                }
                reaction.user_ids.push(actor.clone());
                reaction.user_ids.sort();
            }
            (Some(reaction), false) => {
                reaction.user_ids.retain(|user_id| user_id != &actor);
            }
            (None, true) => {
                if target.value.reactions.len() >= MAX_REACTIONS {
                    return Err(DiscussionRepoError::SafetyLimitExceeded);
                }
                target.value.reactions.push(Reaction {
                    emoji: emoji.to_string(),
                    user_ids: vec![actor.clone()],
                });
                target
                    .value
                    .reactions
                    .sort_by(|left, right| left.emoji.cmp(&right.emoji));
            }
            (None, false) => return Err(DiscussionRepoError::CorruptData),
        }
        target
            .value
            .reactions
            .retain(|reaction| !reaction.user_ids.is_empty());
        validate_stored_comment(thread_id, &target.value).map_err(corrupt)?;
        (target.value.clone(), target.revision)
    };
    let projected = comments
        .iter()
        .map(|stored| stored.value.clone())
        .collect::<Vec<_>>();
    ensure_projected_size(&projected)?;

    if active {
        sqlx::query(
            "INSERT INTO comment_reactions (trip_id, thread_id, comment_id, emoji, user_id) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(trip_id)
        .bind(thread_id)
        .bind(comment_id)
        .bind(emoji)
        .bind(&actor)
        .execute(&mut *transaction)
        .await
        .map_err(unavailable)?;
    } else {
        let deleted = sqlx::query(
            "DELETE FROM comment_reactions \
             WHERE trip_id = ? AND thread_id = ? AND comment_id = ? \
               AND emoji = ? AND user_id = ?",
        )
        .bind(trip_id)
        .bind(thread_id)
        .bind(comment_id)
        .bind(emoji)
        .bind(&actor)
        .execute(&mut *transaction)
        .await
        .map_err(unavailable)?;
        if deleted.rows_affected() != 1 {
            return Err(DiscussionRepoError::Conflict);
        }
    }
    let changed = sqlx::query(
        "UPDATE discussion_comments SET revision = ? \
         WHERE trip_id = ? AND thread_id = ? AND id = ? AND revision = ?",
    )
    .bind(next_revision(target_revision).map_err(corrupt)?)
    .bind(trip_id)
    .bind(thread_id)
    .bind(comment_id)
    .bind(target_revision)
    .execute(&mut *transaction)
    .await
    .map_err(unavailable)?;
    if changed.rows_affected() != 1 {
        return Err(DiscussionRepoError::Conflict);
    }
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(in crate::sqlite) async fn validate_plan_anchor_survival(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    current_days: &[Day],
    current_stops: &[Stop],
    resulting_days: &[Day],
    resulting_stops: &[Stop],
) -> Result<(), DiscussionRepoError> {
    let rows = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT anchor_kind, anchor_id FROM discussion_threads \
         WHERE trip_id = ? AND anchor_kind IN ('day', 'stop') \
         ORDER BY anchor_kind, anchor_id LIMIT ?",
    )
    .bind(trip_id)
    .bind(COLLECTION_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() > MAX_THREADS {
        return Err(DiscussionRepoError::CorruptData);
    }
    let current_day_ids = current_days
        .iter()
        .map(|day| day.id.as_str())
        .collect::<HashSet<_>>();
    let current_stop_ids = current_stops
        .iter()
        .map(|stop| stop.id.as_str())
        .collect::<HashSet<_>>();
    let resulting_day_ids = resulting_days
        .iter()
        .map(|day| day.id.as_str())
        .collect::<HashSet<_>>();
    let resulting_stop_ids = resulting_stops
        .iter()
        .map(|stop| stop.id.as_str())
        .collect::<HashSet<_>>();
    for (kind, id) in rows {
        let id = id.ok_or(DiscussionRepoError::CorruptData)?;
        let (exists_now, survives) = match kind.as_str() {
            "day" => (
                current_day_ids.contains(id.as_str()),
                resulting_day_ids.contains(id.as_str()),
            ),
            "stop" => (
                current_stop_ids.contains(id.as_str()),
                resulting_stop_ids.contains(id.as_str()),
            ),
            _ => return Err(DiscussionRepoError::CorruptData),
        };
        if !exists_now {
            return Err(DiscussionRepoError::CorruptData);
        }
        if !survives {
            return Err(DiscussionRepoError::Conflict);
        }
    }
    Ok(())
}

async fn authorize_discussion(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    role: RequiredRole,
) -> Result<String, DiscussionRepoError> {
    authorize(transaction, trip_id, authorization, role)
        .await
        .map_err(map_trip_error)?;
    validate_trip_aggregate(transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    authorization
        .human_user_id()
        .map(|user_id| user_id.0.clone())
        .ok_or(DiscussionRepoError::Forbidden)
}

async fn load_threads(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<StoredThread>, DiscussionRepoError> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM discussion_threads WHERE trip_id = ?")
            .bind(trip_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(unavailable)?;
    if usize::try_from(count)
        .ok()
        .is_none_or(|count| count > MAX_THREADS)
    {
        return Err(DiscussionRepoError::CorruptData);
    }
    let rows = sqlx::query_as::<_, ThreadRow>(
        "SELECT t.trip_id AS thread_trip_id, t.id AS thread_id, \
                t.anchor_kind, t.anchor_id, t.anchor_key, \
                t.title AS thread_title, t.created_at AS thread_created_at, \
                t.last_activity_at AS thread_last_activity_at, \
                t.revision AS thread_revision, COUNT(c.id) AS comment_count, \
                MIN(c.created_at) AS earliest_comment_at, \
                MAX(c.created_at) AS latest_comment_at \
         FROM discussion_threads AS t \
         LEFT JOIN discussion_comments AS c \
           ON c.trip_id = t.trip_id AND c.thread_id = t.id \
         WHERE t.trip_id = ? \
         GROUP BY t.trip_id, t.id \
         ORDER BY t.last_activity_at DESC, t.id DESC LIMIT ?",
    )
    .bind(trip_id)
    .bind(COLLECTION_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if i64::try_from(rows.len()).ok() != Some(count) {
        return Err(DiscussionRepoError::CorruptData);
    }
    let mut stored = rows
        .into_iter()
        .map(|row| row.into_thread(trip_id))
        .collect::<Result<Vec<_>, _>>()?;
    stored.sort_by(|left, right| {
        right
            .value
            .last_activity_at
            .cmp(&left.value.last_activity_at)
            .then_with(|| right.value.id.cmp(&left.value.id))
    });
    Ok(stored)
}

async fn load_thread(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    thread_id: &str,
) -> Result<StoredThread, DiscussionRepoError> {
    let row = sqlx::query_as::<_, ThreadRow>(
        "SELECT t.trip_id AS thread_trip_id, t.id AS thread_id, \
                t.anchor_kind, t.anchor_id, t.anchor_key, \
                t.title AS thread_title, t.created_at AS thread_created_at, \
                t.last_activity_at AS thread_last_activity_at, \
                t.revision AS thread_revision, COUNT(c.id) AS comment_count, \
                MIN(c.created_at) AS earliest_comment_at, \
                MAX(c.created_at) AS latest_comment_at \
         FROM discussion_threads AS t \
         LEFT JOIN discussion_comments AS c \
           ON c.trip_id = t.trip_id AND c.thread_id = t.id \
         WHERE t.trip_id = ? AND t.id = ? \
         GROUP BY t.trip_id, t.id",
    )
    .bind(trip_id)
    .bind(thread_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?
    .ok_or(DiscussionRepoError::NotFound)?;
    row.into_thread(trip_id)
}

async fn load_comments(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    thread: &StoredThread,
) -> Result<Vec<StoredComment>, DiscussionRepoError> {
    let raw_bytes: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM( \
             length(CAST(id AS BLOB)) + length(CAST(thread_id AS BLOB)) + \
             length(CAST(author_id AS BLOB)) + length(CAST(body AS BLOB)) + \
             length(CAST(created_at AS BLOB)) \
         ), 0) FROM discussion_comments WHERE trip_id = ? AND thread_id = ?",
    )
    .bind(trip_id)
    .bind(&thread.value.id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let reaction_lower_bound: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(length(CAST(user_id AS BLOB)) + 3), 0) \
         FROM comment_reactions WHERE trip_id = ? AND thread_id = ?",
    )
    .bind(trip_id)
    .bind(&thread.value.id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if raw_bytes < 0
        || reaction_lower_bound < 0
        || raw_bytes
            .checked_add(reaction_lower_bound)
            .and_then(|bytes| usize::try_from(bytes).ok())
            .is_none_or(|bytes| bytes > MAX_RESPONSE_BYTES)
    {
        return Err(DiscussionRepoError::CorruptData);
    }
    let rows = sqlx::query_as::<_, CommentRow>(
        "SELECT trip_id AS comment_trip_id, thread_id AS comment_thread_id, \
                id AS comment_id, author_id AS comment_author_id, \
                body AS comment_body, created_at AS comment_created_at, \
                revision AS comment_revision \
         FROM discussion_comments WHERE trip_id = ? AND thread_id = ? \
         ORDER BY created_at, id LIMIT ?",
    )
    .bind(trip_id)
    .bind(&thread.value.id)
    .bind(COLLECTION_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() != thread.value.comment_count as usize || rows.len() > MAX_COMMENTS {
        return Err(DiscussionRepoError::CorruptData);
    }
    let reaction_rows = sqlx::query_as::<_, ReactionRow>(
        "SELECT trip_id AS reaction_trip_id, thread_id AS reaction_thread_id, \
                comment_id AS reaction_comment_id, emoji AS reaction_emoji, \
                user_id AS reaction_user_id \
         FROM comment_reactions WHERE trip_id = ? AND thread_id = ? \
         ORDER BY comment_id, emoji, user_id LIMIT 1000001",
    )
    .bind(trip_id)
    .bind(&thread.value.id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if reaction_rows.len() > MAX_COMMENTS * MAX_REACTIONS * MAX_REACTIONS {
        return Err(DiscussionRepoError::CorruptData);
    }
    let mut grouped = BTreeMap::<String, BTreeMap<String, Vec<String>>>::new();
    for row in reaction_rows {
        let comment_id = row.comment_id().to_string();
        let (emoji, user_id) = row.into_parts(trip_id, &thread.value.id, &comment_id)?;
        let reactions = grouped.entry(comment_id).or_default();
        if reactions.len() >= MAX_REACTIONS && !reactions.contains_key(&emoji) {
            return Err(DiscussionRepoError::CorruptData);
        }
        let users = reactions.entry(emoji).or_default();
        if users.len() >= MAX_REACTIONS {
            return Err(DiscussionRepoError::CorruptData);
        }
        users.push(user_id);
    }
    let mut comments = Vec::with_capacity(rows.len());
    for row in rows {
        let comment_id = row.id().to_string();
        let reactions = grouped
            .remove(&comment_id)
            .unwrap_or_default()
            .into_iter()
            .map(|(emoji, user_ids)| Reaction { emoji, user_ids })
            .collect::<Vec<_>>();
        comments.push(row.into_comment(trip_id, &thread.value.id, reactions)?);
    }
    if !grouped.is_empty() {
        return Err(DiscussionRepoError::CorruptData);
    }
    comments.sort_by(|left, right| {
        left.value
            .created_at
            .cmp(&right.value.created_at)
            .then_with(|| left.value.id.cmp(&right.value.id))
    });
    if comments
        .last()
        .is_none_or(|comment| comment.value.created_at != thread.value.last_activity_at)
    {
        return Err(DiscussionRepoError::CorruptData);
    }
    let values = comments
        .iter()
        .map(|comment| &comment.value)
        .collect::<Vec<_>>();
    ensure_response_size(&values)?;
    Ok(comments)
}

async fn validate_anchors<'a>(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    anchors: impl Iterator<Item = &'a ThreadAnchor>,
    missing: MissingAnchor,
) -> Result<(), DiscussionRepoError> {
    let anchors = anchors.collect::<Vec<_>>();
    let candidate_ids = anchors
        .iter()
        .filter_map(|anchor| match anchor {
            ThreadAnchor::Candidate { candidate_id } => Some(candidate_id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let poll_ids = anchors
        .iter()
        .filter_map(|anchor| match anchor {
            ThreadAnchor::Poll { poll_id } => Some(poll_id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    if !candidate_ids.is_empty() {
        let stored = sqlx::query_scalar::<_, String>(
            "SELECT id FROM candidates WHERE trip_id = ? ORDER BY id LIMIT 1001",
        )
        .bind(trip_id)
        .fetch_all(&mut **transaction)
        .await
        .map_err(unavailable)?;
        if stored.len() > MAX_THREADS {
            return Err(DiscussionRepoError::CorruptData);
        }
        let stored = stored.into_iter().collect::<HashSet<_>>();
        if candidate_ids.iter().any(|id| !stored.contains(*id)) {
            return Err(missing.error());
        }
    }
    if !poll_ids.is_empty() {
        let stored = sqlx::query_scalar::<_, String>(
            "SELECT id FROM polls WHERE trip_id = ? ORDER BY id LIMIT 1001",
        )
        .bind(trip_id)
        .fetch_all(&mut **transaction)
        .await
        .map_err(unavailable)?;
        if stored.len() > MAX_THREADS {
            return Err(DiscussionRepoError::CorruptData);
        }
        let stored = stored.into_iter().collect::<HashSet<_>>();
        if poll_ids.iter().any(|id| !stored.contains(*id)) {
            return Err(missing.error());
        }
    }
    let day_ids = anchors
        .iter()
        .filter_map(|anchor| match anchor {
            ThreadAnchor::Day { day_id } => Some(day_id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let stop_ids = anchors
        .iter()
        .filter_map(|anchor| match anchor {
            ThreadAnchor::Stop { stop_id } => Some(stop_id.as_str()),
            _ => None,
        })
        .collect::<HashSet<_>>();
    if !day_ids.is_empty() || !stop_ids.is_empty() {
        let pointer = sqlx::query_as::<_, (Option<String>, Option<i64>)>(
            "SELECT current_plan_id, current_plan_version FROM trips WHERE id = ?",
        )
        .bind(trip_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(unavailable)?;
        let (Some(plan_id), Some(version)) = pointer else {
            return Err(missing.error());
        };
        let version = u32::try_from(version)
            .ok()
            .filter(|version| *version > 0)
            .ok_or(DiscussionRepoError::CorruptData)?;
        let plan = load_plan_detail(transaction, trip_id, &plan_id, version)
            .await
            .map_err(map_trip_error)?;
        let current_days = plan
            .days
            .iter()
            .map(|day| day.id.as_str())
            .collect::<HashSet<_>>();
        let current_stops = plan
            .stops
            .iter()
            .map(|stop| stop.id.as_str())
            .collect::<HashSet<_>>();
        if day_ids.iter().any(|id| !current_days.contains(id))
            || stop_ids.iter().any(|id| !current_stops.contains(id))
        {
            return Err(missing.error());
        }
    }
    Ok(())
}

async fn insert_comment(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    thread_id: &str,
    comment_id: &str,
    author: &str,
    body: &str,
    created_at: &str,
) -> Result<(), DiscussionRepoError> {
    sqlx::query(
        "INSERT INTO discussion_comments ( \
             trip_id, thread_id, id, author_id, body, created_at, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, 1)",
    )
    .bind(trip_id)
    .bind(thread_id)
    .bind(comment_id)
    .bind(author)
    .bind(body)
    .bind(created_at)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    Ok(())
}

fn validate_new_thread(trip_id: &str, new: &NewThread) -> Result<(), DiscussionRepoError> {
    validate_id(trip_id).map_err(corrupt)?;
    validate_id(&new.id).map_err(corrupt)?;
    validate_id(&new.first_comment_id).map_err(corrupt)?;
    validate_thread_anchor(&new.anchor).map_err(corrupt)?;
    validate_text(&new.title, MAX_THREAD_TITLE_CHARS).map_err(corrupt)?;
    validate_text(&new.body, MAX_COMMENT_BODY_CHARS).map_err(corrupt)
}

fn sort_threads(threads: &mut [DiscussionThread]) -> Result<(), DiscussionRepoError> {
    for thread in threads.iter() {
        parse_stored_time(&thread.last_activity_at)?;
    }
    threads.sort_by(|left, right| {
        right
            .last_activity_at
            .cmp(&left.last_activity_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(())
}

fn sort_comments(comments: &mut [Comment]) -> Result<(), DiscussionRepoError> {
    for comment in comments.iter() {
        parse_stored_time(&comment.created_at)?;
    }
    comments.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(())
}

fn ensure_projected_size<T: serde::Serialize>(value: &T) -> Result<(), DiscussionRepoError> {
    let bytes = serde_json::to_vec(value).map_err(corrupt)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        Err(DiscussionRepoError::SafetyLimitExceeded)
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum MissingAnchor {
    NotFound,
    Corrupt,
}

impl MissingAnchor {
    fn error(self) -> DiscussionRepoError {
        match self {
            Self::NotFound => DiscussionRepoError::NotFound,
            Self::Corrupt => DiscussionRepoError::CorruptData,
        }
    }
}

fn map_trip_error(error: TripRepoError) -> DiscussionRepoError {
    match error {
        TripRepoError::Unavailable => DiscussionRepoError::Unavailable,
        TripRepoError::CorruptData => DiscussionRepoError::CorruptData,
        TripRepoError::NotFound => DiscussionRepoError::NotFound,
        TripRepoError::Forbidden => DiscussionRepoError::Forbidden,
        TripRepoError::Conflict | TripRepoError::DuplicateInvite => DiscussionRepoError::Conflict,
    }
}

fn unavailable<T>(_error: T) -> DiscussionRepoError {
    DiscussionRepoError::Unavailable
}

fn corrupt<T>(_error: T) -> DiscussionRepoError {
    DiscussionRepoError::CorruptData
}
