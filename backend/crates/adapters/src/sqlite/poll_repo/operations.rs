//! Bounded poll reads and transaction-composed lifecycle operations.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, SecondsFormat};
use itinera_core::{
    domain::{
        poll::{Poll, PollKind, PollOption, PollStatus, PollVote},
        proposal::{Proposal, ProposalDecision, ProposalRoute, ProposalStatus},
        trip::TripRole,
    },
    ports::{
        authorization::TripAuthorizationContext,
        clock::Clock,
        poll::{NewDecisionPoll, NewPlanChangePoll, PollRepoError},
        proposal::{ProposalApplicationIds, ProposalRepoError},
        trip::TripRepoError,
    },
    services::{
        polls::{
            ADOPT_LABEL, BELOW_QUORUM_NOTE, KEEP_LABEL, KEEP_NOTE, NO_MAJORITY_NOTE, STALE_NOTE,
            TIE_NOTE, decisive_winner, validate_stored_poll,
        },
        proposals::validate_stored_proposal,
    },
};
use sqlx::{Sqlite, Transaction};

use crate::sqlite::{
    SqliteDb,
    codec::{checked_revision, next_revision, validate_id},
    proposal_repo::{
        operations::{
            current_plan_version, ensure_projection as ensure_proposal_projection, insert_proposal,
            load_proposal, load_proposals, update_proposal, validate_proposal_plan_links,
        },
        publication,
        records::LoadedProposal,
    },
    trip_repo::access::{RequiredRole, authorize, validate_trip_aggregate},
};

use super::records::{
    BallotOptionRow, BallotRow, LoadedBallot, LoadedPoll, MAX_POLL_RESPONSE_BYTES, MAX_POLLS,
    POLL_QUERY_LIMIT, PollOptionRow, PollRow, encode_kind, encode_status,
};

const MAX_NORMALIZED_POLL_SCAN_BYTES: usize = 128 * 1024 * 1024;

pub(in crate::sqlite) async fn list_polls(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<Poll>, PollRepoError> {
    validate_requested_id(trip_id)?;
    let mut transaction = db.pool().begin().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::AnyMember,
    )
    .await
    .map_err(map_trip_error)?;
    validate_trip_aggregate(&mut transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    let polls = load_polls(&mut transaction, trip_id).await?;
    validate_poll_links(&mut transaction, trip_id, &polls).await?;
    let result = polls.into_iter().map(|loaded| loaded.poll).collect();
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(in crate::sqlite) async fn create_decision_poll(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    new: NewDecisionPoll,
) -> Result<Poll, PollRepoError> {
    validate_requested_id(trip_id)?;
    let actor = authorization
        .human_user_id()
        .ok_or(PollRepoError::Forbidden)?;
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Editor,
    )
    .await
    .map_err(map_trip_error)?;
    validate_trip_aggregate(&mut transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    let polls = load_polls(&mut transaction, trip_id).await?;
    validate_poll_links(&mut transaction, trip_id, &polls).await?;
    if polls.iter().any(|poll| poll.poll.id == new.id) {
        return Err(PollRepoError::Conflict);
    }
    let quorum = eligible_voter_count(&mut transaction, trip_id)
        .await?
        .div_ceil(2);
    let poll = Poll {
        id: new.id,
        trip_id: trip_id.to_string(),
        created_by: actor.0.clone(),
        kind: PollKind::Decision,
        title: new.title,
        description: new.description,
        options: new.options,
        opens_at: None,
        closes_at: new.closes_at,
        decided_at: None,
        quorum,
        allow_multi: new.allow_multi,
        status: PollStatus::Open,
        votes: Vec::new(),
        resolution_note: None,
    };
    validate_stored_poll(trip_id, &poll, &new.created_at).map_err(corrupt)?;
    ensure_poll_projection(&polls, None, &poll)?;
    insert_poll(&mut transaction, trip_id, &poll, &new.created_at, None, 1).await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(poll)
}

pub(in crate::sqlite) async fn create_proposal_poll(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    mut proposal: Proposal,
    new: NewPlanChangePoll,
    application_ids: ProposalApplicationIds,
) -> Result<Proposal, PollRepoError> {
    validate_requested_id(trip_id)?;
    let actor = authorization
        .human_user_id()
        .ok_or(PollRepoError::Forbidden)?;
    if proposal.trip_id != trip_id
        || proposal.created_by != actor.0
        || proposal.route != ProposalRoute::Poll
        || proposal.status != ProposalStatus::Pending
        || proposal.decided_by.is_some()
        || proposal.rejection_reason.is_some()
        || proposal.created_at != new.created_at
    {
        return Err(PollRepoError::CorruptData);
    }
    proposal.decided_by = Some(ProposalDecision::Poll {
        poll_id: new.poll_id.clone(),
    });
    validate_stored_proposal(trip_id, &proposal).map_err(corrupt)?;

    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Editor,
    )
    .await
    .map_err(map_trip_error)?;
    validate_trip_aggregate(&mut transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    let proposals = load_proposals(&mut transaction, trip_id)
        .await
        .map_err(map_proposal_error)?;
    validate_proposal_plan_links(&mut transaction, trip_id, &proposals)
        .await
        .map_err(map_proposal_error)?;
    let polls = load_polls(&mut transaction, trip_id).await?;
    validate_poll_links(&mut transaction, trip_id, &polls).await?;
    if proposals
        .iter()
        .any(|stored| stored.value.id == proposal.id)
        || polls.iter().any(|poll| poll.poll.id == new.poll_id)
    {
        return Err(PollRepoError::Conflict);
    }
    if current_plan_version(&mut transaction, trip_id)
        .await
        .map_err(map_proposal_error)?
        != proposal.change_set.base_plan_version
    {
        return Err(PollRepoError::Conflict);
    }
    let quorum = eligible_voter_count(&mut transaction, trip_id)
        .await?
        .div_ceil(2);
    let poll = plan_change_poll(trip_id, actor, &proposal, &new, quorum);
    validate_stored_poll(trip_id, &poll, &new.created_at).map_err(corrupt)?;
    let mut projected_terminal = poll.clone();
    close_poll_state(
        &mut projected_terminal,
        &new.closes_at,
        PollStatus::Passed,
        None,
    );
    let terminal_poll_write_bytes =
        publication::terminal_poll_write_bytes(&projected_terminal, &new.created_at)
            .map_err(map_proposal_error)?;
    publication::preflight(
        &mut transaction,
        trip_id,
        &proposal,
        &new.created_at,
        &application_ids,
        terminal_poll_write_bytes,
    )
    .await
    .map_err(map_proposal_error)?;
    ensure_proposal_projection(&proposals, None, &proposal).map_err(map_proposal_error)?;
    ensure_poll_projection(&polls, None, &poll)?;
    insert_proposal(&mut transaction, trip_id, &proposal, 1)
        .await
        .map_err(map_proposal_error)?;
    insert_poll(&mut transaction, trip_id, &poll, &new.created_at, None, 1).await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(proposal)
}

pub(in crate::sqlite) async fn route_proposal_to_poll(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    proposal_id: &str,
    new: NewPlanChangePoll,
    application_ids: ProposalApplicationIds,
) -> Result<Poll, PollRepoError> {
    validate_requested_id(trip_id)?;
    validate_requested_id(proposal_id)?;
    let actor = authorization
        .human_user_id()
        .ok_or(PollRepoError::Forbidden)?;
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Leader,
    )
    .await
    .map_err(map_trip_error)?;
    validate_trip_aggregate(&mut transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    let proposals = load_proposals(&mut transaction, trip_id)
        .await
        .map_err(map_proposal_error)?;
    validate_proposal_plan_links(&mut transaction, trip_id, &proposals)
        .await
        .map_err(map_proposal_error)?;
    let polls = load_polls(&mut transaction, trip_id).await?;
    validate_poll_links(&mut transaction, trip_id, &polls).await?;
    let stored = proposals
        .iter()
        .find(|proposal| proposal.value.id == proposal_id)
        .cloned()
        .ok_or(PollRepoError::NotFound)?;
    if let Some(ProposalDecision::Poll { poll_id }) = &stored.value.decided_by {
        let prior = polls
            .iter()
            .find(|poll| poll.poll.id == *poll_id)
            .ok_or(PollRepoError::CorruptData)?;
        if stored.value.status != ProposalStatus::Pending
            || matches!(
                prior.poll.status,
                PollStatus::Draft | PollStatus::Scheduled | PollStatus::Open
            )
        {
            let result = prior.poll.clone();
            db.commit(transaction).await.map_err(unavailable)?;
            return Ok(result);
        }
    } else if stored.value.decided_by.is_some() {
        return Err(PollRepoError::Conflict);
    }
    if stored.value.status != ProposalStatus::Pending {
        return Err(PollRepoError::Conflict);
    }
    let replacement_created = utc(&new.created_at)?;
    if replacement_created < utc(&stored.value.created_at)? {
        return Err(PollRepoError::Conflict);
    }
    if let Some(ProposalDecision::Poll { poll_id }) = &stored.value.decided_by {
        let prior = polls
            .iter()
            .find(|poll| poll.poll.id == *poll_id)
            .ok_or(PollRepoError::CorruptData)?;
        let historical_no_decision = matches!(prior.poll.status, PollStatus::Expired)
            || prior.poll.status == PollStatus::Failed
                && decisive_winner(&prior.poll).map_err(corrupt)?.is_none();
        let prior_decided = prior
            .poll
            .decided_at
            .as_deref()
            .ok_or(PollRepoError::CorruptData)
            .and_then(utc)?;
        if !historical_no_decision || replacement_created < prior_decided {
            return Err(PollRepoError::Conflict);
        }
    }
    let replaces_poll_id = match &stored.value.decided_by {
        Some(ProposalDecision::Poll { poll_id }) => Some(poll_id.clone()),
        None => None,
        Some(ProposalDecision::Leader { .. }) => return Err(PollRepoError::Conflict),
    };
    let current = current_plan_version(&mut transaction, trip_id)
        .await
        .map_err(map_proposal_error)?;
    if current < stored.value.change_set.base_plan_version {
        return Err(PollRepoError::CorruptData);
    }
    if current > stored.value.change_set.base_plan_version {
        return Err(PollRepoError::Conflict);
    }
    if polls.iter().any(|poll| poll.poll.id == new.poll_id) {
        return Err(PollRepoError::Conflict);
    }
    let mut proposal = stored.value;
    proposal.route = ProposalRoute::Poll;
    proposal.decided_by = Some(ProposalDecision::Poll {
        poll_id: new.poll_id.clone(),
    });
    proposal.rejection_reason = None;
    validate_stored_proposal(trip_id, &proposal).map_err(corrupt)?;
    let quorum = eligible_voter_count(&mut transaction, trip_id)
        .await?
        .div_ceil(2);
    let poll = plan_change_poll(trip_id, actor, &proposal, &new, quorum);
    validate_stored_poll(trip_id, &poll, &new.created_at).map_err(corrupt)?;
    let mut projected_terminal = poll.clone();
    close_poll_state(
        &mut projected_terminal,
        &new.closes_at,
        PollStatus::Passed,
        None,
    );
    let terminal_poll_write_bytes =
        publication::terminal_poll_write_bytes(&projected_terminal, &new.created_at)
            .map_err(map_proposal_error)?;
    publication::preflight(
        &mut transaction,
        trip_id,
        &proposal,
        &new.created_at,
        &application_ids,
        terminal_poll_write_bytes,
    )
    .await
    .map_err(map_proposal_error)?;
    ensure_proposal_projection(&proposals, Some(proposal_id), &proposal)
        .map_err(map_proposal_error)?;
    ensure_poll_projection(&polls, None, &poll)?;
    update_proposal(&mut transaction, trip_id, &proposal, stored.revision)
        .await
        .map_err(map_proposal_error)?;
    insert_poll(
        &mut transaction,
        trip_id,
        &poll,
        &new.created_at,
        replaces_poll_id.as_deref(),
        1,
    )
    .await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(poll)
}

pub(in crate::sqlite) async fn open_poll(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    poll_id: &str,
    clock: &dyn Clock,
) -> Result<Poll, PollRepoError> {
    validate_requested_id(trip_id)?;
    validate_requested_id(poll_id)?;
    let actor = authorization
        .human_user_id()
        .ok_or(PollRepoError::Forbidden)?;
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    let (opened, _) = canonical_transaction_time(&clock.now())?;
    let role = authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Editor,
    )
    .await
    .map_err(map_trip_error)?;
    validate_trip_aggregate(&mut transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    let mut polls = load_polls(&mut transaction, trip_id).await?;
    validate_poll_links(&mut transaction, trip_id, &polls).await?;
    let index = polls
        .iter()
        .position(|poll| poll.poll.id == poll_id)
        .ok_or(PollRepoError::NotFound)?;
    if role != TripRole::Leader && polls[index].poll.created_by != actor.0 {
        return Err(PollRepoError::Forbidden);
    }
    match polls[index].poll.status {
        PollStatus::Open => {
            let result = polls[index].poll.clone();
            db.commit(transaction).await.map_err(unavailable)?;
            return Ok(result);
        }
        PollStatus::Draft | PollStatus::Scheduled => {}
        PollStatus::Passed | PollStatus::Failed | PollStatus::Expired => {
            return Err(PollRepoError::Conflict);
        }
    }
    if opened < utc(&polls[index].created_at)? || opened >= utc(&polls[index].poll.closes_at)? {
        return Err(PollRepoError::Conflict);
    }
    let previous_revision = polls[index].revision;
    polls[index].poll.status = PollStatus::Open;
    polls[index].poll.opens_at = None;
    validate_stored_poll(trip_id, &polls[index].poll, &polls[index].created_at).map_err(corrupt)?;
    ensure_loaded_poll_budget(&polls)?;
    update_poll(&mut transaction, &polls[index], previous_revision).await?;
    let result = polls[index].poll.clone();
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(in crate::sqlite) async fn cast_vote(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    poll_id: &str,
    option_ids: &[String],
    clock: &dyn Clock,
) -> Result<Poll, PollRepoError> {
    validate_requested_id(trip_id)?;
    validate_requested_id(poll_id)?;
    let actor = authorization
        .human_user_id()
        .ok_or(PollRepoError::Forbidden)?;
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    let (voted, voted_at) = canonical_transaction_time(&clock.now())?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Editor,
    )
    .await
    .map_err(map_trip_error)?;
    validate_trip_aggregate(&mut transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    let mut polls = load_polls(&mut transaction, trip_id).await?;
    validate_poll_links(&mut transaction, trip_id, &polls).await?;
    let index = polls
        .iter()
        .position(|poll| poll.poll.id == poll_id)
        .ok_or(PollRepoError::NotFound)?;
    let mut desired = option_ids.to_vec();
    desired.sort();
    if !valid_ballot(&polls[index].poll, &desired) {
        return Err(PollRepoError::InvalidVote);
    }
    let existing = load_actor_ballot(&mut transaction, trip_id, &polls[index], &actor.0).await?;
    if existing
        .as_ref()
        .is_some_and(|ballot| ballot.option_ids == desired)
        || existing.is_none() && desired.is_empty()
    {
        let result = polls[index].poll.clone();
        db.commit(transaction).await.map_err(unavailable)?;
        return Ok(result);
    }
    if polls[index].poll.status != PollStatus::Open
        || voted < utc(&polls[index].created_at)?
        || voted >= utc(&polls[index].poll.closes_at)?
    {
        return Err(PollRepoError::Conflict);
    }
    let previous_revision = polls[index].revision;
    polls[index]
        .poll
        .votes
        .retain(|vote| vote.user_id != actor.0);
    polls[index]
        .poll
        .votes
        .extend(desired.iter().map(|option_id| PollVote {
            user_id: actor.0.clone(),
            option_id: option_id.clone(),
            at: voted_at.clone(),
        }));
    polls[index].poll.votes.sort_by(|left, right| {
        left.user_id
            .cmp(&right.user_id)
            .then_with(|| left.option_id.cmp(&right.option_id))
    });
    validate_stored_poll(trip_id, &polls[index].poll, &polls[index].created_at).map_err(corrupt)?;
    ensure_loaded_poll_budget(&polls)?;
    update_poll(&mut transaction, &polls[index], previous_revision).await?;
    write_ballot(
        &mut transaction,
        trip_id,
        poll_id,
        &actor.0,
        &desired,
        &voted_at,
        existing.as_ref(),
    )
    .await?;
    let result = polls[index].poll.clone();
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(in crate::sqlite) async fn close_poll(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    poll_id: &str,
    decided_at: &str,
    application_ids: ProposalApplicationIds,
) -> Result<Poll, PollRepoError> {
    validate_requested_id(trip_id)?;
    validate_requested_id(poll_id)?;
    let actor = authorization
        .human_user_id()
        .ok_or(PollRepoError::Forbidden)?;
    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
    authorize(
        &mut transaction,
        trip_id,
        authorization,
        RequiredRole::Leader,
    )
    .await
    .map_err(map_trip_error)?;
    validate_trip_aggregate(&mut transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    let mut polls = load_polls(&mut transaction, trip_id).await?;
    validate_poll_links(&mut transaction, trip_id, &polls).await?;
    let index = polls
        .iter()
        .position(|poll| poll.poll.id == poll_id)
        .ok_or(PollRepoError::NotFound)?;
    if polls[index].poll.status.is_terminal() {
        let result = polls[index].poll.clone();
        db.commit(transaction).await.map_err(unavailable)?;
        return Ok(result);
    }
    if polls[index].poll.status != PollStatus::Open {
        return Err(PollRepoError::Conflict);
    }
    let decided = utc(decided_at)?;
    if decided < utc(&polls[index].created_at)?
        || polls[index]
            .poll
            .votes
            .iter()
            .any(|vote| utc(&vote.at).is_ok_and(|voted| voted > decided))
    {
        return Err(PollRepoError::Conflict);
    }
    let previous_revision = polls[index].revision;
    let voters = polls[index]
        .poll
        .votes
        .iter()
        .map(|vote| vote.user_id.as_str())
        .collect::<HashSet<_>>()
        .len();
    if voters < usize::try_from(polls[index].poll.quorum).map_err(corrupt)? {
        close_poll_state(
            &mut polls[index].poll,
            decided_at,
            PollStatus::Expired,
            Some(BELOW_QUORUM_NOTE),
        );
        validate_stored_poll(trip_id, &polls[index].poll, &polls[index].created_at)
            .map_err(corrupt)?;
        ensure_loaded_poll_budget(&polls)?;
        update_poll(&mut transaction, &polls[index], previous_revision).await?;
    } else {
        let winner = decisive_winner(&polls[index].poll)
            .map_err(corrupt)?
            .cloned();
        match (polls[index].poll.kind, winner) {
            (PollKind::Decision, Some(_)) => {
                close_poll_state(&mut polls[index].poll, decided_at, PollStatus::Passed, None);
                validate_stored_poll(trip_id, &polls[index].poll, &polls[index].created_at)
                    .map_err(corrupt)?;
                ensure_loaded_poll_budget(&polls)?;
                update_poll(&mut transaction, &polls[index], previous_revision).await?;
            }
            (PollKind::Decision, None) | (PollKind::PlanChange, None) => {
                let note = no_decision_note(&polls[index].poll)?;
                close_poll_state(
                    &mut polls[index].poll,
                    decided_at,
                    PollStatus::Failed,
                    Some(note),
                );
                validate_stored_poll(trip_id, &polls[index].poll, &polls[index].created_at)
                    .map_err(corrupt)?;
                ensure_loaded_poll_budget(&polls)?;
                update_poll(&mut transaction, &polls[index], previous_revision).await?;
            }
            (PollKind::PlanChange, Some(winner)) => {
                close_plan_poll(
                    &mut transaction,
                    trip_id,
                    actor,
                    &mut polls,
                    ClosePlanCommand {
                        index,
                        previous_revision,
                        winner,
                        decided_at,
                        application_ids: &application_ids,
                    },
                )
                .await?;
            }
        }
    }
    validate_poll_links(&mut transaction, trip_id, &polls).await?;
    let result = load_poll(&mut transaction, trip_id, poll_id).await?.poll;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

struct ClosePlanCommand<'a> {
    index: usize,
    previous_revision: i64,
    winner: PollOption,
    decided_at: &'a str,
    application_ids: &'a ProposalApplicationIds,
}

async fn close_plan_poll(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    actor: &itinera_core::domain::user::UserId,
    polls: &mut [LoadedPoll],
    command: ClosePlanCommand<'_>,
) -> Result<(), PollRepoError> {
    let ClosePlanCommand {
        index,
        previous_revision,
        winner,
        decided_at,
        application_ids,
    } = command;
    let proposal_id = polls[index]
        .poll
        .options
        .iter()
        .find_map(|option| option.proposal_id.as_deref())
        .ok_or(PollRepoError::CorruptData)?;
    let stored = load_proposal(transaction, trip_id, proposal_id)
        .await
        .map_err(map_proposal_error)?
        .ok_or(PollRepoError::CorruptData)?;
    if stored.value.status != ProposalStatus::Pending
        || stored.value.route != ProposalRoute::Poll
        || stored.value.decided_by
            != Some(ProposalDecision::Poll {
                poll_id: polls[index].poll.id.clone(),
            })
    {
        return Err(PollRepoError::CorruptData);
    }
    let proposals = load_proposals(transaction, trip_id)
        .await
        .map_err(map_proposal_error)?;
    if winner.proposal_id.is_none() {
        let mut rejected = stored.value;
        rejected.status = ProposalStatus::Rejected;
        rejected.rejection_reason = Some(KEEP_NOTE.to_string());
        validate_stored_proposal(trip_id, &rejected).map_err(corrupt)?;
        ensure_proposal_projection(&proposals, Some(&rejected.id), &rejected)
            .map_err(map_proposal_error)?;
        close_poll_state(
            &mut polls[index].poll,
            decided_at,
            PollStatus::Failed,
            Some(KEEP_NOTE),
        );
        validate_stored_poll(trip_id, &polls[index].poll, &polls[index].created_at)
            .map_err(corrupt)?;
        ensure_loaded_poll_budget(polls)?;
        update_proposal(transaction, trip_id, &rejected, stored.revision)
            .await
            .map_err(map_proposal_error)?;
        update_poll(transaction, &polls[index], previous_revision).await?;
        return Ok(());
    }
    let current = current_plan_version(transaction, trip_id)
        .await
        .map_err(map_proposal_error)?;
    let base = stored.value.change_set.base_plan_version;
    if current < base {
        return Err(PollRepoError::CorruptData);
    }
    if current > base {
        let mut stale = stored.value;
        stale.status = ProposalStatus::Stale;
        validate_stored_proposal(trip_id, &stale).map_err(corrupt)?;
        ensure_proposal_projection(&proposals, Some(&stale.id), &stale)
            .map_err(map_proposal_error)?;
        close_poll_state(
            &mut polls[index].poll,
            decided_at,
            PollStatus::Failed,
            Some(STALE_NOTE),
        );
        validate_stored_poll(trip_id, &polls[index].poll, &polls[index].created_at)
            .map_err(corrupt)?;
        ensure_loaded_poll_budget(polls)?;
        update_proposal(transaction, trip_id, &stale, stored.revision)
            .await
            .map_err(map_proposal_error)?;
        update_poll(transaction, &polls[index], previous_revision).await?;
        return Ok(());
    }
    close_poll_state(&mut polls[index].poll, decided_at, PollStatus::Passed, None);
    validate_stored_poll(trip_id, &polls[index].poll, &polls[index].created_at).map_err(corrupt)?;
    ensure_loaded_poll_budget(polls)?;
    let terminal_poll_write_bytes =
        publication::terminal_poll_write_bytes(&polls[index].poll, &polls[index].created_at)
            .map_err(map_proposal_error)?;
    publication::publish(
        transaction,
        trip_id,
        actor,
        stored.value,
        Some(stored.revision),
        publication::PublicationCommand {
            decision: ProposalDecision::Poll {
                poll_id: polls[index].poll.id.clone(),
            },
            applied_at: decided_at,
            application_ids,
            terminal_poll_write_bytes,
        },
    )
    .await
    .map_err(map_proposal_error)?;
    update_poll(transaction, &polls[index], previous_revision).await
}

pub(in crate::sqlite) async fn load_polls(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<LoadedPoll>, PollRepoError> {
    let (count, response_lower_bound): (i64, i64) = sqlx::query_as(
        "SELECT \
             (SELECT COUNT(*) FROM polls WHERE trip_id = ?), \
             COALESCE((SELECT SUM( \
                 length(CAST(trip_id AS BLOB)) + length(CAST(id AS BLOB)) \
                 + length(CAST(created_by AS BLOB)) + length(CAST(kind AS BLOB)) \
                 + length(CAST(title AS BLOB)) + length(CAST(description AS BLOB)) \
                 + COALESCE(length(CAST(opens_at AS BLOB)), 0) \
                 + length(CAST(closes_at AS BLOB)) \
                 + COALESCE(length(CAST(decided_at AS BLOB)), 0) \
                 + length(CAST(status AS BLOB)) \
                 + COALESCE(length(CAST(resolution_note AS BLOB)), 0) \
             ) FROM polls WHERE trip_id = ?), 0) \
             + COALESCE((SELECT SUM( \
                 length(CAST(id AS BLOB)) + length(CAST(label AS BLOB)) \
                 + COALESCE(length(CAST(proposal_id AS BLOB)), 0) \
             ) FROM poll_options WHERE trip_id = ?), 0) \
             + COALESCE((SELECT SUM( \
                 length(CAST(user_id AS BLOB)) + length(CAST(voted_at AS BLOB)) \
             ) FROM poll_ballots WHERE trip_id = ?), 0) \
             + COALESCE((SELECT SUM( \
                 length(CAST(option_id AS BLOB)) \
             ) FROM poll_ballot_options WHERE trip_id = ?), 0)",
    )
    .bind(trip_id)
    .bind(trip_id)
    .bind(trip_id)
    .bind(trip_id)
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let count = usize::try_from(count).map_err(corrupt)?;
    let response_lower_bound = usize::try_from(response_lower_bound).map_err(corrupt)?;
    if count > MAX_POLLS || response_lower_bound > MAX_POLL_RESPONSE_BYTES {
        return Err(PollRepoError::SafetyLimitExceeded);
    }
    let normalized_bytes: i64 = sqlx::query_scalar(
        "SELECT \
             COALESCE((SELECT SUM( \
                 length(CAST(trip_id AS BLOB)) + length(CAST(id AS BLOB)) \
                 + length(CAST(created_by AS BLOB)) + length(CAST(kind AS BLOB)) \
                 + COALESCE(length(CAST(replaces_poll_id AS BLOB)), 0) \
                 + length(CAST(title AS BLOB)) + length(CAST(description AS BLOB)) \
                 + length(CAST(created_at AS BLOB)) \
                 + COALESCE(length(CAST(opens_at AS BLOB)), 0) \
                 + length(CAST(closes_at AS BLOB)) \
                 + COALESCE(length(CAST(decided_at AS BLOB)), 0) \
                 + length(CAST(status AS BLOB)) \
                 + COALESCE(length(CAST(resolution_note AS BLOB)), 0) \
             ) FROM polls WHERE trip_id = ?), 0) \
             + COALESCE((SELECT SUM( \
                 length(CAST(poll_id AS BLOB)) + length(CAST(id AS BLOB)) \
                 + length(CAST(label AS BLOB)) \
                 + COALESCE(length(CAST(proposal_id AS BLOB)), 0) \
             ) FROM poll_options WHERE trip_id = ?), 0) \
             + COALESCE((SELECT SUM( \
                 length(CAST(poll_id AS BLOB)) + length(CAST(user_id AS BLOB)) \
                 + length(CAST(voted_at AS BLOB)) \
             ) FROM poll_ballots WHERE trip_id = ?), 0) \
             + COALESCE((SELECT SUM( \
                 length(CAST(poll_id AS BLOB)) + length(CAST(user_id AS BLOB)) \
                 + length(CAST(option_id AS BLOB)) \
             ) FROM poll_ballot_options WHERE trip_id = ?), 0)",
    )
    .bind(trip_id)
    .bind(trip_id)
    .bind(trip_id)
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if usize::try_from(normalized_bytes).map_err(corrupt)? > MAX_NORMALIZED_POLL_SCAN_BYTES {
        return Err(PollRepoError::SafetyLimitExceeded);
    }
    let rows = sqlx::query_as::<_, PollRow>(
        "SELECT trip_id AS poll_trip_id, id AS poll_id, \
                created_by AS poll_created_by, kind AS poll_kind, \
                replaces_poll_id AS poll_replaces_poll_id, title AS poll_title, \
                description AS poll_description, created_at AS poll_created_at, \
                opens_at AS poll_opens_at, closes_at AS poll_closes_at, \
                decided_at AS poll_decided_at, quorum AS poll_quorum, \
                allow_multi AS poll_allow_multi, status AS poll_status, \
                resolution_note AS poll_resolution_note, revision AS poll_revision \
         FROM polls WHERE trip_id = ? LIMIT ?",
    )
    .bind(trip_id)
    .bind(POLL_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() != count {
        return Err(PollRepoError::CorruptData);
    }
    let option_rows = sqlx::query_as::<_, PollOptionRow>(
        "SELECT trip_id AS option_trip_id, poll_id AS option_poll_id, \
                id AS option_id, position AS option_position, label AS option_label, \
                proposal_id AS option_proposal_id \
         FROM poll_options WHERE trip_id = ? \
         ORDER BY poll_id, position LIMIT 6001",
    )
    .bind(trip_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if option_rows.len() > MAX_POLLS * 6 {
        return Err(PollRepoError::CorruptData);
    }
    let ballot_rows = sqlx::query_as::<_, BallotRow>(
        "SELECT trip_id AS ballot_trip_id, poll_id AS ballot_poll_id, \
                user_id AS ballot_user_id, voted_at AS ballot_voted_at, \
                revision AS ballot_revision \
         FROM poll_ballots WHERE trip_id = ? \
         ORDER BY poll_id, user_id LIMIT 1000001",
    )
    .bind(trip_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if ballot_rows.len() > MAX_POLLS * 1_000 {
        return Err(PollRepoError::CorruptData);
    }
    let selection_rows = sqlx::query_as::<_, BallotOptionRow>(
        "SELECT trip_id AS selection_trip_id, poll_id AS selection_poll_id, \
                user_id AS selection_user_id, option_id AS selection_option_id \
         FROM poll_ballot_options WHERE trip_id = ? \
         ORDER BY poll_id, user_id, option_id LIMIT 6000001",
    )
    .bind(trip_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if selection_rows.len() > MAX_POLLS * 6_000 {
        return Err(PollRepoError::CorruptData);
    }

    assemble_polls(
        trip_id,
        rows,
        option_rows,
        ballot_rows,
        selection_rows,
        true,
    )
}

pub(in crate::sqlite) async fn load_applied_plan_polls(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<LoadedPoll>, PollRepoError> {
    load_linked_plan_polls(transaction, trip_id, true).await
}

pub(in crate::sqlite) async fn load_proposal_linked_polls(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<LoadedPoll>, PollRepoError> {
    load_linked_plan_polls(transaction, trip_id, false).await
}

async fn load_linked_plan_polls(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    applied_only: bool,
) -> Result<Vec<LoadedPoll>, PollRepoError> {
    let applied_only = i64::from(applied_only);
    let (count, normalized_bytes): (i64, i64) = sqlx::query_as(
        "WITH relevant(trip_id, id) AS ( \
             SELECT poll.trip_id, poll.id FROM polls AS poll \
             WHERE poll.trip_id = ? \
               AND EXISTS ( \
                   SELECT 1 FROM poll_options AS link \
                   JOIN proposals AS proposal \
                     ON proposal.trip_id = link.trip_id \
                    AND proposal.id = link.proposal_id \
                   WHERE link.trip_id = poll.trip_id AND link.poll_id = poll.id \
                     AND (? = 0 OR proposal.status = 'applied') \
               ) \
         ) \
         SELECT \
             (SELECT COUNT(*) FROM relevant), \
             COALESCE((SELECT SUM( \
                 length(CAST(poll.trip_id AS BLOB)) + length(CAST(poll.id AS BLOB)) + \
                 length(CAST(poll.created_by AS BLOB)) + length(CAST(poll.kind AS BLOB)) + \
                 COALESCE(length(CAST(poll.replaces_poll_id AS BLOB)), 0) + \
                 length(CAST(poll.title AS BLOB)) + length(CAST(poll.description AS BLOB)) + \
                 length(CAST(poll.created_at AS BLOB)) + \
                 COALESCE(length(CAST(poll.opens_at AS BLOB)), 0) + \
                 length(CAST(poll.closes_at AS BLOB)) + \
                 COALESCE(length(CAST(poll.decided_at AS BLOB)), 0) + \
                 length(CAST(poll.status AS BLOB)) + \
                 COALESCE(length(CAST(poll.resolution_note AS BLOB)), 0) \
             ) FROM polls AS poll JOIN relevant \
               ON relevant.trip_id = poll.trip_id AND relevant.id = poll.id), 0) + \
             COALESCE((SELECT SUM( \
                 length(CAST(option.poll_id AS BLOB)) + length(CAST(option.id AS BLOB)) + \
                 length(CAST(option.label AS BLOB)) + \
                 COALESCE(length(CAST(option.proposal_id AS BLOB)), 0) \
             ) FROM poll_options AS option JOIN relevant \
               ON relevant.trip_id = option.trip_id AND relevant.id = option.poll_id), 0) + \
             COALESCE((SELECT SUM( \
                 length(CAST(ballot.poll_id AS BLOB)) + length(CAST(ballot.user_id AS BLOB)) + \
                 length(CAST(ballot.voted_at AS BLOB)) \
             ) FROM poll_ballots AS ballot JOIN relevant \
               ON relevant.trip_id = ballot.trip_id AND relevant.id = ballot.poll_id), 0) + \
             COALESCE((SELECT SUM( \
                 length(CAST(selection.poll_id AS BLOB)) + \
                 length(CAST(selection.user_id AS BLOB)) + \
                 length(CAST(selection.option_id AS BLOB)) \
             ) FROM poll_ballot_options AS selection JOIN relevant \
               ON relevant.trip_id = selection.trip_id AND relevant.id = selection.poll_id), 0)",
    )
    .bind(trip_id)
    .bind(applied_only)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let response_lower_bound: i64 = sqlx::query_scalar(
        "WITH relevant(trip_id, id) AS ( \
             SELECT poll.trip_id, poll.id FROM polls AS poll \
             WHERE poll.trip_id = ? \
               AND EXISTS ( \
                   SELECT 1 FROM poll_options AS link \
                   JOIN proposals AS proposal \
                     ON proposal.trip_id = link.trip_id \
                    AND proposal.id = link.proposal_id \
                   WHERE link.trip_id = poll.trip_id AND link.poll_id = poll.id \
                     AND (? = 0 OR proposal.status = 'applied') \
               ) \
         ) \
         SELECT \
             COALESCE((SELECT SUM( \
                 length(CAST(poll.trip_id AS BLOB)) + length(CAST(poll.id AS BLOB)) + \
                 length(CAST(poll.created_by AS BLOB)) + length(CAST(poll.kind AS BLOB)) + \
                 length(CAST(poll.title AS BLOB)) + length(CAST(poll.description AS BLOB)) + \
                 COALESCE(length(CAST(poll.opens_at AS BLOB)), 0) + \
                 length(CAST(poll.closes_at AS BLOB)) + \
                 COALESCE(length(CAST(poll.decided_at AS BLOB)), 0) + \
                 length(CAST(poll.status AS BLOB)) + \
                 COALESCE(length(CAST(poll.resolution_note AS BLOB)), 0) \
             ) FROM polls AS poll JOIN relevant \
               ON relevant.trip_id = poll.trip_id AND relevant.id = poll.id), 0) + \
             COALESCE((SELECT SUM( \
                 length(CAST(option.id AS BLOB)) + length(CAST(option.label AS BLOB)) + \
                 COALESCE(length(CAST(option.proposal_id AS BLOB)), 0) \
             ) FROM poll_options AS option JOIN relevant \
               ON relevant.trip_id = option.trip_id AND relevant.id = option.poll_id), 0) + \
             COALESCE((SELECT SUM( \
                 length(CAST(ballot.user_id AS BLOB)) + \
                 length(CAST(ballot.voted_at AS BLOB)) \
             ) FROM poll_ballots AS ballot JOIN relevant \
               ON relevant.trip_id = ballot.trip_id AND relevant.id = ballot.poll_id), 0) + \
             COALESCE((SELECT SUM(length(CAST(selection.option_id AS BLOB))) \
             FROM poll_ballot_options AS selection JOIN relevant \
               ON relevant.trip_id = selection.trip_id AND relevant.id = selection.poll_id), 0)",
    )
    .bind(trip_id)
    .bind(applied_only)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let count = usize::try_from(count).map_err(corrupt)?;
    let normalized_bytes = usize::try_from(normalized_bytes).map_err(corrupt)?;
    let response_lower_bound = usize::try_from(response_lower_bound).map_err(corrupt)?;
    if count > MAX_POLLS
        || normalized_bytes > MAX_NORMALIZED_POLL_SCAN_BYTES
        || response_lower_bound > MAX_POLL_RESPONSE_BYTES
    {
        return Err(PollRepoError::CorruptData);
    }
    let rows = sqlx::query_as::<_, PollRow>(
        "SELECT poll.trip_id AS poll_trip_id, poll.id AS poll_id, \
                poll.created_by AS poll_created_by, poll.kind AS poll_kind, \
                poll.replaces_poll_id AS poll_replaces_poll_id, \
                poll.title AS poll_title, poll.description AS poll_description, \
                poll.created_at AS poll_created_at, poll.opens_at AS poll_opens_at, \
                poll.closes_at AS poll_closes_at, poll.decided_at AS poll_decided_at, \
                poll.quorum AS poll_quorum, poll.allow_multi AS poll_allow_multi, \
                poll.status AS poll_status, poll.resolution_note AS poll_resolution_note, \
                poll.revision AS poll_revision \
         FROM polls AS poll \
         WHERE poll.trip_id = ? \
           AND EXISTS ( \
               SELECT 1 FROM poll_options AS link \
               JOIN proposals AS proposal \
                 ON proposal.trip_id = link.trip_id AND proposal.id = link.proposal_id \
               WHERE link.trip_id = poll.trip_id AND link.poll_id = poll.id \
                 AND (? = 0 OR proposal.status = 'applied') \
           ) LIMIT ?",
    )
    .bind(trip_id)
    .bind(applied_only)
    .bind(POLL_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() != count {
        return Err(PollRepoError::CorruptData);
    }
    let option_rows = sqlx::query_as::<_, PollOptionRow>(
        "SELECT option.trip_id AS option_trip_id, option.poll_id AS option_poll_id, \
                option.id AS option_id, option.position AS option_position, \
                option.label AS option_label, option.proposal_id AS option_proposal_id \
         FROM poll_options AS option JOIN polls AS poll \
           ON poll.trip_id = option.trip_id AND poll.id = option.poll_id \
         WHERE poll.trip_id = ? \
           AND EXISTS ( \
               SELECT 1 FROM poll_options AS link \
               JOIN proposals AS proposal \
                 ON proposal.trip_id = link.trip_id AND proposal.id = link.proposal_id \
               WHERE link.trip_id = poll.trip_id AND link.poll_id = poll.id \
                 AND (? = 0 OR proposal.status = 'applied') \
           ) ORDER BY option.poll_id, option.position LIMIT 6001",
    )
    .bind(trip_id)
    .bind(applied_only)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if option_rows.len() > count.saturating_mul(6) {
        return Err(PollRepoError::CorruptData);
    }
    let ballot_rows = sqlx::query_as::<_, BallotRow>(
        "SELECT ballot.trip_id AS ballot_trip_id, ballot.poll_id AS ballot_poll_id, \
                ballot.user_id AS ballot_user_id, ballot.voted_at AS ballot_voted_at, \
                ballot.revision AS ballot_revision \
         FROM poll_ballots AS ballot JOIN polls AS poll \
           ON poll.trip_id = ballot.trip_id AND poll.id = ballot.poll_id \
         WHERE poll.trip_id = ? \
           AND EXISTS ( \
               SELECT 1 FROM poll_options AS link \
               JOIN proposals AS proposal \
                 ON proposal.trip_id = link.trip_id AND proposal.id = link.proposal_id \
               WHERE link.trip_id = poll.trip_id AND link.poll_id = poll.id \
                 AND (? = 0 OR proposal.status = 'applied') \
           ) ORDER BY ballot.poll_id, ballot.user_id LIMIT 1000001",
    )
    .bind(trip_id)
    .bind(applied_only)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if ballot_rows.len() > count.saturating_mul(1_000) {
        return Err(PollRepoError::CorruptData);
    }
    let selection_rows = sqlx::query_as::<_, BallotOptionRow>(
        "SELECT selection.trip_id AS selection_trip_id, \
                selection.poll_id AS selection_poll_id, \
                selection.user_id AS selection_user_id, \
                selection.option_id AS selection_option_id \
         FROM poll_ballot_options AS selection JOIN polls AS poll \
           ON poll.trip_id = selection.trip_id AND poll.id = selection.poll_id \
         WHERE poll.trip_id = ? \
           AND EXISTS ( \
               SELECT 1 FROM poll_options AS link \
               JOIN proposals AS proposal \
                 ON proposal.trip_id = link.trip_id AND proposal.id = link.proposal_id \
               WHERE link.trip_id = poll.trip_id AND link.poll_id = poll.id \
                 AND (? = 0 OR proposal.status = 'applied') \
           ) ORDER BY selection.poll_id, selection.user_id, selection.option_id \
         LIMIT 6000001",
    )
    .bind(trip_id)
    .bind(applied_only)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if selection_rows.len() > count.saturating_mul(6_000) {
        return Err(PollRepoError::CorruptData);
    }
    assemble_polls(
        trip_id,
        rows,
        option_rows,
        ballot_rows,
        selection_rows,
        false,
    )
}

fn assemble_polls(
    trip_id: &str,
    rows: Vec<PollRow>,
    option_rows: Vec<PollOptionRow>,
    ballot_rows: Vec<BallotRow>,
    selection_rows: Vec<BallotOptionRow>,
    enforce_response_budget: bool,
) -> Result<Vec<LoadedPoll>, PollRepoError> {
    let mut options_by_poll = HashMap::<String, Vec<PollOptionRow>>::new();
    for option in option_rows {
        options_by_poll
            .entry(option.poll_id().to_string())
            .or_default()
            .push(option);
    }
    let mut ballots_by_poll = HashMap::<String, Vec<BallotRow>>::new();
    for ballot in ballot_rows {
        ballots_by_poll
            .entry(ballot.poll_id().to_string())
            .or_default()
            .push(ballot);
    }
    let mut selections_by_ballot = HashMap::<(String, String), Vec<BallotOptionRow>>::new();
    for selection in selection_rows {
        selections_by_ballot
            .entry((
                selection.poll_id().to_string(),
                selection.user_id().to_string(),
            ))
            .or_default()
            .push(selection);
    }

    let mut polls = Vec::with_capacity(rows.len());
    for row in rows {
        let poll_id = row.id().to_string();
        let option_rows = options_by_poll.remove(&poll_id).unwrap_or_default();
        if option_rows.len() > 6 {
            return Err(PollRepoError::CorruptData);
        }
        let options = option_rows
            .into_iter()
            .enumerate()
            .map(|(position, option)| option.into_option(trip_id, &poll_id, position))
            .collect::<Result<Vec<_>, _>>()?;
        let ballot_rows = ballots_by_poll.remove(&poll_id).unwrap_or_default();
        if ballot_rows.len() > 1_000 {
            return Err(PollRepoError::CorruptData);
        }
        let mut votes = Vec::new();
        for ballot in ballot_rows {
            let user_id = ballot.user_id().to_string();
            let option_ids = selections_by_ballot
                .remove(&(poll_id.clone(), user_id.clone()))
                .unwrap_or_default()
                .into_iter()
                .map(|selection| selection.into_selection(trip_id, &poll_id, &user_id))
                .collect::<Result<Vec<_>, _>>()?;
            let ballot = ballot.into_ballot(trip_id, option_ids)?;
            votes.extend(ballot.option_ids.into_iter().map(|option_id| PollVote {
                user_id: ballot.user_id.clone(),
                option_id,
                at: ballot.voted_at.clone(),
            }));
        }
        votes.sort_by(|left, right| {
            left.user_id
                .cmp(&right.user_id)
                .then_with(|| left.option_id.cmp(&right.option_id))
        });
        polls.push(row.into_loaded(trip_id, options, votes)?);
    }
    if !options_by_poll.is_empty()
        || !ballots_by_poll.is_empty()
        || !selections_by_ballot.is_empty()
    {
        return Err(PollRepoError::CorruptData);
    }
    sort_polls(&mut polls)?;
    if enforce_response_budget {
        ensure_loaded_poll_budget(&polls)?;
    }
    Ok(polls)
}

pub(in crate::sqlite) async fn load_poll(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    poll_id: &str,
) -> Result<LoadedPoll, PollRepoError> {
    let row = sqlx::query_as::<_, PollRow>(
        "SELECT trip_id AS poll_trip_id, id AS poll_id, \
                created_by AS poll_created_by, kind AS poll_kind, \
                replaces_poll_id AS poll_replaces_poll_id, title AS poll_title, \
                description AS poll_description, created_at AS poll_created_at, \
                opens_at AS poll_opens_at, closes_at AS poll_closes_at, \
                decided_at AS poll_decided_at, quorum AS poll_quorum, \
                allow_multi AS poll_allow_multi, status AS poll_status, \
                resolution_note AS poll_resolution_note, revision AS poll_revision \
         FROM polls WHERE trip_id = ? AND id = ?",
    )
    .bind(trip_id)
    .bind(poll_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?
    .ok_or(PollRepoError::NotFound)?;
    let poll = load_poll_parts(transaction, trip_id, row).await?;
    validate_plan_link(transaction, trip_id, &poll).await?;
    Ok(poll)
}

async fn load_poll_parts(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    row: PollRow,
) -> Result<LoadedPoll, PollRepoError> {
    load_poll_parts_for_id(transaction, trip_id, row).await
}

async fn load_poll_parts_for_id(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    row: PollRow,
) -> Result<LoadedPoll, PollRepoError> {
    let poll_id = row.id().to_string();
    let option_rows = sqlx::query_as::<_, PollOptionRow>(
        "SELECT trip_id AS option_trip_id, poll_id AS option_poll_id, \
                id AS option_id, position AS option_position, label AS option_label, \
                proposal_id AS option_proposal_id \
         FROM poll_options WHERE trip_id = ? AND poll_id = ? \
         ORDER BY position LIMIT 7",
    )
    .bind(trip_id)
    .bind(&poll_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let options = option_rows
        .into_iter()
        .enumerate()
        .map(|(position, option)| option.into_option(trip_id, &poll_id, position))
        .collect::<Result<Vec<_>, _>>()?;
    let ballot_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM poll_ballots WHERE trip_id = ? AND poll_id = ?")
            .bind(trip_id)
            .bind(&poll_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(unavailable)?;
    if !(0..=1_000).contains(&ballot_count) {
        return Err(PollRepoError::CorruptData);
    }
    let ballot_rows = sqlx::query_as::<_, BallotRow>(
        "SELECT trip_id AS ballot_trip_id, poll_id AS ballot_poll_id, \
                user_id AS ballot_user_id, voted_at AS ballot_voted_at, \
                revision AS ballot_revision \
         FROM poll_ballots WHERE trip_id = ? AND poll_id = ? \
         ORDER BY user_id LIMIT 1001",
    )
    .bind(trip_id)
    .bind(&poll_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if i64::try_from(ballot_rows.len()).ok() != Some(ballot_count) {
        return Err(PollRepoError::CorruptData);
    }
    let selection_rows = sqlx::query_as::<_, BallotOptionRow>(
        "SELECT trip_id AS selection_trip_id, poll_id AS selection_poll_id, \
                user_id AS selection_user_id, option_id AS selection_option_id \
         FROM poll_ballot_options WHERE trip_id = ? AND poll_id = ? \
         ORDER BY user_id, option_id LIMIT 6001",
    )
    .bind(trip_id)
    .bind(&poll_id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if selection_rows.len() > 6_000 {
        return Err(PollRepoError::CorruptData);
    }
    let mut selections = HashMap::<String, Vec<BallotOptionRow>>::new();
    for selection in selection_rows {
        selections
            .entry(selection.user_id().to_string())
            .or_default()
            .push(selection);
    }
    let mut votes = Vec::new();
    for ballot in ballot_rows {
        let user_id = ballot.user_id().to_string();
        let option_ids = selections
            .remove(&user_id)
            .unwrap_or_default()
            .into_iter()
            .map(|selection| selection.into_selection(trip_id, &poll_id, &user_id))
            .collect::<Result<Vec<_>, _>>()?;
        let ballot = ballot.into_ballot(trip_id, option_ids)?;
        votes.extend(ballot.option_ids.into_iter().map(|option_id| PollVote {
            user_id: ballot.user_id.clone(),
            option_id,
            at: ballot.voted_at.clone(),
        }));
    }
    if !selections.is_empty() {
        return Err(PollRepoError::CorruptData);
    }
    votes.sort_by(|left, right| {
        left.user_id
            .cmp(&right.user_id)
            .then_with(|| left.option_id.cmp(&right.option_id))
    });
    row.into_loaded(trip_id, options, votes)
}

async fn validate_poll_links(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    polls: &[LoadedPoll],
) -> Result<(), PollRepoError> {
    let proposals = load_proposals(transaction, trip_id)
        .await
        .map_err(map_proposal_error)?;
    let plans = crate::sqlite::trip_repo::plans::load_plan_versions(transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    validate_governance_links(polls, &proposals, &plans)
}

pub(in crate::sqlite) fn validate_governance_links(
    polls: &[LoadedPoll],
    proposals: &[LoadedProposal],
    plans: &[itinera_core::domain::trip::Plan],
) -> Result<(), PollRepoError> {
    let proposals_by_id = proposals
        .iter()
        .map(|proposal| (proposal.value.id.as_str(), proposal))
        .collect::<HashMap<_, _>>();
    if proposals_by_id.len() != proposals.len() {
        return Err(PollRepoError::CorruptData);
    }
    let mut plans_by_proposal = HashMap::new();
    for (index, plan) in plans.iter().enumerate() {
        if usize::try_from(plan.version).ok() != Some(index + 1) {
            return Err(PollRepoError::CorruptData);
        }
        let Some(proposal_id) = plan.created_from_proposal_id.as_deref() else {
            if plan.version != 1 {
                return Err(PollRepoError::CorruptData);
            }
            continue;
        };
        if plan.version == 1
            || plans_by_proposal.insert(proposal_id, plan).is_some()
            || !proposals_by_id.contains_key(proposal_id)
        {
            return Err(PollRepoError::CorruptData);
        }
    }
    let mut linked = HashMap::<&str, Vec<&LoadedPoll>>::new();
    for poll in polls {
        let proposal_ids = poll
            .poll
            .options
            .iter()
            .filter_map(|option| option.proposal_id.as_deref())
            .collect::<HashSet<_>>();
        if poll.poll.kind == PollKind::Decision {
            if !proposal_ids.is_empty() || poll.replaces_poll_id.is_some() {
                return Err(PollRepoError::CorruptData);
            }
            continue;
        }
        let proposal_id = proposal_ids
            .iter()
            .copied()
            .next()
            .filter(|_| proposal_ids.len() == 1)
            .ok_or(PollRepoError::CorruptData)?;
        if !proposals_by_id.contains_key(proposal_id) {
            return Err(PollRepoError::CorruptData);
        }
        linked.entry(proposal_id).or_default().push(poll);
    }
    for proposal in proposals {
        let proposal = &proposal.value;
        let proposal_polls = linked.remove(proposal.id.as_str()).unwrap_or_default();
        let linked_plan = plans_by_proposal.remove(proposal.id.as_str());
        match (proposal.status, linked_plan) {
            (ProposalStatus::Applied, Some(plan))
                if proposal.change_set.base_plan_version.checked_add(1) == Some(plan.version) =>
            {
                let base = plans
                    .get(usize::try_from(plan.version - 1).map_err(corrupt)? - 1)
                    .ok_or(PollRepoError::CorruptData)?;
                if utc(&plan.created_at)? < utc(&proposal.created_at)?
                    || utc(&plan.created_at)? < utc(&base.created_at)?
                {
                    return Err(PollRepoError::CorruptData);
                }
            }
            (ProposalStatus::Applied, _) | (_, Some(_)) => {
                return Err(PollRepoError::CorruptData);
            }
            (_, None) => {}
        }
        if proposal.route != ProposalRoute::Poll {
            if !proposal_polls.is_empty() {
                return Err(PollRepoError::CorruptData);
            }
            continue;
        }
        let current_id = proposal_poll_id(proposal).ok_or(PollRepoError::CorruptData)?;
        let current = proposal_polls
            .iter()
            .copied()
            .find(|poll| poll.poll.id == current_id)
            .ok_or(PollRepoError::CorruptData)?;
        validate_replacement_chronology(proposal, &proposal_polls, current_id)?;
        let winner = decisive_winner(&current.poll).map_err(corrupt)?;
        if !current_poll_link_is_valid(&current.poll, proposal, winner) {
            return Err(PollRepoError::CorruptData);
        }
        validate_terminal_plan_time(proposal, &current.poll, linked_plan)?;
    }
    if linked.is_empty() && plans_by_proposal.is_empty() {
        Ok(())
    } else {
        Err(PollRepoError::CorruptData)
    }
}

fn validate_replacement_chronology(
    proposal: &Proposal,
    polls: &[&LoadedPoll],
    current_id: &str,
) -> Result<(), PollRepoError> {
    let proposal_created = utc(&proposal.created_at)?;
    let by_id = polls
        .iter()
        .map(|poll| (poll.poll.id.as_str(), *poll))
        .collect::<HashMap<_, _>>();
    if by_id.len() != polls.len() {
        return Err(PollRepoError::CorruptData);
    }
    let mut current = *by_id.get(current_id).ok_or(PollRepoError::CorruptData)?;
    let mut visited = HashSet::new();
    loop {
        if current.poll.kind != PollKind::PlanChange
            || utc(&current.created_at)? < proposal_created
            || !visited.insert(current.poll.id.as_str())
        {
            return Err(PollRepoError::CorruptData);
        }
        let Some(previous_id) = current.replaces_poll_id.as_deref() else {
            break;
        };
        let previous = *by_id.get(previous_id).ok_or(PollRepoError::CorruptData)?;
        let winner = decisive_winner(&previous.poll).map_err(corrupt)?;
        let historical_no_decision = previous.poll.status == PollStatus::Expired
            || previous.poll.status == PollStatus::Failed && winner.is_none();
        let decided = previous
            .poll
            .decided_at
            .as_deref()
            .ok_or(PollRepoError::CorruptData)
            .and_then(utc)?;
        if !historical_no_decision || decided > utc(&current.created_at)? {
            return Err(PollRepoError::CorruptData);
        }
        current = previous;
    }
    if visited.len() == polls.len() {
        Ok(())
    } else {
        Err(PollRepoError::CorruptData)
    }
}

pub(in crate::sqlite) async fn validate_plan_link(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    poll: &LoadedPoll,
) -> Result<(), PollRepoError> {
    if poll.poll.kind == PollKind::Decision {
        return Ok(());
    }
    let proposal_id = poll
        .poll
        .options
        .iter()
        .find_map(|option| option.proposal_id.as_deref())
        .ok_or(PollRepoError::CorruptData)?;
    let proposal = load_proposal(transaction, trip_id, proposal_id)
        .await
        .map_err(map_proposal_error)?
        .ok_or(PollRepoError::CorruptData)?;
    if proposal.value.route != ProposalRoute::Poll {
        return Err(PollRepoError::CorruptData);
    }
    if utc(&poll.created_at)? < utc(&proposal.value.created_at)? {
        return Err(PollRepoError::CorruptData);
    }
    let linked_plan = validate_one_proposal_plan_link(transaction, trip_id, &proposal).await?;
    let winner = decisive_winner(&poll.poll).map_err(corrupt)?;
    if current_poll_link_is_valid(&poll.poll, &proposal.value, winner) {
        validate_terminal_plan_time(&proposal.value, &poll.poll, linked_plan.as_ref())?;
        return Ok(());
    }
    let historical_no_decision = poll.poll.status == PollStatus::Expired
        || poll.poll.status == PollStatus::Failed && winner.is_none();
    let replacement_id = proposal_poll_id(&proposal.value).ok_or(PollRepoError::CorruptData)?;
    if !historical_no_decision || replacement_id == poll.poll.id {
        return Err(PollRepoError::CorruptData);
    }
    let replacement_row = sqlx::query_as::<_, PollRow>(
        "SELECT trip_id AS poll_trip_id, id AS poll_id, \
                created_by AS poll_created_by, kind AS poll_kind, \
                replaces_poll_id AS poll_replaces_poll_id, title AS poll_title, \
                description AS poll_description, created_at AS poll_created_at, \
                opens_at AS poll_opens_at, closes_at AS poll_closes_at, \
                decided_at AS poll_decided_at, quorum AS poll_quorum, \
                allow_multi AS poll_allow_multi, status AS poll_status, \
                resolution_note AS poll_resolution_note, revision AS poll_revision \
         FROM polls WHERE trip_id = ? AND id = ?",
    )
    .bind(trip_id)
    .bind(replacement_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?
    .ok_or(PollRepoError::CorruptData)?;
    let replacement = load_poll_parts(transaction, trip_id, replacement_row).await?;
    let replacement_proposal = replacement
        .poll
        .options
        .iter()
        .find_map(|option| option.proposal_id.as_deref());
    let previous_decided = poll
        .poll
        .decided_at
        .as_deref()
        .ok_or(PollRepoError::CorruptData)
        .and_then(utc)?;
    if replacement.poll.kind != PollKind::PlanChange
        || replacement_proposal != Some(proposal_id)
        || utc(&replacement.created_at)? < previous_decided
        || !current_poll_link_is_valid(
            &replacement.poll,
            &proposal.value,
            decisive_winner(&replacement.poll).map_err(corrupt)?,
        )
    {
        return Err(PollRepoError::CorruptData);
    }
    validate_terminal_plan_time(&proposal.value, &replacement.poll, linked_plan.as_ref())?;
    Ok(())
}

pub(in crate::sqlite) async fn validate_proposal_link(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    proposal: &Proposal,
) -> Result<(), PollRepoError> {
    let poll_ids = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT poll_id FROM poll_options \
         WHERE trip_id = ? AND proposal_id = ? ORDER BY poll_id LIMIT 1001",
    )
    .bind(trip_id)
    .bind(&proposal.id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if poll_ids.len() > MAX_POLLS {
        return Err(PollRepoError::CorruptData);
    }
    if proposal.route != ProposalRoute::Poll {
        return if poll_ids.is_empty() {
            Ok(())
        } else {
            Err(PollRepoError::CorruptData)
        };
    }
    let current = proposal_poll_id(proposal).ok_or(PollRepoError::CorruptData)?;
    if !poll_ids.iter().any(|poll_id| poll_id == current) {
        return Err(PollRepoError::CorruptData);
    }
    for poll_id in poll_ids {
        let row = sqlx::query_as::<_, PollRow>(
            "SELECT trip_id AS poll_trip_id, id AS poll_id, \
                    created_by AS poll_created_by, kind AS poll_kind, \
                    replaces_poll_id AS poll_replaces_poll_id, title AS poll_title, \
                    description AS poll_description, created_at AS poll_created_at, \
                    opens_at AS poll_opens_at, closes_at AS poll_closes_at, \
                    decided_at AS poll_decided_at, quorum AS poll_quorum, \
                    allow_multi AS poll_allow_multi, status AS poll_status, \
                    resolution_note AS poll_resolution_note, revision AS poll_revision \
             FROM polls WHERE trip_id = ? AND id = ?",
        )
        .bind(trip_id)
        .bind(&poll_id)
        .fetch_optional(&mut **transaction)
        .await
        .map_err(unavailable)?
        .ok_or(PollRepoError::CorruptData)?;
        let poll = load_poll_parts(transaction, trip_id, row).await?;
        validate_plan_link(transaction, trip_id, &poll).await?;
    }
    Ok(())
}

pub(in crate::sqlite) async fn validate_current_plan_poll_provenance(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    plan: &itinera_core::domain::trip::Plan,
) -> Result<(), PollRepoError> {
    let Some(proposal_id) = plan.created_from_proposal_id.as_deref() else {
        return if plan.version == 1 {
            Ok(())
        } else {
            Err(PollRepoError::CorruptData)
        };
    };
    let proposal = load_proposal(transaction, trip_id, proposal_id)
        .await
        .map_err(map_proposal_error)?
        .ok_or(PollRepoError::CorruptData)?;
    if proposal.value.status != ProposalStatus::Applied
        || proposal.value.change_set.base_plan_version.checked_add(1) != Some(plan.version)
    {
        return Err(PollRepoError::CorruptData);
    }
    if proposal.value.route == ProposalRoute::LeaderApproval {
        let linked_poll_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT poll_id) FROM poll_options \
             WHERE trip_id = ? AND proposal_id = ?",
        )
        .bind(trip_id)
        .bind(proposal_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(unavailable)?;
        return if linked_poll_count == 0 {
            Ok(())
        } else {
            Err(PollRepoError::CorruptData)
        };
    }
    let poll_id = proposal_poll_id(&proposal.value).ok_or(PollRepoError::CorruptData)?;
    let row = sqlx::query_as::<_, PollRow>(
        "SELECT trip_id AS poll_trip_id, id AS poll_id, \
                created_by AS poll_created_by, kind AS poll_kind, \
                replaces_poll_id AS poll_replaces_poll_id, title AS poll_title, \
                description AS poll_description, created_at AS poll_created_at, \
                opens_at AS poll_opens_at, closes_at AS poll_closes_at, \
                decided_at AS poll_decided_at, quorum AS poll_quorum, \
                allow_multi AS poll_allow_multi, status AS poll_status, \
                resolution_note AS poll_resolution_note, revision AS poll_revision \
         FROM polls WHERE trip_id = ? AND id = ?",
    )
    .bind(trip_id)
    .bind(poll_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?
    .ok_or(PollRepoError::CorruptData)?;
    let poll = load_poll_parts(transaction, trip_id, row).await?;
    let winner = decisive_winner(&poll.poll).map_err(corrupt)?;
    if poll.poll.kind != PollKind::PlanChange
        || utc(&poll.created_at)? < utc(&proposal.value.created_at)?
        || !current_poll_link_is_valid(&poll.poll, &proposal.value, winner)
    {
        return Err(PollRepoError::CorruptData);
    }
    validate_terminal_plan_time(&proposal.value, &poll.poll, Some(plan))
}

async fn validate_one_proposal_plan_link(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    proposal: &LoadedProposal,
) -> Result<Option<itinera_core::domain::trip::Plan>, PollRepoError> {
    let versions = sqlx::query_as::<_, (i64, String)>(
        "SELECT version, id FROM plans WHERE trip_id = ? AND created_from_proposal_id = ?",
    )
    .bind(trip_id)
    .bind(&proposal.value.id)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    match (proposal.value.status, versions.as_slice()) {
        (ProposalStatus::Applied, [(version, plan_id)])
            if u32::try_from(*version).ok()
                == proposal.value.change_set.base_plan_version.checked_add(1) =>
        {
            crate::sqlite::trip_repo::plans::load_plan_detail(
                transaction,
                trip_id,
                plan_id,
                u32::try_from(*version).map_err(corrupt)?,
            )
            .await
            .map(|detail| Some(detail.plan))
            .map_err(map_trip_error)
        }
        (ProposalStatus::Applied, _) => Err(PollRepoError::CorruptData),
        (_, []) => Ok(None),
        _ => Err(PollRepoError::CorruptData),
    }
}

fn validate_terminal_plan_time(
    proposal: &Proposal,
    poll: &Poll,
    plan: Option<&itinera_core::domain::trip::Plan>,
) -> Result<(), PollRepoError> {
    match (proposal.status, poll.status, plan) {
        (ProposalStatus::Applied, PollStatus::Passed, Some(plan))
            if poll.decided_at.as_deref() == Some(plan.created_at.as_str()) =>
        {
            Ok(())
        }
        (ProposalStatus::Applied, _, _) => Err(PollRepoError::CorruptData),
        (_, _, None) => Ok(()),
        _ => Err(PollRepoError::CorruptData),
    }
}

fn current_poll_link_is_valid(
    poll: &Poll,
    proposal: &Proposal,
    winner: Option<&PollOption>,
) -> bool {
    let points_here = proposal_poll_id(proposal) == Some(poll.id.as_str());
    match poll.status {
        PollStatus::Draft | PollStatus::Scheduled | PollStatus::Open => {
            proposal.status == ProposalStatus::Pending && points_here
        }
        PollStatus::Passed => {
            winner.is_some_and(|option| option.proposal_id.is_some())
                && proposal.status == ProposalStatus::Applied
                && points_here
        }
        PollStatus::Failed => match winner {
            Some(option) if option.proposal_id.is_some() => {
                proposal.status == ProposalStatus::Stale && points_here
            }
            Some(_) => proposal.status == ProposalStatus::Rejected && points_here,
            None => proposal.status == ProposalStatus::Pending && points_here,
        },
        PollStatus::Expired => proposal.status == ProposalStatus::Pending && points_here,
    }
}

fn proposal_poll_id(proposal: &Proposal) -> Option<&str> {
    match proposal.decided_by.as_ref() {
        Some(ProposalDecision::Poll { poll_id }) => Some(poll_id),
        _ => None,
    }
}

async fn insert_poll(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    poll: &Poll,
    created_at: &str,
    replaces_poll_id: Option<&str>,
    revision: i64,
) -> Result<(), PollRepoError> {
    validate_stored_poll(trip_id, poll, created_at).map_err(corrupt)?;
    sqlx::query(
        "INSERT INTO polls ( \
             trip_id, id, created_by, kind, replaces_poll_id, title, description, created_at, opens_at, \
             closes_at, decided_at, quorum, allow_multi, status, resolution_note, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(trip_id)
    .bind(&poll.id)
    .bind(&poll.created_by)
    .bind(encode_kind(poll.kind))
    .bind(replaces_poll_id)
    .bind(&poll.title)
    .bind(&poll.description)
    .bind(created_at)
    .bind(&poll.opens_at)
    .bind(&poll.closes_at)
    .bind(&poll.decided_at)
    .bind(i64::from(poll.quorum))
    .bind(i64::from(poll.allow_multi))
    .bind(encode_status(poll.status))
    .bind(&poll.resolution_note)
    .bind(revision)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    for (position, option) in poll.options.iter().enumerate() {
        sqlx::query(
            "INSERT INTO poll_options (trip_id, poll_id, id, position, label, proposal_id) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(trip_id)
        .bind(&poll.id)
        .bind(&option.id)
        .bind(i64::try_from(position).map_err(corrupt)?)
        .bind(&option.label)
        .bind(&option.proposal_id)
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?;
    }
    Ok(())
}

async fn update_poll(
    transaction: &mut Transaction<'static, Sqlite>,
    poll: &LoadedPoll,
    previous_revision: i64,
) -> Result<(), PollRepoError> {
    validate_stored_poll(&poll.poll.trip_id, &poll.poll, &poll.created_at).map_err(corrupt)?;
    let next = next_revision(previous_revision).map_err(corrupt)?;
    let updated = sqlx::query(
        "UPDATE polls SET created_by = ?, kind = ?, title = ?, description = ?, \
             created_at = ?, opens_at = ?, closes_at = ?, decided_at = ?, quorum = ?, \
             allow_multi = ?, status = ?, resolution_note = ?, revision = ? \
         WHERE trip_id = ? AND id = ? AND revision = ?",
    )
    .bind(&poll.poll.created_by)
    .bind(encode_kind(poll.poll.kind))
    .bind(&poll.poll.title)
    .bind(&poll.poll.description)
    .bind(&poll.created_at)
    .bind(&poll.poll.opens_at)
    .bind(&poll.poll.closes_at)
    .bind(&poll.poll.decided_at)
    .bind(i64::from(poll.poll.quorum))
    .bind(i64::from(poll.poll.allow_multi))
    .bind(encode_status(poll.poll.status))
    .bind(&poll.poll.resolution_note)
    .bind(next)
    .bind(&poll.poll.trip_id)
    .bind(&poll.poll.id)
    .bind(previous_revision)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(PollRepoError::Conflict)
    }
}

async fn write_ballot(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    poll_id: &str,
    user_id: &str,
    option_ids: &[String],
    voted_at: &str,
    existing: Option<&LoadedBallot>,
) -> Result<(), PollRepoError> {
    sqlx::query(
        "DELETE FROM poll_ballot_options \
         WHERE trip_id = ? AND poll_id = ? AND user_id = ?",
    )
    .bind(trip_id)
    .bind(poll_id)
    .bind(user_id)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if option_ids.is_empty() {
        let Some(existing) = existing else {
            return Ok(());
        };
        let deleted = sqlx::query(
            "DELETE FROM poll_ballots \
             WHERE trip_id = ? AND poll_id = ? AND user_id = ? AND revision = ?",
        )
        .bind(trip_id)
        .bind(poll_id)
        .bind(user_id)
        .bind(existing.revision)
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?;
        if deleted.rows_affected() != 1 {
            return Err(PollRepoError::Conflict);
        }
        return Ok(());
    }
    match existing {
        Some(existing) => {
            let updated = sqlx::query(
                "UPDATE poll_ballots SET voted_at = ?, revision = ? \
                 WHERE trip_id = ? AND poll_id = ? AND user_id = ? AND revision = ?",
            )
            .bind(voted_at)
            .bind(next_revision(existing.revision).map_err(corrupt)?)
            .bind(trip_id)
            .bind(poll_id)
            .bind(user_id)
            .bind(existing.revision)
            .execute(&mut **transaction)
            .await
            .map_err(unavailable)?;
            if updated.rows_affected() != 1 {
                return Err(PollRepoError::Conflict);
            }
        }
        None => {
            sqlx::query(
                "INSERT INTO poll_ballots (trip_id, poll_id, user_id, voted_at, revision) \
                 VALUES (?, ?, ?, ?, 1)",
            )
            .bind(trip_id)
            .bind(poll_id)
            .bind(user_id)
            .bind(voted_at)
            .execute(&mut **transaction)
            .await
            .map_err(unavailable)?;
        }
    }
    for option_id in option_ids {
        sqlx::query(
            "INSERT INTO poll_ballot_options (trip_id, poll_id, user_id, option_id) \
             VALUES (?, ?, ?, ?)",
        )
        .bind(trip_id)
        .bind(poll_id)
        .bind(user_id)
        .bind(option_id)
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?;
    }
    Ok(())
}

async fn load_actor_ballot(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    poll: &LoadedPoll,
    user_id: &str,
) -> Result<Option<LoadedBallot>, PollRepoError> {
    let mut votes = poll
        .poll
        .votes
        .iter()
        .filter(|vote| vote.user_id == user_id)
        .collect::<Vec<_>>();
    if votes.is_empty() {
        let orphan: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM poll_ballots \
             WHERE trip_id = ? AND poll_id = ? AND user_id = ?",
        )
        .bind(trip_id)
        .bind(&poll.poll.id)
        .bind(user_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(unavailable)?;
        return if orphan == 0 {
            Ok(None)
        } else {
            Err(PollRepoError::CorruptData)
        };
    }
    votes.sort_by(|left, right| left.option_id.cmp(&right.option_id));
    let at = votes[0].at.clone();
    if votes.iter().any(|vote| vote.at != at) {
        return Err(PollRepoError::CorruptData);
    }
    let (stored_at, revision): (String, i64) = sqlx::query_as(
        "SELECT voted_at, revision FROM poll_ballots \
         WHERE trip_id = ? AND poll_id = ? AND user_id = ?",
    )
    .bind(trip_id)
    .bind(&poll.poll.id)
    .bind(user_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    checked_revision(revision).map_err(corrupt)?;
    if stored_at != at {
        return Err(PollRepoError::CorruptData);
    }
    Ok(Some(LoadedBallot {
        user_id: user_id.to_string(),
        option_ids: votes
            .into_iter()
            .map(|vote| vote.option_id.clone())
            .collect(),
        voted_at: at,
        revision,
    }))
}

async fn eligible_voter_count(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<u32, PollRepoError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM trip_memberships \
         WHERE trip_id = ? AND role IN ('leader', 'member')",
    )
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    u32::try_from(count)
        .ok()
        .filter(|count| (1..=1_000).contains(count))
        .ok_or(PollRepoError::CorruptData)
}

fn plan_change_poll(
    trip_id: &str,
    actor: &itinera_core::domain::user::UserId,
    proposal: &Proposal,
    new: &NewPlanChangePoll,
    quorum: u32,
) -> Poll {
    Poll {
        id: new.poll_id.clone(),
        trip_id: trip_id.to_string(),
        created_by: actor.0.clone(),
        kind: PollKind::PlanChange,
        title: proposal.title.clone(),
        description: proposal.rationale.clone(),
        options: vec![
            PollOption {
                id: new.adopt_option_id.clone(),
                label: ADOPT_LABEL.to_string(),
                proposal_id: Some(proposal.id.clone()),
            },
            PollOption {
                id: new.keep_option_id.clone(),
                label: KEEP_LABEL.to_string(),
                proposal_id: None,
            },
        ],
        opens_at: None,
        closes_at: new.closes_at.clone(),
        decided_at: None,
        quorum,
        allow_multi: false,
        status: PollStatus::Open,
        votes: Vec::new(),
        resolution_note: None,
    }
}

fn close_poll_state(poll: &mut Poll, decided_at: &str, status: PollStatus, note: Option<&str>) {
    poll.status = status;
    poll.decided_at = Some(decided_at.to_string());
    poll.resolution_note = note.map(str::to_string);
}

fn no_decision_note(poll: &Poll) -> Result<&'static str, PollRepoError> {
    let voters = poll
        .votes
        .iter()
        .map(|vote| vote.user_id.as_str())
        .collect::<HashSet<_>>()
        .len();
    let mut counts = poll
        .options
        .iter()
        .map(|option| (option.id.as_str(), 0_usize))
        .collect::<HashMap<_, _>>();
    for vote in &poll.votes {
        *counts
            .get_mut(vote.option_id.as_str())
            .ok_or(PollRepoError::CorruptData)? += 1;
    }
    let top = counts.values().copied().max().unwrap_or(0);
    let winners = counts.values().filter(|count| **count == top).count();
    Ok(if top == 0 || winners != 1 {
        TIE_NOTE
    } else if top.saturating_mul(2) <= voters {
        NO_MAJORITY_NOTE
    } else {
        return Err(PollRepoError::CorruptData);
    })
}

fn valid_ballot(poll: &Poll, option_ids: &[String]) -> bool {
    if option_ids.len() > poll.options.len()
        || (!poll.allow_multi && option_ids.len() != 1)
        || poll.allow_multi && option_ids.len() > poll.options.len()
    {
        return false;
    }
    let available = poll
        .options
        .iter()
        .map(|option| option.id.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    option_ids
        .iter()
        .all(|id| available.contains(id.as_str()) && seen.insert(id))
}

fn ensure_poll_projection(
    current: &[LoadedPoll],
    replaced_id: Option<&str>,
    next: &Poll,
) -> Result<(), PollRepoError> {
    let mut projected = current
        .iter()
        .filter(|poll| Some(poll.poll.id.as_str()) != replaced_id)
        .map(|poll| poll.poll.clone())
        .collect::<Vec<_>>();
    projected.push(next.clone());
    if projected.len() > MAX_POLLS
        || serde_json::to_vec(&projected).map_err(corrupt)?.len() > MAX_POLL_RESPONSE_BYTES
    {
        return Err(PollRepoError::SafetyLimitExceeded);
    }
    Ok(())
}

fn ensure_loaded_poll_budget(polls: &[LoadedPoll]) -> Result<(), PollRepoError> {
    let values = polls
        .iter()
        .map(|poll| poll.poll.clone())
        .collect::<Vec<_>>();
    if values.len() > MAX_POLLS
        || serde_json::to_vec(&values).map_err(corrupt)?.len() > MAX_POLL_RESPONSE_BYTES
    {
        Err(PollRepoError::SafetyLimitExceeded)
    } else {
        Ok(())
    }
}

fn sort_polls(polls: &mut [LoadedPoll]) -> Result<(), PollRepoError> {
    let mut timestamps = HashMap::new();
    for poll in polls.iter() {
        timestamps.insert(poll.poll.id.clone(), utc(&poll.created_at)?);
    }
    polls.sort_by(|left, right| {
        timestamps[&right.poll.id]
            .cmp(&timestamps[&left.poll.id])
            .then_with(|| right.poll.id.cmp(&left.poll.id))
    });
    Ok(())
}

fn utc(value: &str) -> Result<DateTime<chrono::FixedOffset>, PollRepoError> {
    if value.len() > 64 || !value.ends_with('Z') {
        return Err(PollRepoError::CorruptData);
    }
    let timestamp = DateTime::parse_from_rfc3339(value).map_err(corrupt)?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err(PollRepoError::CorruptData);
    }
    Ok(timestamp)
}

fn canonical_transaction_time(
    value: &str,
) -> Result<(DateTime<chrono::FixedOffset>, String), PollRepoError> {
    if value.len() > 64 {
        return Err(PollRepoError::CorruptData);
    }
    let timestamp = DateTime::parse_from_rfc3339(value).map_err(corrupt)?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err(PollRepoError::CorruptData);
    }
    let canonical = value.strip_suffix("+00:00").map_or_else(
        || {
            if value.ends_with('Z') {
                value.to_string()
            } else {
                timestamp.to_rfc3339_opts(SecondsFormat::AutoSi, true)
            }
        },
        |prefix| format!("{prefix}Z"),
    );
    Ok((timestamp, canonical))
}

fn validate_requested_id(value: &str) -> Result<(), PollRepoError> {
    validate_id(value).map_err(|_| PollRepoError::NotFound)
}

fn map_trip_error(error: TripRepoError) -> PollRepoError {
    match error {
        TripRepoError::Unavailable => PollRepoError::Unavailable,
        TripRepoError::CorruptData => PollRepoError::CorruptData,
        TripRepoError::NotFound => PollRepoError::NotFound,
        TripRepoError::Forbidden => PollRepoError::Forbidden,
        TripRepoError::Conflict | TripRepoError::DuplicateInvite => PollRepoError::Conflict,
    }
}

fn map_proposal_error(error: ProposalRepoError) -> PollRepoError {
    match error {
        ProposalRepoError::Unavailable => PollRepoError::Unavailable,
        ProposalRepoError::CorruptData => PollRepoError::CorruptData,
        ProposalRepoError::NotFound => PollRepoError::NotFound,
        ProposalRepoError::Forbidden => PollRepoError::Forbidden,
        ProposalRepoError::Conflict => PollRepoError::Conflict,
        ProposalRepoError::InvalidChange => PollRepoError::Conflict,
        ProposalRepoError::SafetyLimitExceeded => PollRepoError::SafetyLimitExceeded,
    }
}

fn unavailable<T>(_error: T) -> PollRepoError {
    PollRepoError::Unavailable
}

fn corrupt<T>(_error: T) -> PollRepoError {
    PollRepoError::CorruptData
}
