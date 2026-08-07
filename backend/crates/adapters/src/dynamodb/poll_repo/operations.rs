use std::collections::HashMap;

use chrono::DateTime;
use itinera_core::{
    domain::{
        poll::{Poll, PollKind, PollStatus},
        proposal::{Proposal, ProposalDecision, ProposalRoute, ProposalStatus},
        trip::TripRole,
        user::UserId,
    },
    ports::poll::{NewDecisionPoll, PollRepoError},
};

use crate::dynamodb::{
    DynamoUserRepo, ENTITY_TYPE,
    primitives::{condition_action, delete_action, put_action, transaction_condition_failed},
    proposal_repo::operations::get_proposal,
    trip_repo::records::{string, trip_pk},
};

use super::{
    access::{MAX_POLL_BYTES, RequiredPollRole},
    proposal_error, record_error,
    records::{
        BALLOT_ENTITY, BallotRecord, LoadedBallot, LoadedPoll, POLL_ENTITY, POLL_PREFIX,
        attach_ballots, decisive_winner, decode_ballot, decode_poll, encode_ballot, encode_poll,
        poll_sk, validate_ballot_for_poll,
    },
};

pub(super) struct LoadedPollAggregate {
    pub(super) loaded: LoadedPoll,
    pub(super) ballots: HashMap<String, LoadedBallot>,
}

impl LoadedPollAggregate {
    pub(super) fn poll(&self) -> Result<Poll, PollRepoError> {
        let mut loaded = self.loaded.clone();
        attach_ballots(&mut loaded, self.ballots.values().cloned())?;
        Ok(loaded.poll)
    }
}

pub(super) async fn list_polls(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Vec<Poll>, PollRepoError> {
    repo.poll_authorize(trip_id, actor, RequiredPollRole::Any)
        .await?;
    repo.poll_trip_meta(trip_id).await?;
    let pk = trip_pk(trip_id);
    let items = repo.poll_query(&pk, POLL_PREFIX).await?;
    let mut polls = HashMap::<String, LoadedPoll>::new();
    let mut ballots = Vec::new();
    for item in items {
        match string(&item, ENTITY_TYPE).map_err(record_error)?.as_str() {
            POLL_ENTITY => {
                let loaded = decode_poll(&item, trip_id)?;
                if polls.insert(loaded.poll.id.clone(), loaded).is_some() {
                    return Err(PollRepoError::CorruptData);
                }
            }
            BALLOT_ENTITY => ballots.push(decode_ballot(&item, trip_id)?),
            _ => return Err(PollRepoError::CorruptData),
        }
    }
    let mut ballots_by_poll = HashMap::<String, Vec<LoadedBallot>>::new();
    for ballot in ballots {
        if !polls.contains_key(&ballot.ballot.poll_id) {
            return Err(PollRepoError::CorruptData);
        }
        ballots_by_poll
            .entry(ballot.ballot.poll_id.clone())
            .or_default()
            .push(ballot);
    }
    let mut loaded = polls.into_values().collect::<Vec<_>>();
    loaded.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.poll.id.cmp(&left.poll.id))
    });
    let mut result = Vec::with_capacity(loaded.len());
    for mut poll in loaded {
        let poll_id = poll.poll.id.clone();
        attach_ballots(
            &mut poll,
            ballots_by_poll.remove(&poll_id).unwrap_or_default(),
        )?;
        validate_plan_link(repo, trip_id, &poll.poll).await?;
        result.push(poll.poll);
    }
    let response_bytes = serde_json::to_vec(&result)
        .map_err(|_| PollRepoError::CorruptData)?
        .len();
    if response_bytes > MAX_POLL_BYTES {
        return Err(PollRepoError::SafetyLimitExceeded);
    }
    Ok(result)
}

pub(super) async fn create_decision_poll(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    new: NewDecisionPoll,
) -> Result<Poll, PollRepoError> {
    repo.poll_authorize(trip_id, actor, RequiredPollRole::Editor)
        .await?;
    let meta = repo.poll_trip_meta(trip_id).await?;
    let eligible = repo
        .eligible_voter_count(trip_id, meta.revision, meta.value.member_count)
        .await?;
    let quorum = eligible.div_ceil(2);
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
        votes: vec![],
        resolution_note: None,
    };
    let item = encode_poll(&poll, &new.created_at, 1)?;
    let result = repo
        .transaction()
        .transact_items(condition_action(repo.poll_membership_condition(
            trip_id,
            actor,
            RequiredPollRole::Editor,
        )))
        .transact_items(condition_action(repo.trip_meta_revision_condition(
            trip_id,
            meta.revision,
            meta.value.member_count,
        )))
        .transact_items(put_action(repo.create_only_put(item)))
        .send()
        .await;
    match result {
        Ok(_) => Ok(poll),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            repo.poll_authorize(trip_id, actor, RequiredPollRole::Editor)
                .await?;
            Err(PollRepoError::Conflict)
        }
        Err(_) => Err(PollRepoError::Unavailable),
    }
}

pub(super) async fn open_poll(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    poll_id: &str,
    opened_at: &str,
) -> Result<Poll, PollRepoError> {
    let role = repo
        .poll_authorize(trip_id, actor, RequiredPollRole::Editor)
        .await?;
    let mut aggregate = load_poll(repo, trip_id, poll_id).await?;
    if role != TripRole::Leader && aggregate.loaded.poll.created_by != actor.0 {
        return Err(PollRepoError::Forbidden);
    }
    match aggregate.loaded.poll.status {
        PollStatus::Open => return aggregate.poll(),
        PollStatus::Draft | PollStatus::Scheduled => {}
        PollStatus::Passed | PollStatus::Failed | PollStatus::Expired => {
            return Err(PollRepoError::Conflict);
        }
    }
    if utc(opened_at)? >= utc(&aggregate.loaded.poll.closes_at)? {
        return Err(PollRepoError::Conflict);
    }
    let next_revision = aggregate
        .loaded
        .revision
        .checked_add(1)
        .ok_or(PollRepoError::CorruptData)?;
    aggregate.loaded.poll.status = PollStatus::Open;
    aggregate.loaded.poll.opens_at = None;
    let item = encode_poll(
        &aggregate.loaded.poll,
        &aggregate.loaded.created_at,
        next_revision,
    )?;
    let result = repo
        .transaction()
        .transact_items(condition_action(repo.poll_membership_condition(
            trip_id,
            actor,
            RequiredPollRole::Editor,
        )))
        .transact_items(put_action(
            repo.revision_put(item, aggregate.loaded.revision),
        ))
        .send()
        .await;
    match result {
        Ok(_) => aggregate.poll(),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            let role = repo
                .poll_authorize(trip_id, actor, RequiredPollRole::Editor)
                .await?;
            let latest = load_poll(repo, trip_id, poll_id).await?;
            if role != TripRole::Leader && latest.loaded.poll.created_by != actor.0 {
                return Err(PollRepoError::Forbidden);
            }
            if latest.loaded.poll.status == PollStatus::Open {
                latest.poll()
            } else {
                Err(PollRepoError::Conflict)
            }
        }
        Err(_) => Err(PollRepoError::Unavailable),
    }
}

pub(super) async fn cast_vote(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    poll_id: &str,
    option_ids: &[String],
    voted_at: &str,
) -> Result<Poll, PollRepoError> {
    repo.poll_authorize(trip_id, actor, RequiredPollRole::Editor)
        .await?;
    let mut desired = option_ids.to_vec();
    desired.sort();
    let mut aggregate = load_poll(repo, trip_id, poll_id).await?;
    validate_ballot_for_poll(&aggregate.loaded.poll, &desired)?;
    if ballot_matches(aggregate.ballots.get(&actor.0), &desired) {
        return aggregate.poll();
    }
    let voted_at_timestamp = utc(voted_at)?;
    if aggregate.loaded.poll.status != PollStatus::Open
        || voted_at_timestamp < utc(&aggregate.loaded.created_at)?
        || voted_at_timestamp >= utc(&aggregate.loaded.poll.closes_at)?
    {
        return Err(PollRepoError::Conflict);
    }
    let poll_revision = aggregate
        .loaded
        .revision
        .checked_add(1)
        .ok_or(PollRepoError::CorruptData)?;
    let poll_item = encode_poll(
        &aggregate.loaded.poll,
        &aggregate.loaded.created_at,
        poll_revision,
    )?;
    let existing = aggregate.ballots.get(&actor.0);
    if desired.is_empty() && existing.is_none() {
        return aggregate.poll();
    }
    let ballot_action = if desired.is_empty() {
        delete_action(
            repo.ballot_delete(
                trip_id,
                poll_id,
                &actor.0,
                existing
                    .expect("empty withdrawal has an existing ballot")
                    .revision,
            ),
        )
    } else {
        let ballot = BallotRecord {
            poll_id: poll_id.to_string(),
            user_id: actor.0.clone(),
            option_ids: desired.clone(),
            at: voted_at.to_string(),
        };
        let revision = match existing {
            Some(loaded) => loaded
                .revision
                .checked_add(1)
                .ok_or(PollRepoError::CorruptData)?,
            None => 1,
        };
        let item = encode_ballot(trip_id, &ballot, revision)?;
        put_action(match existing {
            Some(loaded) => repo.revision_put(item, loaded.revision),
            None => repo.create_only_put(item),
        })
    };
    let result = repo
        .transaction()
        .transact_items(condition_action(repo.poll_membership_condition(
            trip_id,
            actor,
            RequiredPollRole::Editor,
        )))
        // Every ballot mutation advances the poll revision. A close therefore
        // cannot decide from a vote snapshot while another ballot commits.
        .transact_items(put_action(
            repo.revision_put(poll_item, aggregate.loaded.revision),
        ))
        .transact_items(ballot_action)
        .send()
        .await;
    match result {
        Ok(_) => {
            aggregate.loaded.revision = poll_revision;
            if desired.is_empty() {
                aggregate.ballots.remove(&actor.0);
            } else {
                let next_revision = existing.map_or(1, |loaded| loaded.revision + 1);
                aggregate.ballots.insert(
                    actor.0.clone(),
                    LoadedBallot {
                        ballot: BallotRecord {
                            poll_id: poll_id.to_string(),
                            user_id: actor.0.clone(),
                            option_ids: desired,
                            at: voted_at.to_string(),
                        },
                        revision: next_revision,
                    },
                );
            }
            aggregate.poll()
        }
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            repo.poll_authorize(trip_id, actor, RequiredPollRole::Editor)
                .await?;
            let latest = load_poll(repo, trip_id, poll_id).await?;
            if ballot_matches(latest.ballots.get(&actor.0), &desired) {
                latest.poll()
            } else {
                Err(PollRepoError::Conflict)
            }
        }
        Err(_) => Err(PollRepoError::Unavailable),
    }
}

pub(super) async fn load_poll(
    repo: &DynamoUserRepo,
    trip_id: &str,
    poll_id: &str,
) -> Result<LoadedPollAggregate, PollRepoError> {
    let aggregate = load_poll_records(repo, trip_id, poll_id).await?;
    validate_plan_link(repo, trip_id, &aggregate.poll()?).await?;
    Ok(aggregate)
}

async fn load_poll_records(
    repo: &DynamoUserRepo,
    trip_id: &str,
    poll_id: &str,
) -> Result<LoadedPollAggregate, PollRepoError> {
    let pk = trip_pk(trip_id);
    let sk = poll_sk(poll_id);
    let item = repo
        .poll_get(&pk, &sk)
        .await?
        .ok_or(PollRepoError::NotFound)?;
    let loaded = decode_poll(&item, trip_id)?;
    let items = repo
        .poll_query(&pk, &format!("{}#VOTE#", poll_sk(poll_id)))
        .await?;
    let mut ballots = HashMap::new();
    for item in items {
        let ballot = decode_ballot(&item, trip_id)?;
        if ballot.ballot.poll_id != poll_id
            || ballots
                .insert(ballot.ballot.user_id.clone(), ballot)
                .is_some()
        {
            return Err(PollRepoError::CorruptData);
        }
    }
    let aggregate = LoadedPollAggregate { loaded, ballots };
    aggregate.poll()?;
    Ok(aggregate)
}

pub(super) async fn validate_plan_link(
    repo: &DynamoUserRepo,
    trip_id: &str,
    poll: &Poll,
) -> Result<(), PollRepoError> {
    if poll.kind == PollKind::Decision {
        return Ok(());
    }
    let proposal_id = poll
        .options
        .iter()
        .find_map(|option| option.proposal_id.as_deref())
        .ok_or(PollRepoError::CorruptData)?;
    let proposal = get_proposal(repo, trip_id, proposal_id)
        .await
        .map_err(proposal_error)?
        .ok_or(PollRepoError::CorruptData)?
        .value;
    if proposal.route != ProposalRoute::Poll {
        return Err(PollRepoError::CorruptData);
    }
    let winner = decisive_winner(poll)?;
    if current_plan_link_is_valid(poll, &proposal, winner) {
        return Ok(());
    }

    let historical_no_decision = poll.status == PollStatus::Expired
        || (poll.status == PollStatus::Failed && winner.is_none());
    let Some(replacement_poll_id) = proposal_poll_id(&proposal) else {
        return Err(PollRepoError::CorruptData);
    };
    if !historical_no_decision || replacement_poll_id == poll.id {
        return Err(PollRepoError::CorruptData);
    }

    let replacement = load_poll_records(repo, trip_id, replacement_poll_id)
        .await
        .map_err(|error| match error {
            PollRepoError::NotFound => PollRepoError::CorruptData,
            other => other,
        })?
        .poll()?;
    let replacement_proposal_id = replacement
        .options
        .iter()
        .find_map(|option| option.proposal_id.as_deref());
    if replacement.kind != PollKind::PlanChange
        || replacement_proposal_id != Some(proposal_id)
        || !current_plan_link_is_valid(&replacement, &proposal, decisive_winner(&replacement)?)
    {
        return Err(PollRepoError::CorruptData);
    }
    Ok(())
}

fn current_plan_link_is_valid(
    poll: &Poll,
    proposal: &Proposal,
    winner: Option<&itinera_core::domain::poll::PollOption>,
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

fn ballot_matches(existing: Option<&LoadedBallot>, desired: &[String]) -> bool {
    match existing {
        Some(existing) => existing.ballot.option_ids == desired,
        None => desired.is_empty(),
    }
}

fn utc(value: &str) -> Result<DateTime<chrono::FixedOffset>, PollRepoError> {
    let timestamp = DateTime::parse_from_rfc3339(value).map_err(|_| PollRepoError::CorruptData)?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err(PollRepoError::CorruptData);
    }
    Ok(timestamp)
}
