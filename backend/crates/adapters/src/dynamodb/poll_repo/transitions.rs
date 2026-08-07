use std::collections::HashMap;

use itinera_core::{
    domain::{
        poll::{Poll, PollKind, PollOption, PollStatus},
        proposal::{Proposal, ProposalDecision, ProposalRoute, ProposalStatus},
        user::UserId,
    },
    ports::{
        poll::{NewPlanChangePoll, PollRepoError},
        proposal::ProposalApplicationIds,
    },
    services::proposals::validate_stored_proposal,
};

use crate::dynamodb::{
    DynamoUserRepo,
    primitives::{condition_action, put_action, transaction_condition_failed},
    proposal_repo::{
        access::enforce_transaction_data_limit,
        application::{
            ApplicationCommand, ProposalWrite, prepare_application, publish_application,
        },
        operations::get_proposal,
        records::encode_proposal,
    },
};

use super::{
    access::RequiredPollRole,
    operations::{LoadedPollAggregate, load_poll},
    proposal_error,
    records::{ADOPT_LABEL, KEEP_LABEL, encode_poll},
};

const BELOW_QUORUM_NOTE: &str = "Closed below quorum - no decision recorded.";
const TIE_NOTE: &str = "Closed with a tied result - no decision recorded.";
const NO_MAJORITY_NOTE: &str = "No option reached a majority - no decision recorded.";
const KEEP_NOTE: &str = "The group chose to keep the current plan.";
const STALE_NOTE: &str =
    "The winning proposal was based on an outdated plan - no plan change was applied.";

pub(super) async fn create_proposal_poll(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    mut proposal: Proposal,
    new: NewPlanChangePoll,
    application_ids: ProposalApplicationIds,
) -> Result<Proposal, PollRepoError> {
    repo.poll_authorize(trip_id, actor, RequiredPollRole::Editor)
        .await?;
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
    validate_stored_proposal(trip_id, &proposal).map_err(|_| PollRepoError::CorruptData)?;
    let meta = repo
        .proposal_trip_meta(trip_id)
        .await
        .map_err(proposal_error)?;
    let meta_revision = meta.revision;
    let member_count = meta.value.member_count;
    let eligible = repo
        .eligible_voter_count(trip_id, meta_revision, member_count)
        .await?;
    let plan_id = meta
        .value
        .current_plan_id
        .clone()
        .ok_or(PollRepoError::Conflict)?;
    let version = meta
        .value
        .current_plan_version
        .ok_or(PollRepoError::Conflict)?;
    if version != proposal.change_set.base_plan_version {
        return Err(PollRepoError::Conflict);
    }
    prepare_application(
        repo,
        trip_id,
        actor,
        proposal.clone(),
        meta,
        ApplicationCommand {
            decision: ProposalDecision::Poll {
                poll_id: new.poll_id.clone(),
            },
            applied_at: &new.created_at,
            ids: application_ids,
        },
    )
    .await
    .map_err(proposal_error)?;
    let poll = plan_change_poll(trip_id, actor, &proposal, new, eligible.div_ceil(2));
    let proposal_item = encode_proposal(&proposal, 1).map_err(proposal_error)?;
    let poll_item = encode_poll(&poll, &proposal.created_at, 1)?;
    enforce_transaction_data_limit(&[proposal_item.clone(), poll_item.clone()])
        .map_err(proposal_error)?;
    let result = repo
        .transaction()
        .transact_items(condition_action(repo.poll_membership_condition(
            trip_id,
            actor,
            RequiredPollRole::Editor,
        )))
        .transact_items(condition_action(repo.current_plan_condition(
            trip_id,
            meta_revision,
            &plan_id,
            version,
        )))
        .transact_items(put_action(repo.create_only_put(proposal_item)))
        .transact_items(put_action(repo.create_only_put(poll_item)))
        .send()
        .await;
    match result {
        Ok(_) => Ok(proposal),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            repo.poll_authorize(trip_id, actor, RequiredPollRole::Editor)
                .await?;
            Err(PollRepoError::Conflict)
        }
        Err(_) => Err(PollRepoError::Unavailable),
    }
}

pub(super) async fn route_proposal_to_poll(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    proposal_id: &str,
    new: NewPlanChangePoll,
    application_ids: ProposalApplicationIds,
) -> Result<Poll, PollRepoError> {
    repo.poll_authorize(trip_id, actor, RequiredPollRole::Leader)
        .await?;
    let stored = get_proposal(repo, trip_id, proposal_id)
        .await
        .map_err(proposal_error)?
        .ok_or(PollRepoError::NotFound)?;
    let prior_poll_id = match &stored.value.decided_by {
        Some(ProposalDecision::Poll { poll_id }) => Some(poll_id.clone()),
        _ => None,
    };
    if let Some(ProposalDecision::Poll { poll_id }) = &stored.value.decided_by {
        let prior = load_poll(repo, trip_id, poll_id).await?;
        if stored.value.status != ProposalStatus::Pending
            || matches!(
                prior.loaded.poll.status,
                PollStatus::Draft | PollStatus::Scheduled | PollStatus::Open
            )
        {
            return prior.poll();
        }
    } else if stored.value.decided_by.is_some() {
        return Err(PollRepoError::Conflict);
    }
    if stored.value.status != ProposalStatus::Pending {
        return Err(PollRepoError::Conflict);
    }
    let meta = repo
        .proposal_trip_meta(trip_id)
        .await
        .map_err(proposal_error)?;
    let meta_revision = meta.revision;
    let member_count = meta.value.member_count;
    let eligible = repo
        .eligible_voter_count(trip_id, meta_revision, member_count)
        .await?;
    let plan_id = meta
        .value
        .current_plan_id
        .clone()
        .ok_or(PollRepoError::Conflict)?;
    let version = meta
        .value
        .current_plan_version
        .ok_or(PollRepoError::CorruptData)?;
    if version < stored.value.change_set.base_plan_version {
        return Err(PollRepoError::CorruptData);
    }
    if version > stored.value.change_set.base_plan_version {
        return Err(PollRepoError::Conflict);
    }
    let mut proposal = stored.value;
    proposal.route = ProposalRoute::Poll;
    proposal.decided_by = Some(ProposalDecision::Poll {
        poll_id: new.poll_id.clone(),
    });
    proposal.rejection_reason = None;
    validate_stored_proposal(trip_id, &proposal).map_err(|_| PollRepoError::CorruptData)?;
    prepare_application(
        repo,
        trip_id,
        actor,
        proposal.clone(),
        meta,
        ApplicationCommand {
            decision: ProposalDecision::Poll {
                poll_id: new.poll_id.clone(),
            },
            applied_at: &new.created_at,
            ids: application_ids,
        },
    )
    .await
    .map_err(proposal_error)?;
    let created_at = new.created_at.clone();
    let poll = plan_change_poll(trip_id, actor, &proposal, new, eligible.div_ceil(2));
    let proposal_revision = stored
        .revision
        .checked_add(1)
        .ok_or(PollRepoError::CorruptData)?;
    let proposal_item = encode_proposal(&proposal, proposal_revision).map_err(proposal_error)?;
    let poll_item = encode_poll(&poll, &created_at, 1)?;
    enforce_transaction_data_limit(&[proposal_item.clone(), poll_item.clone()])
        .map_err(proposal_error)?;
    let result = repo
        .transaction()
        .transact_items(condition_action(repo.poll_membership_condition(
            trip_id,
            actor,
            RequiredPollRole::Leader,
        )))
        .transact_items(condition_action(repo.current_plan_condition(
            trip_id,
            meta_revision,
            &plan_id,
            version,
        )))
        .transact_items(put_action(
            repo.revision_put(proposal_item, stored.revision),
        ))
        .transact_items(put_action(repo.create_only_put(poll_item)))
        .send()
        .await;
    match result {
        Ok(_) => Ok(poll),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            repo.poll_authorize(trip_id, actor, RequiredPollRole::Leader)
                .await?;
            let latest = get_proposal(repo, trip_id, proposal_id)
                .await
                .map_err(proposal_error)?
                .ok_or(PollRepoError::NotFound)?;
            if let Some(ProposalDecision::Poll { poll_id }) = latest.value.decided_by
                && prior_poll_id.as_deref() != Some(poll_id.as_str())
            {
                load_poll(repo, trip_id, &poll_id).await?.poll()
            } else {
                Err(PollRepoError::Conflict)
            }
        }
        Err(_) => Err(PollRepoError::Unavailable),
    }
}

pub(super) async fn close_poll(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    poll_id: &str,
    decided_at: &str,
    application_ids: ProposalApplicationIds,
) -> Result<Poll, PollRepoError> {
    // One bounded retry re-evaluates a concurrent ballot or plan publication.
    // Every ballot advances the poll revision, so no close can use a torn vote
    // snapshot; a second collision is reported rather than spun indefinitely.
    for _ in 0..2 {
        repo.poll_authorize(trip_id, actor, RequiredPollRole::Leader)
            .await?;
        let aggregate = load_poll(repo, trip_id, poll_id).await?;
        if aggregate.loaded.poll.status.is_terminal() {
            return aggregate.poll();
        }
        if aggregate.loaded.poll.status != PollStatus::Open {
            return Err(PollRepoError::Conflict);
        }
        let result = close_loaded(
            repo,
            trip_id,
            actor,
            aggregate,
            decided_at,
            application_ids.clone(),
        )
        .await;
        if !matches!(result, Err(PollRepoError::Conflict)) {
            return result;
        }
    }
    Err(PollRepoError::Conflict)
}

async fn close_loaded(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    aggregate: LoadedPollAggregate,
    decided_at: &str,
    application_ids: ProposalApplicationIds,
) -> Result<Poll, PollRepoError> {
    let voters = u32::try_from(aggregate.ballots.len()).map_err(|_| PollRepoError::CorruptData)?;
    if voters < aggregate.loaded.poll.quorum {
        return close_poll_only(
            repo,
            trip_id,
            actor,
            aggregate,
            decided_at,
            PollStatus::Expired,
            Some(BELOW_QUORUM_NOTE),
        )
        .await;
    }
    let mut counts = aggregate
        .loaded
        .poll
        .options
        .iter()
        .map(|option| (option.id.as_str(), 0_u32))
        .collect::<HashMap<_, _>>();
    for ballot in aggregate.ballots.values() {
        for option_id in &ballot.ballot.option_ids {
            let count = counts
                .get_mut(option_id.as_str())
                .ok_or(PollRepoError::CorruptData)?;
            *count = count.checked_add(1).ok_or(PollRepoError::CorruptData)?;
        }
    }
    let top = counts.values().copied().max().unwrap_or(0);
    let winners = aggregate
        .loaded
        .poll
        .options
        .iter()
        .filter(|option| counts.get(option.id.as_str()) == Some(&top))
        .cloned()
        .collect::<Vec<_>>();
    if top == 0 || winners.len() != 1 {
        return close_poll_only(
            repo,
            trip_id,
            actor,
            aggregate,
            decided_at,
            PollStatus::Failed,
            Some(TIE_NOTE),
        )
        .await;
    }
    if top.saturating_mul(2) <= voters {
        return close_poll_only(
            repo,
            trip_id,
            actor,
            aggregate,
            decided_at,
            PollStatus::Failed,
            Some(NO_MAJORITY_NOTE),
        )
        .await;
    }
    if aggregate.loaded.poll.kind == PollKind::Decision {
        return close_poll_only(
            repo,
            trip_id,
            actor,
            aggregate,
            decided_at,
            PollStatus::Passed,
            None,
        )
        .await;
    }
    let winner = winners.into_iter().next().expect("one winner was checked");
    if winner.proposal_id.is_some() {
        close_adopt(repo, trip_id, actor, aggregate, decided_at, application_ids).await
    } else {
        close_keep(repo, trip_id, actor, aggregate, decided_at).await
    }
}

async fn close_poll_only(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    mut aggregate: LoadedPollAggregate,
    decided_at: &str,
    status: PollStatus,
    note: Option<&str>,
) -> Result<Poll, PollRepoError> {
    aggregate.loaded.poll.status = status;
    aggregate.loaded.poll.decided_at = Some(decided_at.to_string());
    aggregate.loaded.poll.resolution_note = note.map(str::to_string);
    let next = aggregate
        .loaded
        .revision
        .checked_add(1)
        .ok_or(PollRepoError::CorruptData)?;
    let item = encode_poll(&aggregate.loaded.poll, &aggregate.loaded.created_at, next)?;
    let result = repo
        .transaction()
        .transact_items(condition_action(repo.poll_membership_condition(
            trip_id,
            actor,
            RequiredPollRole::Leader,
        )))
        .transact_items(put_action(
            repo.revision_put(item, aggregate.loaded.revision),
        ))
        .send()
        .await;
    match result {
        Ok(_) => aggregate.poll(),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            Err(PollRepoError::Conflict)
        }
        Err(_) => Err(PollRepoError::Unavailable),
    }
}

async fn close_keep(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    mut aggregate: LoadedPollAggregate,
    decided_at: &str,
) -> Result<Poll, PollRepoError> {
    let mut stored = linked_pending_proposal(repo, trip_id, &aggregate.loaded.poll).await?;
    stored.value.status = ProposalStatus::Rejected;
    stored.value.rejection_reason = Some(KEEP_NOTE.to_string());
    validate_stored_proposal(trip_id, &stored.value).map_err(|_| PollRepoError::CorruptData)?;
    aggregate.loaded.poll.status = PollStatus::Failed;
    aggregate.loaded.poll.decided_at = Some(decided_at.to_string());
    aggregate.loaded.poll.resolution_note = Some(KEEP_NOTE.to_string());
    let poll_revision = aggregate
        .loaded
        .revision
        .checked_add(1)
        .ok_or(PollRepoError::CorruptData)?;
    let proposal_revision = stored
        .revision
        .checked_add(1)
        .ok_or(PollRepoError::CorruptData)?;
    let poll_item = encode_poll(
        &aggregate.loaded.poll,
        &aggregate.loaded.created_at,
        poll_revision,
    )?;
    let proposal_item =
        encode_proposal(&stored.value, proposal_revision).map_err(proposal_error)?;
    let result = repo
        .transaction()
        .transact_items(condition_action(repo.poll_membership_condition(
            trip_id,
            actor,
            RequiredPollRole::Leader,
        )))
        .transact_items(put_action(
            repo.revision_put(poll_item, aggregate.loaded.revision),
        ))
        .transact_items(put_action(
            repo.revision_put(proposal_item, stored.revision),
        ))
        .send()
        .await;
    match result {
        Ok(_) => aggregate.poll(),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            Err(PollRepoError::Conflict)
        }
        Err(_) => Err(PollRepoError::Unavailable),
    }
}

async fn close_adopt(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    mut aggregate: LoadedPollAggregate,
    decided_at: &str,
    application_ids: ProposalApplicationIds,
) -> Result<Poll, PollRepoError> {
    let stored = linked_pending_proposal(repo, trip_id, &aggregate.loaded.poll).await?;
    let meta = repo
        .proposal_trip_meta(trip_id)
        .await
        .map_err(proposal_error)?;
    let current = meta
        .value
        .current_plan_version
        .ok_or(PollRepoError::CorruptData)?;
    let base = stored.value.change_set.base_plan_version;
    if current < base {
        return Err(PollRepoError::CorruptData);
    }
    if current > base {
        return close_stale(repo, trip_id, actor, aggregate, stored, decided_at).await;
    }
    let proposal_revision = stored.revision;
    let prepared = prepare_application(
        repo,
        trip_id,
        actor,
        stored.value,
        meta,
        ApplicationCommand {
            decision: ProposalDecision::Poll {
                poll_id: aggregate.loaded.poll.id.clone(),
            },
            applied_at: decided_at,
            ids: application_ids,
        },
    )
    .await
    .map_err(proposal_error)?;
    aggregate.loaded.poll.status = PollStatus::Passed;
    aggregate.loaded.poll.decided_at = Some(decided_at.to_string());
    aggregate.loaded.poll.resolution_note = None;
    let next = aggregate
        .loaded
        .revision
        .checked_add(1)
        .ok_or(PollRepoError::CorruptData)?;
    let poll_item = encode_poll(&aggregate.loaded.poll, &aggregate.loaded.created_at, next)?;
    publish_application(
        repo,
        trip_id,
        actor,
        prepared,
        ProposalWrite::Update {
            revision: proposal_revision,
        },
        vec![poll_item.clone()],
        vec![put_action(
            repo.revision_put(poll_item, aggregate.loaded.revision),
        )],
    )
    .await
    .map_err(proposal_error)?;
    aggregate.poll()
}

async fn close_stale(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    mut aggregate: LoadedPollAggregate,
    mut stored: crate::dynamodb::proposal_repo::access::Loaded<Proposal>,
    decided_at: &str,
) -> Result<Poll, PollRepoError> {
    let base = stored.value.change_set.base_plan_version;
    stored.value.status = ProposalStatus::Stale;
    validate_stored_proposal(trip_id, &stored.value).map_err(|_| PollRepoError::CorruptData)?;
    aggregate.loaded.poll.status = PollStatus::Failed;
    aggregate.loaded.poll.decided_at = Some(decided_at.to_string());
    aggregate.loaded.poll.resolution_note = Some(STALE_NOTE.to_string());
    let poll_revision = aggregate
        .loaded
        .revision
        .checked_add(1)
        .ok_or(PollRepoError::CorruptData)?;
    let proposal_revision = stored
        .revision
        .checked_add(1)
        .ok_or(PollRepoError::CorruptData)?;
    let poll_item = encode_poll(
        &aggregate.loaded.poll,
        &aggregate.loaded.created_at,
        poll_revision,
    )?;
    let proposal_item =
        encode_proposal(&stored.value, proposal_revision).map_err(proposal_error)?;
    let result = repo
        .transaction()
        .transact_items(condition_action(repo.poll_membership_condition(
            trip_id,
            actor,
            RequiredPollRole::Leader,
        )))
        .transact_items(condition_action(repo.stale_plan_condition(trip_id, base)))
        .transact_items(put_action(
            repo.revision_put(poll_item, aggregate.loaded.revision),
        ))
        .transact_items(put_action(
            repo.revision_put(proposal_item, stored.revision),
        ))
        .send()
        .await;
    match result {
        Ok(_) => aggregate.poll(),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            Err(PollRepoError::Conflict)
        }
        Err(_) => Err(PollRepoError::Unavailable),
    }
}

async fn linked_pending_proposal(
    repo: &DynamoUserRepo,
    trip_id: &str,
    poll: &Poll,
) -> Result<crate::dynamodb::proposal_repo::access::Loaded<Proposal>, PollRepoError> {
    let proposal_id = poll
        .options
        .iter()
        .find_map(|option| option.proposal_id.as_deref())
        .ok_or(PollRepoError::CorruptData)?;
    let stored = get_proposal(repo, trip_id, proposal_id)
        .await
        .map_err(proposal_error)?
        .ok_or(PollRepoError::CorruptData)?;
    if stored.value.status != ProposalStatus::Pending
        || stored.value.route != ProposalRoute::Poll
        || stored.value.decided_by
            != Some(ProposalDecision::Poll {
                poll_id: poll.id.clone(),
            })
    {
        return Err(PollRepoError::CorruptData);
    }
    Ok(stored)
}

fn plan_change_poll(
    trip_id: &str,
    actor: &UserId,
    proposal: &Proposal,
    new: NewPlanChangePoll,
    quorum: u32,
) -> Poll {
    Poll {
        id: new.poll_id,
        trip_id: trip_id.to_string(),
        created_by: actor.0.clone(),
        kind: PollKind::PlanChange,
        title: proposal.title.clone(),
        description: proposal.rationale.clone(),
        options: vec![
            PollOption {
                id: new.adopt_option_id,
                label: ADOPT_LABEL.to_string(),
                proposal_id: Some(proposal.id.clone()),
            },
            PollOption {
                id: new.keep_option_id,
                label: KEEP_LABEL.to_string(),
                proposal_id: None,
            },
        ],
        opens_at: None,
        closes_at: new.closes_at,
        decided_at: None,
        quorum,
        allow_multi: false,
        status: PollStatus::Open,
        votes: vec![],
        resolution_note: None,
    }
}
