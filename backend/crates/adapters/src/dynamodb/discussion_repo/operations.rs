use std::collections::{HashMap, HashSet};

use aws_sdk_dynamodb::types::ConditionCheck;
use chrono::DateTime;
use itinera_core::{
    domain::{
        discussion::{Comment, DiscussionThread, Reaction, ThreadAnchor},
        trip::{Candidate, Day, Plan, Stop},
        user::UserId,
    },
    ports::discussion::{DiscussionRepoError, NewComment, NewThread},
    services::{candidates::validate_stored_candidate, plans::validate_stored_plan_graph},
};

use crate::dynamodb::{
    DynamoUserRepo, ENTITY_TYPE, SK,
    poll_repo::records::{POLL_ENTITY, decode_poll, poll_sk},
    primitives::{condition_action, put_action, transaction_condition_failed},
    trip_repo::records::{
        CANDIDATE_ENTITY, DAY_ENTITY, PLAN_ENTITY, STOP_ENTITY, Stored, candidate_sk, day_sk,
        decode_record, plan_prefix, plan_sk, stop_sk, string, trip_pk,
    },
};

use super::{
    access::{MAX_COMMENTS_PER_THREAD, MAX_DISCUSSION_BYTES, MAX_THREADS, RequiredDiscussionRole},
    poll_error, record_error,
    records::{
        AnchorClaimRecord, DISCUSSION_META_SK, DiscussionMetaRecord, Loaded, LoadedThread,
        THREAD_ANCHOR_PREFIX, THREAD_ENTITY, THREAD_PREFIX, anchor_claim_sk, comment_prefix,
        decode_anchor_claim, decode_comment, decode_discussion_meta, decode_thread,
        encode_anchor_claim, encode_comment, encode_discussion_meta, encode_thread, thread_sk,
    },
};

pub(super) async fn list_threads(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Vec<DiscussionThread>, DiscussionRepoError> {
    repo.discussion_authorize(trip_id, actor, RequiredDiscussionRole::Any)
        .await?;
    let loaded = load_thread_collection_with_retry(repo, trip_id).await?;
    let result = loaded
        .into_iter()
        .map(|loaded| loaded.thread)
        .collect::<Vec<_>>();
    enforce_response_limit(&result)?;
    Ok(result)
}

pub(super) async fn create_thread(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    new: NewThread,
) -> Result<DiscussionThread, DiscussionRepoError> {
    repo.discussion_authorize(trip_id, actor, RequiredDiscussionRole::Editor)
        .await?;
    load_thread_collection_with_retry(repo, trip_id).await?;
    let anchor_conditions = prepare_anchor_conditions(repo, trip_id, &new.anchor).await?;
    let existing_claim = load_anchor_claim(repo, trip_id, &new.anchor).await?;
    if let Some(claim) = existing_claim {
        let existing = load_claimed_thread(repo, trip_id, &claim.value.thread_id).await?;
        return if existing.thread.anchor == new.anchor {
            Err(DiscussionRepoError::Conflict)
        } else {
            Err(DiscussionRepoError::CorruptData)
        };
    }
    let meta = load_discussion_meta(repo, trip_id).await?;
    let current_count = meta.as_ref().map_or(0, |meta| meta.value.thread_count);
    if current_count as usize >= MAX_THREADS {
        return Err(DiscussionRepoError::SafetyLimitExceeded);
    }
    let next_count = current_count
        .checked_add(1)
        .ok_or(DiscussionRepoError::SafetyLimitExceeded)?;
    let thread = DiscussionThread {
        id: new.id.clone(),
        trip_id: trip_id.to_string(),
        anchor: new.anchor.clone(),
        title: new.title,
        comment_count: 1,
        last_activity_at: new.created_at.clone(),
    };
    let first_comment = Comment {
        id: new.first_comment_id,
        thread_id: new.id,
        author: actor.0.clone(),
        body: new.body,
        created_at: new.created_at.clone(),
        reactions: vec![],
    };
    let next_meta = DiscussionMetaRecord {
        trip_id: trip_id.to_string(),
        thread_count: next_count,
    };
    let claim = AnchorClaimRecord {
        trip_id: trip_id.to_string(),
        anchor: new.anchor,
        thread_id: thread.id.clone(),
    };
    let next_meta_revision = match &meta {
        Some(meta) => meta
            .revision
            .checked_add(1)
            .ok_or(DiscussionRepoError::CorruptData)?,
        None => 1,
    };
    let meta_item = encode_discussion_meta(&next_meta, next_meta_revision)?;
    let meta_put = match &meta {
        Some(meta) => repo.revision_put(meta_item, meta.revision),
        None => repo.create_only_put(meta_item),
    };
    let mut actions = vec![condition_action(repo.discussion_membership_condition(
        trip_id,
        actor,
        RequiredDiscussionRole::Editor,
    ))];
    actions.extend(anchor_conditions.into_iter().map(condition_action));
    actions.extend([
        put_action(meta_put),
        put_action(repo.create_only_put(encode_anchor_claim(&claim)?)),
        put_action(repo.create_only_put(encode_thread(&thread, &new.created_at, 1)?)),
        put_action(repo.create_only_put(encode_comment(trip_id, &first_comment, 1)?)),
    ]);
    match repo
        .transaction()
        .set_transact_items(Some(actions))
        .send()
        .await
    {
        Ok(_) => Ok(thread),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            repo.discussion_authorize(trip_id, actor, RequiredDiscussionRole::Editor)
                .await?;
            if let Some(existing_claim) = load_anchor_claim(repo, trip_id, &claim.anchor).await? {
                if existing_claim.value.anchor != claim.anchor {
                    return Err(DiscussionRepoError::CorruptData);
                }
                let existing =
                    load_claimed_thread(repo, trip_id, &existing_claim.value.thread_id).await?;
                if existing.thread.anchor != existing_claim.value.anchor {
                    return Err(DiscussionRepoError::CorruptData);
                }
            }
            Err(DiscussionRepoError::Conflict)
        }
        Err(_) => Err(DiscussionRepoError::Unavailable),
    }
}

pub(super) async fn get_comments(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    thread_id: &str,
) -> Result<Vec<Comment>, DiscussionRepoError> {
    repo.discussion_authorize(trip_id, actor, RequiredDiscussionRole::Any)
        .await?;
    let (_, comments) = load_thread_state_with_retry(repo, trip_id, thread_id).await?;
    Ok(comments.into_iter().map(|loaded| loaded.value).collect())
}

pub(super) async fn add_comment(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    thread_id: &str,
    new: NewComment,
) -> Result<Comment, DiscussionRepoError> {
    repo.discussion_authorize(trip_id, actor, RequiredDiscussionRole::Editor)
        .await?;
    let (loaded, _) = load_thread_state_with_retry(repo, trip_id, thread_id).await?;
    if loaded.thread.comment_count as usize >= MAX_COMMENTS_PER_THREAD {
        return Err(DiscussionRepoError::SafetyLimitExceeded);
    }
    let created_at = utc(&new.created_at)?;
    if created_at < utc(&loaded.created_at)? || created_at < utc(&loaded.thread.last_activity_at)? {
        return Err(DiscussionRepoError::Conflict);
    }
    let comment = Comment {
        id: new.id,
        thread_id: thread_id.to_string(),
        author: actor.0.clone(),
        body: new.body,
        created_at: new.created_at,
        reactions: vec![],
    };
    let mut updated_thread = loaded.thread.clone();
    updated_thread.comment_count = updated_thread
        .comment_count
        .checked_add(1)
        .ok_or(DiscussionRepoError::SafetyLimitExceeded)?;
    updated_thread
        .last_activity_at
        .clone_from(&comment.created_at);
    let next_revision = loaded
        .revision
        .checked_add(1)
        .ok_or(DiscussionRepoError::CorruptData)?;
    let actions = vec![
        condition_action(repo.discussion_membership_condition(
            trip_id,
            actor,
            RequiredDiscussionRole::Editor,
        )),
        put_action(repo.revision_put(
            encode_thread(&updated_thread, &loaded.created_at, next_revision)?,
            loaded.revision,
        )),
        put_action(repo.create_only_put(encode_comment(trip_id, &comment, 1)?)),
    ];
    match repo
        .transaction()
        .set_transact_items(Some(actions))
        .send()
        .await
    {
        Ok(_) => Ok(comment),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            repo.discussion_authorize(trip_id, actor, RequiredDiscussionRole::Editor)
                .await?;
            let (_, comments) = load_thread_state_with_retry(repo, trip_id, thread_id).await?;
            if let Some(existing) = comments
                .into_iter()
                .find(|loaded| loaded.value.id == comment.id)
            {
                return if existing.value == comment {
                    Ok(existing.value)
                } else {
                    Err(DiscussionRepoError::CorruptData)
                };
            }
            Err(DiscussionRepoError::Conflict)
        }
        Err(_) => Err(DiscussionRepoError::Unavailable),
    }
}

pub(super) async fn set_reaction(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    thread_id: &str,
    comment_id: &str,
    emoji: &str,
    active: bool,
) -> Result<Comment, DiscussionRepoError> {
    repo.discussion_authorize(trip_id, actor, RequiredDiscussionRole::Editor)
        .await?;
    let (thread, comments) = load_thread_state_with_retry(repo, trip_id, thread_id).await?;
    let loaded = comments
        .into_iter()
        .find(|loaded| loaded.value.id == comment_id)
        .ok_or(DiscussionRepoError::NotFound)?;
    if reaction_is_active(&loaded.value, emoji, &actor.0) == active {
        return Ok(loaded.value);
    }
    if active {
        match loaded
            .value
            .reactions
            .iter()
            .find(|reaction| reaction.emoji == emoji)
        {
            Some(reaction) if reaction.user_ids.len() >= 1_000 => {
                return Err(DiscussionRepoError::SafetyLimitExceeded);
            }
            None if loaded.value.reactions.len() >= 1_000 => {
                return Err(DiscussionRepoError::SafetyLimitExceeded);
            }
            _ => {}
        }
    }
    let mut updated = loaded.value.clone();
    apply_reaction(&mut updated, emoji, &actor.0, active);
    let next_revision = loaded
        .revision
        .checked_add(1)
        .ok_or(DiscussionRepoError::CorruptData)?;
    let actions = vec![
        condition_action(repo.discussion_membership_condition(
            trip_id,
            actor,
            RequiredDiscussionRole::Editor,
        )),
        condition_action(repo.entity_revision_condition(
            trip_pk(trip_id),
            thread_sk(thread_id),
            THREAD_ENTITY,
            thread.revision,
        )),
        put_action(repo.revision_put(
            encode_comment(trip_id, &updated, next_revision)?,
            loaded.revision,
        )),
    ];
    match repo
        .transaction()
        .set_transact_items(Some(actions))
        .send()
        .await
    {
        Ok(_) => Ok(updated),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            repo.discussion_authorize(trip_id, actor, RequiredDiscussionRole::Editor)
                .await?;
            let (_, comments) = load_thread_state_with_retry(repo, trip_id, thread_id).await?;
            let latest = comments
                .into_iter()
                .find(|loaded| loaded.value.id == comment_id)
                .ok_or(DiscussionRepoError::NotFound)?;
            if reaction_is_active(&latest.value, emoji, &actor.0) == active {
                Ok(latest.value)
            } else {
                Err(DiscussionRepoError::Conflict)
            }
        }
        Err(_) => Err(DiscussionRepoError::Unavailable),
    }
}

async fn load_thread_collection_with_retry(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<Vec<LoadedThread>, DiscussionRepoError> {
    match load_thread_collection(repo, trip_id).await {
        Err(DiscussionRepoError::CorruptData) => load_thread_collection(repo, trip_id).await,
        result => result,
    }
}

async fn load_thread_collection(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<Vec<LoadedThread>, DiscussionRepoError> {
    let pk = trip_pk(trip_id);
    let meta = load_discussion_meta(repo, trip_id).await?;
    let thread_items = repo
        .discussion_query(&pk, THREAD_PREFIX, MAX_THREADS)
        .await?;
    let claim_items = repo
        .discussion_query(&pk, THREAD_ANCHOR_PREFIX, MAX_THREADS)
        .await?;
    let Some(meta) = meta else {
        return if thread_items.is_empty() && claim_items.is_empty() {
            Ok(vec![])
        } else {
            Err(DiscussionRepoError::CorruptData)
        };
    };
    if thread_items.len() != meta.value.thread_count as usize
        || claim_items.len() != meta.value.thread_count as usize
    {
        return Err(DiscussionRepoError::CorruptData);
    }
    let mut threads = HashMap::new();
    let mut anchors = HashSet::new();
    for item in thread_items {
        let thread = decode_thread(&item, trip_id)?;
        if !anchors.insert(thread.thread.anchor.clone())
            || threads.insert(thread.thread.id.clone(), thread).is_some()
        {
            return Err(DiscussionRepoError::CorruptData);
        }
    }
    let mut claims = HashMap::new();
    for item in claim_items {
        let claim = decode_anchor_claim(&item, trip_id)?;
        if claims
            .insert(claim.value.thread_id.clone(), claim.value.anchor)
            .is_some()
        {
            return Err(DiscussionRepoError::CorruptData);
        }
    }
    if claims.len() != threads.len()
        || threads
            .iter()
            .any(|(thread_id, thread)| claims.get(thread_id) != Some(&thread.thread.anchor))
    {
        return Err(DiscussionRepoError::CorruptData);
    }
    let mut result = threads
        .into_values()
        .map(|thread| Ok((utc(&thread.thread.last_activity_at)?, thread)))
        .collect::<Result<Vec<_>, DiscussionRepoError>>()?;
    result.sort_by(|(left_at, left), (right_at, right)| {
        right_at
            .cmp(left_at)
            .then_with(|| right.thread.id.cmp(&left.thread.id))
    });
    Ok(result.into_iter().map(|(_, thread)| thread).collect())
}

async fn load_discussion_meta(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<Option<Loaded<DiscussionMetaRecord>>, DiscussionRepoError> {
    repo.discussion_get(&trip_pk(trip_id), DISCUSSION_META_SK)
        .await?
        .map(|item| decode_discussion_meta(&item, trip_id))
        .transpose()
}

async fn load_anchor_claim(
    repo: &DynamoUserRepo,
    trip_id: &str,
    anchor: &ThreadAnchor,
) -> Result<Option<Loaded<AnchorClaimRecord>>, DiscussionRepoError> {
    let sk = anchor_claim_sk(anchor)?;
    repo.discussion_get(&trip_pk(trip_id), &sk)
        .await?
        .map(|item| decode_anchor_claim(&item, trip_id))
        .transpose()
}

async fn load_thread(
    repo: &DynamoUserRepo,
    trip_id: &str,
    thread_id: &str,
) -> Result<LoadedThread, DiscussionRepoError> {
    let pk = trip_pk(trip_id);
    let item = repo
        .discussion_get(&pk, &thread_sk(thread_id))
        .await?
        .ok_or(DiscussionRepoError::NotFound)?;
    let thread = decode_thread(&item, trip_id)?;
    let claim = load_anchor_claim(repo, trip_id, &thread.thread.anchor)
        .await?
        .ok_or(DiscussionRepoError::CorruptData)?;
    if claim.value.thread_id != thread_id || claim.value.anchor != thread.thread.anchor {
        return Err(DiscussionRepoError::CorruptData);
    }
    Ok(thread)
}

async fn load_claimed_thread(
    repo: &DynamoUserRepo,
    trip_id: &str,
    thread_id: &str,
) -> Result<LoadedThread, DiscussionRepoError> {
    load_thread(repo, trip_id, thread_id)
        .await
        .map_err(|error| match error {
            DiscussionRepoError::NotFound => DiscussionRepoError::CorruptData,
            other => other,
        })
}

async fn load_thread_state_with_retry(
    repo: &DynamoUserRepo,
    trip_id: &str,
    thread_id: &str,
) -> Result<(LoadedThread, Vec<Loaded<Comment>>), DiscussionRepoError> {
    match load_thread_state(repo, trip_id, thread_id).await {
        Err(DiscussionRepoError::CorruptData) => load_thread_state(repo, trip_id, thread_id).await,
        result => result,
    }
}

async fn load_thread_state(
    repo: &DynamoUserRepo,
    trip_id: &str,
    thread_id: &str,
) -> Result<(LoadedThread, Vec<Loaded<Comment>>), DiscussionRepoError> {
    let thread = load_thread(repo, trip_id, thread_id).await?;
    let comments = load_comments(repo, trip_id, &thread).await?;
    Ok((thread, comments))
}

async fn load_comments(
    repo: &DynamoUserRepo,
    trip_id: &str,
    thread: &LoadedThread,
) -> Result<Vec<Loaded<Comment>>, DiscussionRepoError> {
    let items = repo
        .discussion_query(
            &trip_pk(trip_id),
            &comment_prefix(&thread.thread.id),
            MAX_COMMENTS_PER_THREAD,
        )
        .await?;
    if items.len() != thread.thread.comment_count as usize {
        return Err(DiscussionRepoError::CorruptData);
    }
    let mut ids = HashSet::new();
    let mut comments = Vec::with_capacity(items.len());
    for item in items {
        let comment = decode_comment(&item, trip_id, &thread.thread.id)?;
        if !ids.insert(comment.value.id.clone()) {
            return Err(DiscussionRepoError::CorruptData);
        }
        validate_comment_time(thread, &comment.value)?;
        comments.push((utc(&comment.value.created_at)?, comment));
    }
    comments.sort_by(|(left_at, left), (right_at, right)| {
        left_at
            .cmp(right_at)
            .then_with(|| left.value.id.cmp(&right.value.id))
    });
    if comments.last().map(|(at, _)| at) != Some(&utc(&thread.thread.last_activity_at)?) {
        return Err(DiscussionRepoError::CorruptData);
    }
    let comments = comments
        .into_iter()
        .map(|(_, comment)| comment)
        .collect::<Vec<_>>();
    enforce_response_limit(
        &comments
            .iter()
            .map(|comment| &comment.value)
            .collect::<Vec<_>>(),
    )?;
    Ok(comments)
}

async fn prepare_anchor_conditions(
    repo: &DynamoUserRepo,
    trip_id: &str,
    anchor: &ThreadAnchor,
) -> Result<Vec<ConditionCheck>, DiscussionRepoError> {
    match anchor {
        ThreadAnchor::Trip => {
            let meta = repo.discussion_trip_meta(trip_id).await?;
            Ok(vec![repo.trip_revision_condition(trip_id, meta.revision)])
        }
        ThreadAnchor::Candidate { candidate_id } => {
            let pk = trip_pk(trip_id);
            let sk = candidate_sk(candidate_id);
            let item = repo
                .discussion_get(&pk, &sk)
                .await?
                .ok_or(DiscussionRepoError::NotFound)?;
            let stored: Stored<Candidate> =
                decode_record(&item, &pk, &sk, CANDIDATE_ENTITY).map_err(record_error)?;
            if stored.revision == 0
                || stored.value.id != *candidate_id
                || validate_stored_candidate(trip_id, &stored.value).is_err()
            {
                return Err(DiscussionRepoError::CorruptData);
            }
            Ok(vec![repo.entity_revision_condition(
                pk,
                sk,
                CANDIDATE_ENTITY,
                stored.revision,
            )])
        }
        ThreadAnchor::Poll { poll_id } => {
            let pk = trip_pk(trip_id);
            let sk = poll_sk(poll_id);
            let item = repo
                .discussion_get(&pk, &sk)
                .await?
                .ok_or(DiscussionRepoError::NotFound)?;
            let poll = decode_poll(&item, trip_id).map_err(poll_error)?;
            if poll.poll.id != *poll_id {
                return Err(DiscussionRepoError::CorruptData);
            }
            Ok(vec![repo.entity_revision_condition(
                pk,
                sk,
                POLL_ENTITY,
                poll.revision,
            )])
        }
        ThreadAnchor::Day { day_id } => {
            prepare_plan_anchor_conditions(repo, trip_id, Some(day_id), None).await
        }
        ThreadAnchor::Stop { stop_id } => {
            prepare_plan_anchor_conditions(repo, trip_id, None, Some(stop_id)).await
        }
    }
}

async fn prepare_plan_anchor_conditions(
    repo: &DynamoUserRepo,
    trip_id: &str,
    day_id: Option<&String>,
    stop_id: Option<&String>,
) -> Result<Vec<ConditionCheck>, DiscussionRepoError> {
    let meta = repo.discussion_trip_meta(trip_id).await?;
    let plan_id = meta
        .value
        .current_plan_id
        .as_deref()
        .ok_or(DiscussionRepoError::NotFound)?;
    let version = meta
        .value
        .current_plan_version
        .ok_or(DiscussionRepoError::NotFound)?;
    let pk = trip_pk(trip_id);
    let items = repo
        .discussion_query(&pk, &format!("{}#", plan_prefix(version)), MAX_THREADS)
        .await?;
    let mut plan = None::<(Plan, u64, String)>;
    let mut days = HashMap::<String, (Day, u64, String)>::new();
    let mut stops = Vec::<(Stop, u64, String)>::new();
    let mut stop_ids = HashSet::new();
    for item in items {
        let entity = string(&item, ENTITY_TYPE).map_err(record_error)?;
        let sk = string(&item, SK).map_err(record_error)?;
        match entity.as_str() {
            PLAN_ENTITY => {
                let stored: Stored<Plan> =
                    decode_record(&item, &pk, &sk, PLAN_ENTITY).map_err(record_error)?;
                if stored.revision == 0
                    || sk != plan_sk(version)
                    || stored.value.id != plan_id
                    || stored.value.trip_id != trip_id
                    || stored.value.version != version
                    || plan.replace((stored.value, stored.revision, sk)).is_some()
                {
                    return Err(DiscussionRepoError::CorruptData);
                }
            }
            DAY_ENTITY => {
                let stored: Stored<Day> =
                    decode_record(&item, &pk, &sk, DAY_ENTITY).map_err(record_error)?;
                if stored.revision == 0
                    || stored.value.plan_id != plan_id
                    || sk != day_sk(version, &stored.value)
                    || days
                        .insert(stored.value.id.clone(), (stored.value, stored.revision, sk))
                        .is_some()
                {
                    return Err(DiscussionRepoError::CorruptData);
                }
            }
            STOP_ENTITY => {
                let stored: Stored<Stop> =
                    decode_record(&item, &pk, &sk, STOP_ENTITY).map_err(record_error)?;
                if stored.revision == 0
                    || !stored.value.seq.is_finite()
                    || stored.value.seq <= 0.0
                    || stored.value.seq.fract() != 0.0
                    || !stop_ids.insert(stored.value.id.clone())
                {
                    return Err(DiscussionRepoError::CorruptData);
                }
                stops.push((stored.value, stored.revision, sk));
            }
            _ => return Err(DiscussionRepoError::CorruptData),
        }
    }
    let (plan, plan_revision, plan_sort_key) = plan.ok_or(DiscussionRepoError::CorruptData)?;
    let canonical_days = days
        .values()
        .map(|(day, _, _)| day.clone())
        .collect::<Vec<_>>();
    let canonical_stops = stops
        .iter()
        .map(|(stop, _, _)| stop.clone())
        .collect::<Vec<_>>();
    if validate_stored_plan_graph(&plan, &canonical_days, &canonical_stops, trip_id, version)
        .is_err()
    {
        return Err(DiscussionRepoError::CorruptData);
    }
    for (stop, _, sk) in &stops {
        let day = days
            .get(&stop.day_id)
            .ok_or(DiscussionRepoError::CorruptData)?;
        if sk != &stop_sk(version, &day.0, stop) {
            return Err(DiscussionRepoError::CorruptData);
        }
    }
    let (entity, revision, sk) = if let Some(day_id) = day_id {
        let (day, revision, sk) = days.get(day_id).ok_or(DiscussionRepoError::NotFound)?;
        if day.id.as_str() != day_id.as_str() {
            return Err(DiscussionRepoError::CorruptData);
        }
        (DAY_ENTITY, *revision, sk.clone())
    } else {
        let stop_id = stop_id.ok_or(DiscussionRepoError::CorruptData)?;
        let mut found = None;
        for (stop, revision, sk) in &stops {
            if stop.id.as_str() == stop_id.as_str()
                && found.replace((*revision, sk.clone())).is_some()
            {
                return Err(DiscussionRepoError::CorruptData);
            }
        }
        let (revision, sk) = found.ok_or(DiscussionRepoError::NotFound)?;
        (STOP_ENTITY, revision, sk)
    };
    Ok(vec![
        repo.discussion_current_plan_condition(trip_id, meta.revision, plan_id, version),
        repo.entity_revision_condition(pk.clone(), plan_sort_key, PLAN_ENTITY, plan_revision),
        repo.entity_revision_condition(pk, sk, entity, revision),
    ])
}

fn validate_comment_time(
    thread: &LoadedThread,
    comment: &Comment,
) -> Result<(), DiscussionRepoError> {
    let created = utc(&comment.created_at)?;
    if created < utc(&thread.created_at)? || created > utc(&thread.thread.last_activity_at)? {
        Err(DiscussionRepoError::CorruptData)
    } else {
        Ok(())
    }
}

fn reaction_is_active(comment: &Comment, emoji: &str, user_id: &str) -> bool {
    comment
        .reactions
        .iter()
        .find(|reaction| reaction.emoji == emoji)
        .is_some_and(|reaction| reaction.user_ids.iter().any(|user| user == user_id))
}

fn apply_reaction(comment: &mut Comment, emoji: &str, user_id: &str, active: bool) {
    if let Some(reaction) = comment
        .reactions
        .iter_mut()
        .find(|reaction| reaction.emoji == emoji)
    {
        reaction.user_ids.retain(|user| user != user_id);
        if active {
            reaction.user_ids.push(user_id.to_string());
            reaction.user_ids.sort();
        }
    } else if active {
        comment.reactions.push(Reaction {
            emoji: emoji.to_string(),
            user_ids: vec![user_id.to_string()],
        });
    }
    comment
        .reactions
        .retain(|reaction| !reaction.user_ids.is_empty());
    comment
        .reactions
        .sort_by(|left, right| left.emoji.cmp(&right.emoji));
}

fn enforce_response_limit<T: serde::Serialize>(value: &T) -> Result<(), DiscussionRepoError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| DiscussionRepoError::CorruptData)?
        .len();
    if bytes > MAX_DISCUSSION_BYTES {
        Err(DiscussionRepoError::SafetyLimitExceeded)
    } else {
        Ok(())
    }
}

fn utc(value: &str) -> Result<DateTime<chrono::FixedOffset>, DiscussionRepoError> {
    let timestamp =
        DateTime::parse_from_rfc3339(value).map_err(|_| DiscussionRepoError::CorruptData)?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err(DiscussionRepoError::CorruptData);
    }
    Ok(timestamp)
}
