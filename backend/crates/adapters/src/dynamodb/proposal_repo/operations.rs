//! Proposal listing, submission, approval, rejection, and stale transitions.

use itinera_core::{
    domain::{
        proposal::{Proposal, ProposalDecision, ProposalRoute, ProposalStatus},
        trip::TripRole,
        user::UserId,
    },
    ports::proposal::{ProposalApplicationIds, ProposalRepoError},
    services::proposals::validate_stored_proposal,
};

use crate::dynamodb::{
    DynamoUserRepo,
    primitives::{condition_action, put_action, transaction_condition_failed},
    trip_repo::records::trip_pk,
};

use super::{
    access::{
        Loaded, MAX_PROPOSAL_BYTES, MAX_PROPOSAL_RECORDS, PROPOSAL_PAGE_SIZE, RequiredProposalRole,
    },
    application::{ApplicationCommand, ProposalWrite, prepare_application, publish_application},
    application_error,
    records::{decode_proposal, encode_proposal, proposal_sk},
};

pub(super) async fn list_proposals(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Vec<Proposal>, ProposalRepoError> {
    repo.proposal_authorize(trip_id, actor, RequiredProposalRole::Any)
        .await?;
    repo.proposal_trip_meta(trip_id).await?;
    let pk = trip_pk(trip_id);
    let mut proposals = repo
        .proposal_query(
            &pk,
            "PROPOSAL#",
            PROPOSAL_PAGE_SIZE,
            MAX_PROPOSAL_RECORDS,
            MAX_PROPOSAL_BYTES,
        )
        .await?
        .into_iter()
        .map(|item| decode_proposal(&item, trip_id).map(|loaded| loaded.value))
        .collect::<Result<Vec<_>, _>>()?;
    proposals.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    let response_bytes = serde_json::to_vec(&proposals)
        .map_err(|_| ProposalRepoError::CorruptData)?
        .len();
    if response_bytes > MAX_PROPOSAL_BYTES {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    Ok(proposals)
}

pub(super) async fn create_proposal(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    proposal: Proposal,
    application_ids: ProposalApplicationIds,
) -> Result<Proposal, ProposalRepoError> {
    let role = repo
        .proposal_authorize(trip_id, actor, RequiredProposalRole::Editor)
        .await?;
    if proposal.route == ProposalRoute::Poll {
        // Poll-routed creation belongs to `PollRepo`, which creates both rows
        // in one transaction. This repository never writes a partial route.
        return Err(ProposalRepoError::Conflict);
    }
    validate_new_proposal(trip_id, actor, &proposal)?;
    let meta = repo.proposal_trip_meta(trip_id).await?;
    let applied_at = proposal.created_at.clone();
    let proposal_id = proposal.id.clone();
    let prepared = prepare_application(
        repo,
        trip_id,
        actor,
        proposal.clone(),
        meta,
        ApplicationCommand {
            decision: ProposalDecision::Leader {
                user_id: actor.0.clone(),
            },
            applied_at: &applied_at,
            ids: application_ids,
        },
    )
    .await?;

    if role == TripRole::Leader {
        return match publish_application(
            repo,
            trip_id,
            actor,
            prepared,
            ProposalWrite::Create,
            vec![],
            vec![],
        )
        .await
        {
            Ok(applied) => Ok(applied),
            Err(ProposalRepoError::Conflict) => {
                repo.proposal_authorize(trip_id, actor, RequiredProposalRole::Leader)
                    .await?;
                if get_proposal(repo, trip_id, &proposal_id).await?.is_some() {
                    return Err(ProposalRepoError::Conflict);
                }
                Err(ProposalRepoError::Conflict)
            }
            Err(error) => Err(error),
        };
    }

    create_pending(repo, trip_id, actor, proposal).await
}

pub(super) async fn approve_proposal(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    proposal_id: &str,
    applied_at: &str,
    application_ids: ProposalApplicationIds,
) -> Result<Proposal, ProposalRepoError> {
    repo.proposal_authorize(trip_id, actor, RequiredProposalRole::Leader)
        .await?;
    let stored = get_proposal(repo, trip_id, proposal_id)
        .await?
        .ok_or(ProposalRepoError::NotFound)?;
    if stored.value.route != ProposalRoute::LeaderApproval {
        return Err(ProposalRepoError::Conflict);
    }
    match stored.value.status {
        ProposalStatus::Applied => return Ok(stored.value),
        ProposalStatus::Pending => {}
        ProposalStatus::Rejected | ProposalStatus::Stale => {
            return Err(ProposalRepoError::Conflict);
        }
        ProposalStatus::Draft | ProposalStatus::Approved => {
            return Err(ProposalRepoError::CorruptData);
        }
    }
    let meta = repo.proposal_trip_meta(trip_id).await?;
    let current_version = meta
        .value
        .current_plan_version
        .ok_or(ProposalRepoError::CorruptData)?;
    if current_version < stored.value.change_set.base_plan_version {
        return Err(ProposalRepoError::CorruptData);
    }
    if current_version > stored.value.change_set.base_plan_version {
        let latest = mark_stale(repo, trip_id, actor, stored).await?;
        return if latest.status == ProposalStatus::Applied {
            Ok(latest)
        } else {
            Err(ProposalRepoError::Conflict)
        };
    }
    let proposal_revision = stored.revision;
    let prepared = prepare_application(
        repo,
        trip_id,
        actor,
        stored.value,
        meta,
        ApplicationCommand {
            decision: ProposalDecision::Leader {
                user_id: actor.0.clone(),
            },
            applied_at,
            ids: application_ids,
        },
    )
    .await?;
    match publish_application(
        repo,
        trip_id,
        actor,
        prepared,
        ProposalWrite::Update {
            revision: proposal_revision,
        },
        vec![],
        vec![],
    )
    .await
    {
        Ok(applied) => Ok(applied),
        Err(ProposalRepoError::Conflict) => {
            classify_approval_conflict(repo, trip_id, actor, proposal_id).await
        }
        Err(error) => Err(error),
    }
}

pub(super) async fn reject_proposal(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    proposal_id: &str,
    reason: &str,
) -> Result<Proposal, ProposalRepoError> {
    repo.proposal_authorize(trip_id, actor, RequiredProposalRole::Leader)
        .await?;
    let stored = get_proposal(repo, trip_id, proposal_id)
        .await?
        .ok_or(ProposalRepoError::NotFound)?;
    if stored.value.route != ProposalRoute::LeaderApproval {
        return Err(ProposalRepoError::Conflict);
    }
    match stored.value.status {
        ProposalStatus::Rejected => return Ok(stored.value),
        ProposalStatus::Pending => {}
        ProposalStatus::Applied | ProposalStatus::Stale => {
            return Err(ProposalRepoError::Conflict);
        }
        ProposalStatus::Draft | ProposalStatus::Approved => {
            return Err(ProposalRepoError::CorruptData);
        }
    }
    let mut rejected = stored.value;
    rejected.status = ProposalStatus::Rejected;
    rejected.decided_by = Some(ProposalDecision::Leader {
        user_id: actor.0.clone(),
    });
    rejected.rejection_reason = Some(reason.to_string());
    validate_stored_proposal(trip_id, &rejected).map_err(application_error)?;
    let next_revision = stored
        .revision
        .checked_add(1)
        .ok_or(ProposalRepoError::CorruptData)?;
    let item = encode_proposal(&rejected, next_revision)?;
    let result = repo
        .transaction()
        .transact_items(condition_action(repo.proposal_membership_condition(
            trip_id,
            actor,
            RequiredProposalRole::Leader,
        )))
        .transact_items(put_action(repo.revision_put(item, stored.revision)))
        .send()
        .await;
    match result {
        Ok(_) => Ok(rejected),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            repo.proposal_authorize(trip_id, actor, RequiredProposalRole::Leader)
                .await?;
            let latest = get_proposal(repo, trip_id, proposal_id)
                .await?
                .ok_or(ProposalRepoError::NotFound)?;
            if latest.value.status == ProposalStatus::Rejected {
                Ok(latest.value)
            } else {
                Err(ProposalRepoError::Conflict)
            }
        }
        Err(_) => Err(ProposalRepoError::Unavailable),
    }
}

async fn create_pending(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    proposal: Proposal,
) -> Result<Proposal, ProposalRepoError> {
    let meta = repo.proposal_trip_meta(trip_id).await?;
    let plan_id = meta
        .value
        .current_plan_id
        .as_deref()
        .ok_or(ProposalRepoError::Conflict)?;
    let version = meta
        .value
        .current_plan_version
        .ok_or(ProposalRepoError::Conflict)?;
    if version != proposal.change_set.base_plan_version {
        return Err(ProposalRepoError::Conflict);
    }
    let item = encode_proposal(&proposal, 1)?;
    let result = repo
        .transaction()
        .transact_items(condition_action(repo.proposal_membership_condition(
            trip_id,
            actor,
            RequiredProposalRole::Editor,
        )))
        .transact_items(condition_action(repo.current_plan_condition(
            trip_id,
            meta.revision,
            plan_id,
            version,
        )))
        .transact_items(put_action(repo.create_only_put(item)))
        .send()
        .await;
    match result {
        Ok(_) => Ok(proposal),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            repo.proposal_authorize(trip_id, actor, RequiredProposalRole::Editor)
                .await?;
            Err(ProposalRepoError::Conflict)
        }
        Err(_) => Err(ProposalRepoError::Unavailable),
    }
}

async fn classify_approval_conflict(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    proposal_id: &str,
) -> Result<Proposal, ProposalRepoError> {
    repo.proposal_authorize(trip_id, actor, RequiredProposalRole::Leader)
        .await?;
    let latest = get_proposal(repo, trip_id, proposal_id)
        .await?
        .ok_or(ProposalRepoError::NotFound)?;
    match latest.value.status {
        ProposalStatus::Applied => Ok(latest.value),
        ProposalStatus::Stale | ProposalStatus::Rejected => Err(ProposalRepoError::Conflict),
        ProposalStatus::Pending => {
            let meta = repo.proposal_trip_meta(trip_id).await?;
            let current_version = meta
                .value
                .current_plan_version
                .ok_or(ProposalRepoError::CorruptData)?;
            if current_version < latest.value.change_set.base_plan_version {
                return Err(ProposalRepoError::CorruptData);
            }
            if current_version > latest.value.change_set.base_plan_version {
                let latest = mark_stale(repo, trip_id, actor, latest).await?;
                if latest.status == ProposalStatus::Applied {
                    return Ok(latest);
                }
            }
            Err(ProposalRepoError::Conflict)
        }
        ProposalStatus::Draft | ProposalStatus::Approved => Err(ProposalRepoError::CorruptData),
    }
}

async fn mark_stale(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    stored: Loaded<Proposal>,
) -> Result<Proposal, ProposalRepoError> {
    if stored.value.status == ProposalStatus::Stale {
        return Ok(stored.value);
    }
    if stored.value.status != ProposalStatus::Pending {
        return Err(ProposalRepoError::Conflict);
    }
    let base_version = stored.value.change_set.base_plan_version;
    let mut stale = stored.value;
    stale.status = ProposalStatus::Stale;
    let next_revision = stored
        .revision
        .checked_add(1)
        .ok_or(ProposalRepoError::CorruptData)?;
    let item = encode_proposal(&stale, next_revision)?;
    let result = repo
        .transaction()
        .transact_items(condition_action(repo.proposal_membership_condition(
            trip_id,
            actor,
            RequiredProposalRole::Leader,
        )))
        .transact_items(condition_action(
            repo.stale_plan_condition(trip_id, base_version),
        ))
        .transact_items(put_action(repo.revision_put(item, stored.revision)))
        .send()
        .await;
    match result {
        Ok(_) => Ok(stale),
        Err(error) if transaction_condition_failed(error.as_service_error()) => {
            repo.proposal_authorize(trip_id, actor, RequiredProposalRole::Leader)
                .await?;
            let latest = get_proposal(repo, trip_id, &stale.id)
                .await?
                .ok_or(ProposalRepoError::NotFound)?;
            if matches!(
                latest.value.status,
                ProposalStatus::Stale | ProposalStatus::Applied
            ) {
                Ok(latest.value)
            } else {
                Err(ProposalRepoError::Conflict)
            }
        }
        Err(_) => Err(ProposalRepoError::Unavailable),
    }
}

pub(in crate::dynamodb) async fn get_proposal(
    repo: &DynamoUserRepo,
    trip_id: &str,
    proposal_id: &str,
) -> Result<Option<Loaded<Proposal>>, ProposalRepoError> {
    let pk = trip_pk(trip_id);
    let sk = proposal_sk(proposal_id);
    repo.proposal_get(&pk, &sk)
        .await?
        .map(|item| decode_proposal(&item, trip_id))
        .transpose()
}

fn validate_new_proposal(
    trip_id: &str,
    actor: &UserId,
    proposal: &Proposal,
) -> Result<(), ProposalRepoError> {
    validate_stored_proposal(trip_id, proposal).map_err(application_error)?;
    if proposal.created_by != actor.0
        || proposal.status != ProposalStatus::Pending
        || proposal.decided_by.is_some()
        || proposal.rejection_reason.is_some()
    {
        return Err(ProposalRepoError::CorruptData);
    }
    Ok(())
}
