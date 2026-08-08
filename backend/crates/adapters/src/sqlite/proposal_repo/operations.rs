//! Proposal reads and leader-routed lifecycle operations.

use itinera_core::{
    domain::{
        proposal::{Proposal, ProposalDecision, ProposalRoute, ProposalStatus},
        trip::TripRole,
    },
    ports::{
        authorization::TripAuthorizationContext,
        proposal::{ProposalApplicationIds, ProposalRepoError},
        trip::TripRepoError,
    },
    services::proposals::validate_stored_proposal,
};
use sqlx::{Sqlite, Transaction};

use crate::sqlite::{
    SqliteDb,
    codec::{next_revision, validate_id},
    trip_repo::access::{RequiredRole, authorize, validate_trip_aggregate},
};

use super::{
    publication,
    records::{
        LoadedProposal, MAX_PROPOSAL_RESPONSE_BYTES, MAX_PROPOSALS, PROPOSAL_QUERY_LIMIT,
        ProposalRow, encode_proposal, encoded_response_size, sort_newest,
    },
};

pub(super) async fn list_proposals(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<Proposal>, ProposalRepoError> {
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
    let proposals = load_proposals(&mut transaction, trip_id).await?;
    validate_proposal_links(&mut transaction, trip_id, &proposals).await?;
    let result = proposals
        .into_iter()
        .map(|proposal| proposal.value)
        .collect();
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(super) async fn create_proposal(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    proposal: Proposal,
    application_ids: ProposalApplicationIds,
) -> Result<Proposal, ProposalRepoError> {
    validate_requested_id(trip_id)?;
    validate_stored_proposal(trip_id, &proposal).map_err(corrupt)?;
    let actor = authorization
        .human_user_id()
        .ok_or(ProposalRepoError::Forbidden)?;
    if proposal.created_by != actor.0
        || proposal.route != ProposalRoute::LeaderApproval
        || proposal.status != ProposalStatus::Pending
        || proposal.decided_by.is_some()
        || proposal.rejection_reason.is_some()
    {
        return Err(ProposalRepoError::CorruptData);
    }

    let mut transaction = db.begin_immediate().await.map_err(unavailable)?;
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
    let existing = load_proposals(&mut transaction, trip_id).await?;
    validate_proposal_links(&mut transaction, trip_id, &existing).await?;
    if existing.iter().any(|stored| stored.value.id == proposal.id) {
        return Err(ProposalRepoError::Conflict);
    }
    require_current_base(
        &mut transaction,
        trip_id,
        proposal.change_set.base_plan_version,
    )
    .await?;

    let applied_at = proposal.created_at.clone();
    let result = if role == TripRole::Leader {
        publication::publish(
            &mut transaction,
            trip_id,
            actor,
            proposal,
            None,
            publication::PublicationCommand {
                decision: ProposalDecision::Leader {
                    user_id: actor.0.clone(),
                },
                applied_at: &applied_at,
                application_ids: &application_ids,
                terminal_poll_write_bytes: 0,
            },
        )
        .await?
    } else {
        publication::preflight(
            &mut transaction,
            trip_id,
            &proposal,
            &applied_at,
            &application_ids,
            0,
        )
        .await?;
        ensure_projection(&existing, None, &proposal)?;
        insert_proposal(&mut transaction, trip_id, &proposal, 1).await?;
        proposal
    };
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(super) async fn approve_proposal(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    proposal_id: &str,
    applied_at: &str,
    application_ids: ProposalApplicationIds,
) -> Result<Proposal, ProposalRepoError> {
    validate_requested_id(trip_id)?;
    validate_requested_id(proposal_id)?;
    let actor = authorization
        .human_user_id()
        .ok_or(ProposalRepoError::Forbidden)?;
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
    let proposals = load_proposals(&mut transaction, trip_id).await?;
    validate_proposal_links(&mut transaction, trip_id, &proposals).await?;
    let stored = proposals
        .iter()
        .find(|proposal| proposal.value.id == proposal_id)
        .cloned()
        .ok_or(ProposalRepoError::NotFound)?;
    if stored.value.route != ProposalRoute::LeaderApproval {
        return Err(ProposalRepoError::Conflict);
    }
    match stored.value.status {
        ProposalStatus::Applied => {
            db.commit(transaction).await.map_err(unavailable)?;
            return Ok(stored.value);
        }
        ProposalStatus::Pending => {}
        ProposalStatus::Rejected | ProposalStatus::Stale => {
            return Err(ProposalRepoError::Conflict);
        }
        ProposalStatus::Draft | ProposalStatus::Approved => {
            return Err(ProposalRepoError::CorruptData);
        }
    }
    let current = current_plan_version(&mut transaction, trip_id).await?;
    let base = stored.value.change_set.base_plan_version;
    if current < base {
        return Err(ProposalRepoError::CorruptData);
    }
    if current > base {
        let mut stale = stored.value.clone();
        stale.status = ProposalStatus::Stale;
        validate_stored_proposal(trip_id, &stale).map_err(corrupt)?;
        ensure_projection(&proposals, Some(proposal_id), &stale)?;
        update_proposal(&mut transaction, trip_id, &stale, stored.revision).await?;
        db.commit(transaction).await.map_err(unavailable)?;
        return Err(ProposalRepoError::Conflict);
    }

    let result = publication::publish(
        &mut transaction,
        trip_id,
        actor,
        stored.value,
        Some(stored.revision),
        publication::PublicationCommand {
            decision: ProposalDecision::Leader {
                user_id: actor.0.clone(),
            },
            applied_at,
            application_ids: &application_ids,
            terminal_poll_write_bytes: 0,
        },
    )
    .await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(result)
}

pub(super) async fn reject_proposal(
    db: &SqliteDb,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    proposal_id: &str,
    reason: &str,
) -> Result<Proposal, ProposalRepoError> {
    validate_requested_id(trip_id)?;
    validate_requested_id(proposal_id)?;
    let actor = authorization
        .human_user_id()
        .ok_or(ProposalRepoError::Forbidden)?;
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
    let proposals = load_proposals(&mut transaction, trip_id).await?;
    validate_proposal_links(&mut transaction, trip_id, &proposals).await?;
    let stored = proposals
        .iter()
        .find(|proposal| proposal.value.id == proposal_id)
        .cloned()
        .ok_or(ProposalRepoError::NotFound)?;
    if stored.value.route != ProposalRoute::LeaderApproval {
        return Err(ProposalRepoError::Conflict);
    }
    match stored.value.status {
        ProposalStatus::Rejected => {
            db.commit(transaction).await.map_err(unavailable)?;
            return Ok(stored.value);
        }
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
    validate_stored_proposal(trip_id, &rejected).map_err(corrupt)?;
    ensure_projection(&proposals, Some(proposal_id), &rejected)?;
    update_proposal(&mut transaction, trip_id, &rejected, stored.revision).await?;
    db.commit(transaction).await.map_err(unavailable)?;
    Ok(rejected)
}

pub(in crate::sqlite) async fn load_proposals(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<LoadedProposal>, ProposalRepoError> {
    let (count, raw_bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM( \
             length(CAST(trip_id AS BLOB)) + length(CAST(id AS BLOB)) \
             + length(CAST(created_by AS BLOB)) + length(CAST(source_kind AS BLOB)) \
             + COALESCE(length(CAST(source_service_id AS BLOB)), 0) \
             + COALESCE(length(CAST(source_service_name AS BLOB)), 0) \
             + length(CAST(title AS BLOB)) + length(CAST(rationale AS BLOB)) \
             + length(CAST(change_set_json AS BLOB)) + length(CAST(route AS BLOB)) \
             + length(CAST(status AS BLOB)) \
             + COALESCE(length(CAST(decision_kind AS BLOB)), 0) \
             + COALESCE(length(CAST(decision_user_id AS BLOB)), 0) \
             + COALESCE(length(CAST(decision_poll_id AS BLOB)), 0) \
             + COALESCE(length(CAST(rejection_reason AS BLOB)), 0) \
             + length(CAST(created_at AS BLOB)) \
         ), 0) FROM proposals WHERE trip_id = ?",
    )
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let count = usize::try_from(count).map_err(corrupt)?;
    let raw_bytes = usize::try_from(raw_bytes).map_err(corrupt)?;
    if count > MAX_PROPOSALS || raw_bytes > MAX_PROPOSAL_RESPONSE_BYTES {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    let rows = sqlx::query_as::<_, ProposalRow>(
        "SELECT trip_id AS proposal_trip_id, id AS proposal_id, \
                created_by AS proposal_created_by, source_kind, source_service_id, \
                source_service_name, title AS proposal_title, \
                rationale AS proposal_rationale, change_set_json, \
                route AS proposal_route, status AS proposal_status, decision_kind, \
                decision_user_id, decision_poll_id, rejection_reason, \
                created_at AS proposal_created_at, revision AS proposal_revision \
         FROM proposals WHERE trip_id = ? LIMIT ?",
    )
    .bind(trip_id)
    .bind(PROPOSAL_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() != count {
        return Err(ProposalRepoError::CorruptData);
    }
    let mut proposals = rows
        .into_iter()
        .map(|row| row.into_proposal(trip_id))
        .collect::<Result<Vec<_>, _>>()?;
    sort_newest(&mut proposals)?;
    let values = proposals
        .iter()
        .map(|proposal| proposal.value.clone())
        .collect::<Vec<_>>();
    if encoded_response_size(&values)? > MAX_PROPOSAL_RESPONSE_BYTES {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    Ok(proposals)
}

pub(in crate::sqlite) async fn load_applied_proposals(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<Vec<LoadedProposal>, ProposalRepoError> {
    let (count, raw_bytes): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM( \
             length(CAST(trip_id AS BLOB)) + length(CAST(id AS BLOB)) \
             + length(CAST(created_by AS BLOB)) + length(CAST(source_kind AS BLOB)) \
             + COALESCE(length(CAST(source_service_id AS BLOB)), 0) \
             + COALESCE(length(CAST(source_service_name AS BLOB)), 0) \
             + length(CAST(title AS BLOB)) + length(CAST(rationale AS BLOB)) \
             + length(CAST(change_set_json AS BLOB)) + length(CAST(route AS BLOB)) \
             + length(CAST(status AS BLOB)) \
             + COALESCE(length(CAST(decision_kind AS BLOB)), 0) \
             + COALESCE(length(CAST(decision_user_id AS BLOB)), 0) \
             + COALESCE(length(CAST(decision_poll_id AS BLOB)), 0) \
             + COALESCE(length(CAST(rejection_reason AS BLOB)), 0) \
             + length(CAST(created_at AS BLOB)) \
         ), 0) FROM proposals WHERE trip_id = ? AND status = 'applied'",
    )
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let count = usize::try_from(count).map_err(corrupt)?;
    let raw_bytes = usize::try_from(raw_bytes).map_err(corrupt)?;
    if count > MAX_PROPOSALS || raw_bytes > MAX_PROPOSAL_RESPONSE_BYTES {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    let rows = sqlx::query_as::<_, ProposalRow>(
        "SELECT trip_id AS proposal_trip_id, id AS proposal_id, \
                created_by AS proposal_created_by, source_kind, source_service_id, \
                source_service_name, title AS proposal_title, \
                rationale AS proposal_rationale, change_set_json, \
                route AS proposal_route, status AS proposal_status, decision_kind, \
                decision_user_id, decision_poll_id, rejection_reason, \
                created_at AS proposal_created_at, revision AS proposal_revision \
         FROM proposals WHERE trip_id = ? AND status = 'applied' LIMIT ?",
    )
    .bind(trip_id)
    .bind(PROPOSAL_QUERY_LIMIT)
    .fetch_all(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if rows.len() != count {
        return Err(ProposalRepoError::CorruptData);
    }
    let mut proposals = rows
        .into_iter()
        .map(|row| row.into_proposal(trip_id))
        .collect::<Result<Vec<_>, _>>()?;
    sort_newest(&mut proposals)?;
    let values = proposals
        .iter()
        .map(|proposal| proposal.value.clone())
        .collect::<Vec<_>>();
    if encoded_response_size(&values)? > MAX_PROPOSAL_RESPONSE_BYTES {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    Ok(proposals)
}

pub(in crate::sqlite) async fn load_proposal(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    proposal_id: &str,
) -> Result<Option<LoadedProposal>, ProposalRepoError> {
    let row = sqlx::query_as::<_, ProposalRow>(
        "SELECT trip_id AS proposal_trip_id, id AS proposal_id, \
                created_by AS proposal_created_by, source_kind, source_service_id, \
                source_service_name, title AS proposal_title, \
                rationale AS proposal_rationale, change_set_json, \
                route AS proposal_route, status AS proposal_status, decision_kind, \
                decision_user_id, decision_poll_id, rejection_reason, \
                created_at AS proposal_created_at, revision AS proposal_revision \
         FROM proposals WHERE trip_id = ? AND id = ?",
    )
    .bind(trip_id)
    .bind(proposal_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(unavailable)?;
    row.map(|row| row.into_proposal(trip_id)).transpose()
}

pub(in crate::sqlite) async fn insert_proposal(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    proposal: &Proposal,
    revision: i64,
) -> Result<(), ProposalRepoError> {
    let encoded = encode_proposal(trip_id, proposal)?;
    sqlx::query(
        "INSERT INTO proposals ( \
             trip_id, id, created_by, source_kind, source_service_id, \
             source_service_name, title, rationale, change_set_json, route, status, \
             decision_kind, decision_user_id, decision_poll_id, rejection_reason, \
             created_at, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(trip_id)
    .bind(&proposal.id)
    .bind(&proposal.created_by)
    .bind(encoded.source_kind)
    .bind(encoded.source_service_id)
    .bind(encoded.source_service_name)
    .bind(&proposal.title)
    .bind(&proposal.rationale)
    .bind(encoded.change_set_json)
    .bind(encoded.route)
    .bind(encoded.status)
    .bind(encoded.decision_kind)
    .bind(encoded.decision_user_id)
    .bind(encoded.decision_poll_id)
    .bind(&proposal.rejection_reason)
    .bind(&proposal.created_at)
    .bind(revision)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    Ok(())
}

pub(in crate::sqlite) async fn update_proposal(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    proposal: &Proposal,
    revision: i64,
) -> Result<(), ProposalRepoError> {
    let encoded = encode_proposal(trip_id, proposal)?;
    let next = next_revision(revision).map_err(corrupt)?;
    let updated = sqlx::query(
        "UPDATE proposals SET source_kind = ?, source_service_id = ?, \
             source_service_name = ?, title = ?, rationale = ?, change_set_json = ?, \
             route = ?, status = ?, decision_kind = ?, decision_user_id = ?, \
             decision_poll_id = ?, rejection_reason = ?, created_at = ?, revision = ? \
         WHERE trip_id = ? AND id = ? AND revision = ?",
    )
    .bind(encoded.source_kind)
    .bind(encoded.source_service_id)
    .bind(encoded.source_service_name)
    .bind(&proposal.title)
    .bind(&proposal.rationale)
    .bind(encoded.change_set_json)
    .bind(encoded.route)
    .bind(encoded.status)
    .bind(encoded.decision_kind)
    .bind(encoded.decision_user_id)
    .bind(encoded.decision_poll_id)
    .bind(&proposal.rejection_reason)
    .bind(&proposal.created_at)
    .bind(next)
    .bind(trip_id)
    .bind(&proposal.id)
    .bind(revision)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if updated.rows_affected() != 1 {
        return Err(ProposalRepoError::Conflict);
    }
    Ok(())
}

pub(in crate::sqlite) fn ensure_projection(
    current: &[LoadedProposal],
    replaced_id: Option<&str>,
    next: &Proposal,
) -> Result<(), ProposalRepoError> {
    let mut projected = current
        .iter()
        .filter(|proposal| Some(proposal.value.id.as_str()) != replaced_id)
        .map(|proposal| proposal.value.clone())
        .collect::<Vec<_>>();
    projected.push(next.clone());
    if projected.len() > MAX_PROPOSALS
        || encoded_response_size(&projected)? > MAX_PROPOSAL_RESPONSE_BYTES
    {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    Ok(())
}

pub(in crate::sqlite) async fn validate_proposal_plan_links(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    proposals: &[LoadedProposal],
) -> Result<(), ProposalRepoError> {
    let plans = crate::sqlite::trip_repo::plans::load_plan_versions(transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    validate_plan_proposal_links(proposals, &plans)
}

fn validate_plan_proposal_links(
    proposals: &[LoadedProposal],
    plans: &[itinera_core::domain::trip::Plan],
) -> Result<(), ProposalRepoError> {
    let proposals_by_id = proposals
        .iter()
        .map(|proposal| (proposal.value.id.as_str(), proposal))
        .collect::<std::collections::HashMap<_, _>>();
    if proposals_by_id.len() != proposals.len() {
        return Err(ProposalRepoError::CorruptData);
    }
    let mut linked = std::collections::HashSet::new();
    for (index, plan) in plans.iter().enumerate() {
        if usize::try_from(plan.version).ok() != Some(index + 1) {
            return Err(ProposalRepoError::CorruptData);
        }
        let Some(proposal_id) = plan.created_from_proposal_id.as_deref() else {
            if plan.version != 1 {
                return Err(ProposalRepoError::CorruptData);
            }
            continue;
        };
        let proposal = proposals_by_id
            .get(proposal_id)
            .ok_or(ProposalRepoError::CorruptData)?;
        if plan.version == 1
            || proposal.value.status != ProposalStatus::Applied
            || proposal.value.change_set.base_plan_version.checked_add(1) != Some(plan.version)
            || !linked.insert(proposal_id)
        {
            return Err(ProposalRepoError::CorruptData);
        }
    }
    if proposals.iter().any(|proposal| {
        (proposal.value.status == ProposalStatus::Applied)
            != linked.contains(proposal.value.id.as_str())
    }) {
        Err(ProposalRepoError::CorruptData)
    } else {
        Ok(())
    }
}

async fn validate_proposal_links(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    proposals: &[LoadedProposal],
) -> Result<(), ProposalRepoError> {
    let plans = crate::sqlite::trip_repo::plans::load_plan_versions(transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    validate_plan_proposal_links(proposals, &plans)?;
    let polls =
        crate::sqlite::poll_repo::operations::load_proposal_linked_polls(transaction, trip_id)
            .await
            .map_err(map_poll_error)?;
    crate::sqlite::poll_repo::operations::validate_governance_links(&polls, proposals, &plans)
        .map_err(map_poll_error)
}

fn map_poll_error(error: itinera_core::ports::poll::PollRepoError) -> ProposalRepoError {
    match error {
        itinera_core::ports::poll::PollRepoError::Unavailable => ProposalRepoError::Unavailable,
        itinera_core::ports::poll::PollRepoError::SafetyLimitExceeded => {
            ProposalRepoError::SafetyLimitExceeded
        }
        itinera_core::ports::poll::PollRepoError::CorruptData
        | itinera_core::ports::poll::PollRepoError::NotFound
        | itinera_core::ports::poll::PollRepoError::Forbidden
        | itinera_core::ports::poll::PollRepoError::Conflict
        | itinera_core::ports::poll::PollRepoError::InvalidVote => ProposalRepoError::CorruptData,
    }
}

pub(in crate::sqlite) async fn current_plan_version(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<u32, ProposalRepoError> {
    let (plan_id, version): (Option<String>, Option<i64>) =
        sqlx::query_as("SELECT current_plan_id, current_plan_version FROM trips WHERE id = ?")
            .bind(trip_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(unavailable)?;
    match (plan_id, version) {
        (Some(_), Some(version)) => u32::try_from(version)
            .ok()
            .filter(|version| *version > 0)
            .ok_or(ProposalRepoError::CorruptData),
        (None, None) => Err(ProposalRepoError::Conflict),
        _ => Err(ProposalRepoError::CorruptData),
    }
}

async fn require_current_base(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    expected: u32,
) -> Result<(), ProposalRepoError> {
    if current_plan_version(transaction, trip_id).await? == expected {
        Ok(())
    } else {
        Err(ProposalRepoError::Conflict)
    }
}

fn validate_requested_id(value: &str) -> Result<(), ProposalRepoError> {
    validate_id(value).map_err(|_| ProposalRepoError::NotFound)
}

pub(in crate::sqlite) fn map_trip_error(error: TripRepoError) -> ProposalRepoError {
    match error {
        TripRepoError::Unavailable => ProposalRepoError::Unavailable,
        TripRepoError::CorruptData => ProposalRepoError::CorruptData,
        TripRepoError::NotFound => ProposalRepoError::NotFound,
        TripRepoError::Forbidden => ProposalRepoError::Forbidden,
        TripRepoError::Conflict | TripRepoError::DuplicateInvite => ProposalRepoError::Conflict,
    }
}

pub(in crate::sqlite) fn unavailable<T>(_error: T) -> ProposalRepoError {
    ProposalRepoError::Unavailable
}

fn corrupt<T>(_error: T) -> ProposalRepoError {
    ProposalRepoError::CorruptData
}
