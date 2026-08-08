//! Create-only plan publication composed inside the caller's writer transaction.

use std::collections::{HashMap, HashSet};

use itinera_core::{
    domain::{
        content_history::{Edit, EditEntity},
        poll::Poll,
        proposal::{ChangeOp, Proposal, ProposalDecision, ProposalRoute, ProposalStatus},
        trip::{CandidateStatus, CandidateWithPlace, Day, Place, Plan, Stop},
        user::UserId,
    },
    ports::{
        content_history::ContentHistoryRepoError,
        proposal::{ProposalApplicationIds, ProposalRepoError},
    },
    services::{
        plans::validate_stored_plan_graph,
        proposals::{application_entity_id_count, apply_change_set, validate_stored_proposal},
    },
};
use serde::Serialize;
use serde_json::json;
use sqlx::{Sqlite, Transaction};

use crate::sqlite::{
    codec::{next_revision, validate_id},
    history_repo::audit::{AuditChange, append_proposal_edits, audit},
    trip_repo::{
        candidate_records::{
            MAX_CANDIDATE_ITEMS, MAX_RESPONSE_BYTES, encode_candidate_status, encode_place,
            encoded_size,
        },
        candidates::{insert_place, load_candidates, load_place},
        plan_records::{
            MAX_PLAN_PROVENANCE_BYTES, MAX_PLAN_RESPONSE_BYTES, MAX_PLAN_VERSIONS,
            StructuralAuditBinding, encode_booking_columns, encode_structural_audits,
        },
        plans::{load_plan_detail, load_plan_versions, plan_structure_hash},
    },
};

use super::operations::{
    ensure_projection, insert_proposal, load_proposals, map_trip_error, update_proposal,
    validate_proposal_plan_links,
};

const MAX_PUBLICATION_ACTIONS: usize = 100;
const MAX_PUBLICATION_BYTES: usize = 3 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
struct CandidateChange {
    candidate_id: String,
    place_id: String,
    old_status: CandidateStatus,
    new_status: CandidateStatus,
    revision: i64,
}

struct EncodedApplicationProvenance {
    change_set_json: String,
    entity_ids_json: String,
    structural_audits_json: String,
    base_structure_hash: String,
    structure_hash: String,
}

struct PublicationPayload<'a> {
    proposal: &'a Proposal,
    application_entity_ids: &'a [String],
    plan: &'a Plan,
    days: &'a [Day],
    stops: &'a [Stop],
    places: &'a [Place],
    changes: &'a [CandidateChange],
    edits: &'a [Edit],
    structural_audits: &'a [StructuralAuditBinding],
    base_structure_hash: &'a str,
    structure_hash: &'a str,
}

struct ApplicationRows<'a> {
    plan: &'a Plan,
    days: &'a [Day],
    stops: &'a [Stop],
    new_places: &'a [Place],
    new_stop_ids: &'a [String],
    provenance: &'a EncodedApplicationProvenance,
}

impl EncodedApplicationProvenance {
    fn byte_len(&self) -> Result<usize, ProposalRepoError> {
        self.change_set_json
            .len()
            .checked_add(self.entity_ids_json.len())
            .and_then(|bytes| bytes.checked_add(self.structural_audits_json.len()))
            .and_then(|bytes| bytes.checked_add(self.base_structure_hash.len()))
            .and_then(|bytes| bytes.checked_add(self.structure_hash.len()))
            .ok_or(ProposalRepoError::SafetyLimitExceeded)
    }
}

pub(in crate::sqlite) struct PublicationCommand<'a> {
    pub(in crate::sqlite) decision: ProposalDecision,
    pub(in crate::sqlite) applied_at: &'a str,
    pub(in crate::sqlite) application_ids: &'a ProposalApplicationIds,
    pub(in crate::sqlite) terminal_poll_write_bytes: usize,
}

pub(in crate::sqlite) async fn preflight(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    proposal: &Proposal,
    applied_at: &str,
    application_ids: &ProposalApplicationIds,
    terminal_poll_write_bytes: usize,
) -> Result<(), ProposalRepoError> {
    validate_stored_proposal(trip_id, proposal).map_err(corrupt)?;
    let (current_plan_id, current_version, _) = current_pointer(transaction, trip_id).await?;
    if current_version != proposal.change_set.base_plan_version {
        return Err(ProposalRepoError::Conflict);
    }
    let current = load_plan_detail(transaction, trip_id, &current_plan_id, current_version)
        .await
        .map_err(map_trip_error)?;
    let application_time = canonical_utc(applied_at)?;
    if application_time < canonical_utc(&proposal.created_at)?
        || application_time < canonical_utc(&current.plan.created_at)?
    {
        return Err(ProposalRepoError::Conflict);
    }
    let resolved_places =
        load_operation_places(transaction, trip_id, &proposal.change_set.ops).await?;
    let application = apply_change_set(
        &current,
        trip_id,
        &proposal.id,
        &proposal.change_set,
        &resolved_places,
        applied_at,
        application_ids.clone(),
    )
    .map_err(application_error)?;
    let application_entity_ids =
        consumed_application_entity_ids(&proposal.change_set, &application_ids.entity_ids)?;
    validate_stored_plan_graph(
        &application.plan,
        &application.days,
        &application.stops,
        trip_id,
        application.plan.version,
    )
    .map_err(corrupt)?;
    let mut candidates = load_candidates(transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    let changes =
        candidate_changes(transaction, trip_id, &application.stops, &mut candidates).await?;
    if changes.len() > application.audit_ids.len() {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    let actor = UserId(proposal.created_by.clone());
    let edits = changes
        .iter()
        .zip(&application.audit_ids)
        .map(|(change, id)| {
            audit(
                trip_id,
                &actor,
                applied_at,
                id,
                AuditChange {
                    entity: EditEntity::Candidate,
                    entity_id: &change.candidate_id,
                    field: "status",
                    old_value: json!(change.old_status),
                    new_value: json!(change.new_status),
                },
            )
        })
        .collect::<Vec<_>>();
    let structural_audits = edits
        .iter()
        .zip(&changes)
        .map(|(edit, change)| StructuralAuditBinding {
            edit: edit.clone(),
            candidate_place_id: change.place_id.clone(),
        })
        .collect::<Vec<_>>();
    let mut versions = load_plan_versions(transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    versions.push(application.plan.clone());
    if versions.len() > MAX_PLAN_VERSIONS
        || encoded_size(&versions).map_err(map_trip_error)? > MAX_PLAN_RESPONSE_BYTES
        || encoded_size(&candidates).map_err(map_trip_error)? > MAX_RESPONSE_BYTES
    {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    let new_stop_ids = application
        .stops
        .iter()
        .filter(|stop| !current.stops.iter().any(|old| old.id == stop.id))
        .map(|stop| stop.id.clone())
        .collect::<Vec<_>>();
    let new_day_ids = application
        .days
        .iter()
        .filter(|day| !current.days.iter().any(|old| old.id == day.id))
        .map(|day| day.id.clone())
        .collect::<Vec<_>>();
    validate_generated_collisions(
        transaction,
        trip_id,
        &application.plan.id,
        &application.new_places,
        &new_day_ids,
        &new_stop_ids,
    )
    .await?;
    let actions = publication_action_count(
        &application.days,
        &application.stops,
        &application.new_places,
        &new_stop_ids,
        &changes,
        terminal_poll_write_bytes > 0,
        base_structure_hash_requires_write(transaction, trip_id, current.plan.version).await?,
    )?;
    let base_structure_hash = plan_structure_hash(&current.plan, &current.days, &current.stops)
        .map_err(map_trip_error)?;
    let structure_hash =
        plan_structure_hash(&application.plan, &application.days, &application.stops)
            .map_err(map_trip_error)?;
    let bytes = publication_bytes(
        PublicationPayload {
            proposal,
            application_entity_ids,
            plan: &application.plan,
            days: &application.days,
            stops: &application.stops,
            places: &application.new_places,
            changes: &changes,
            edits: &edits,
            structural_audits: &structural_audits,
            base_structure_hash: &base_structure_hash,
            structure_hash: &structure_hash,
        },
        terminal_poll_write_bytes,
    )?;
    if actions > MAX_PUBLICATION_ACTIONS {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    ensure_publication_byte_limit(bytes)?;
    Ok(())
}

pub(in crate::sqlite) async fn publish(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    actor: &UserId,
    mut proposal: Proposal,
    stored_revision: Option<i64>,
    command: PublicationCommand<'_>,
) -> Result<Proposal, ProposalRepoError> {
    let PublicationCommand {
        decision,
        applied_at,
        application_ids,
        terminal_poll_write_bytes,
    } = command;
    validate_stored_proposal(trip_id, &proposal).map_err(corrupt)?;
    validate_decision(&proposal, actor, &decision)?;
    if proposal.status != ProposalStatus::Pending {
        return Err(ProposalRepoError::Conflict);
    }

    let (current_plan_id, current_version, trip_revision) =
        current_pointer(transaction, trip_id).await?;
    if current_version != proposal.change_set.base_plan_version {
        return Err(ProposalRepoError::Conflict);
    }
    let current = load_plan_detail(transaction, trip_id, &current_plan_id, current_version)
        .await
        .map_err(map_trip_error)?;
    let application_time = canonical_utc(applied_at)?;
    if application_time < canonical_utc(&proposal.created_at)?
        || application_time < canonical_utc(&current.plan.created_at)?
    {
        return Err(ProposalRepoError::Conflict);
    }
    let resolved_places =
        load_operation_places(transaction, trip_id, &proposal.change_set.ops).await?;
    let application = apply_change_set(
        &current,
        trip_id,
        &proposal.id,
        &proposal.change_set,
        &resolved_places,
        applied_at,
        application_ids.clone(),
    )
    .map_err(application_error)?;
    let application_entity_ids =
        consumed_application_entity_ids(&proposal.change_set, &application_ids.entity_ids)?;
    validate_stored_plan_graph(
        &application.plan,
        &application.days,
        &application.stops,
        trip_id,
        application.plan.version,
    )
    .map_err(corrupt)?;

    let mut candidates = load_candidates(transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    let changes =
        candidate_changes(transaction, trip_id, &application.stops, &mut candidates).await?;
    if candidates.len() > MAX_CANDIDATE_ITEMS
        || encoded_size(&candidates).map_err(map_trip_error)? > MAX_RESPONSE_BYTES
    {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }

    let mut plan_versions = load_plan_versions(transaction, trip_id)
        .await
        .map_err(map_trip_error)?;
    if plan_versions
        .last()
        .is_none_or(|plan| plan.id != current_plan_id || plan.version != current_version)
    {
        return Err(ProposalRepoError::CorruptData);
    }
    plan_versions.push(application.plan.clone());
    if plan_versions.len() > MAX_PLAN_VERSIONS
        || encoded_size(&plan_versions).map_err(map_trip_error)? > MAX_PLAN_RESPONSE_BYTES
    {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }

    let closes_poll = terminal_poll_write_bytes > 0;
    proposal.status = ProposalStatus::Applied;
    proposal.decided_by = Some(decision);
    proposal.rejection_reason = None;
    validate_stored_proposal(trip_id, &proposal).map_err(corrupt)?;
    let proposals = load_proposals(transaction, trip_id).await?;
    ensure_projection(
        &proposals,
        stored_revision.map(|_| proposal.id.as_str()),
        &proposal,
    )?;

    let new_stop_ids = application
        .stops
        .iter()
        .filter(|stop| !current.stops.iter().any(|old| old.id == stop.id))
        .map(|stop| stop.id.clone())
        .collect::<Vec<_>>();
    let new_day_ids = application
        .days
        .iter()
        .filter(|day| !current.days.iter().any(|old| old.id == day.id))
        .map(|day| day.id.clone())
        .collect::<Vec<_>>();
    validate_generated_collisions(
        transaction,
        trip_id,
        &application.plan.id,
        &application.new_places,
        &new_day_ids,
        &new_stop_ids,
    )
    .await?;

    if changes.len() > application.audit_ids.len() {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    let edits = changes
        .iter()
        .zip(&application.audit_ids)
        .map(|(change, id)| {
            audit(
                trip_id,
                actor,
                applied_at,
                id,
                AuditChange {
                    entity: EditEntity::Candidate,
                    entity_id: &change.candidate_id,
                    field: "status",
                    old_value: json!(change.old_status),
                    new_value: json!(change.new_status),
                },
            )
        })
        .collect::<Vec<_>>();
    let structural_audits = edits
        .iter()
        .zip(&changes)
        .map(|(edit, change)| StructuralAuditBinding {
            edit: edit.clone(),
            candidate_place_id: change.place_id.clone(),
        })
        .collect::<Vec<_>>();
    let base_structure_hash = plan_structure_hash(&current.plan, &current.days, &current.stops)
        .map_err(map_trip_error)?;
    let structure_hash =
        plan_structure_hash(&application.plan, &application.days, &application.stops)
            .map_err(map_trip_error)?;
    let sealed_base =
        ensure_base_structure_hash(transaction, trip_id, current_version, &base_structure_hash)
            .await?;
    let provenance = encode_application_provenance(
        &application.plan,
        &proposal.change_set,
        application_entity_ids,
        &structural_audits,
        &base_structure_hash,
        &structure_hash,
    )?;
    ensure_plan_provenance_projection(transaction, trip_id, &provenance).await?;
    let actions = publication_action_count(
        &application.days,
        &application.stops,
        &application.new_places,
        &new_stop_ids,
        &changes,
        closes_poll,
        sealed_base,
    )?;
    if actions > MAX_PUBLICATION_ACTIONS {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    ensure_publication_byte_limit(publication_bytes(
        PublicationPayload {
            proposal: &proposal,
            application_entity_ids,
            plan: &application.plan,
            days: &application.days,
            stops: &application.stops,
            places: &application.new_places,
            changes: &changes,
            edits: &edits,
            structural_audits: &structural_audits,
            base_structure_hash: &base_structure_hash,
            structure_hash: &structure_hash,
        },
        terminal_poll_write_bytes,
    )?)?;

    let candidate_place_ids = changes
        .iter()
        .map(|change| change.place_id.clone())
        .collect::<Vec<_>>();
    append_proposal_edits(transaction, trip_id, &edits, &candidate_place_ids)
        .await
        .map_err(map_history_error)?;
    match stored_revision {
        Some(revision) => {
            update_proposal(transaction, trip_id, &proposal, revision).await?;
        }
        None => insert_proposal(transaction, trip_id, &proposal, 1).await?,
    }
    insert_application(
        transaction,
        trip_id,
        ApplicationRows {
            plan: &application.plan,
            days: &application.days,
            stops: &application.stops,
            new_places: &application.new_places,
            new_stop_ids: &new_stop_ids,
            provenance: &provenance,
        },
    )
    .await?;
    update_trip_pointer(
        transaction,
        trip_id,
        &current_plan_id,
        current_version,
        trip_revision,
        &application.plan.id,
        application.plan.version,
    )
    .await?;
    for (change, edit) in changes.iter().zip(&edits) {
        let updated = sqlx::query(
            "UPDATE candidates SET status = ?, revision = ? \
             WHERE trip_id = ? AND id = ? AND place_id = ? AND revision = ? AND status = ?",
        )
        .bind(encode_candidate_status(change.new_status))
        .bind(next_revision(change.revision).map_err(corrupt)?)
        .bind(trip_id)
        .bind(&change.candidate_id)
        .bind(&change.place_id)
        .bind(change.revision)
        .bind(encode_candidate_status(change.old_status))
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?;
        if updated.rows_affected() != 1 {
            return Err(ProposalRepoError::Conflict);
        }
        sqlx::query(
            "INSERT INTO proposal_content_edits ( \
                 trip_id, edit_id, proposal_id, candidate_id, candidate_place_id \
             ) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(trip_id)
        .bind(&edit.id)
        .bind(&proposal.id)
        .bind(&change.candidate_id)
        .bind(&change.place_id)
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?;
    }

    let stored = load_plan_detail(
        transaction,
        trip_id,
        &application.plan.id,
        application.plan.version,
    )
    .await
    .map_err(map_trip_error)?;
    if stored.plan != application.plan
        || stored.days != application.days
        || stored.stops != application.stops
    {
        return Err(ProposalRepoError::CorruptData);
    }
    let stored_proposals = load_proposals(transaction, trip_id).await?;
    validate_proposal_plan_links(transaction, trip_id, &stored_proposals).await?;
    Ok(proposal)
}

fn publication_action_count(
    days: &[itinera_core::domain::trip::Day],
    stops: &[Stop],
    new_places: &[Place],
    new_stop_ids: &[String],
    changes: &[CandidateChange],
    closes_poll: bool,
    seals_base: bool,
) -> Result<usize, ProposalRepoError> {
    3_usize
        .checked_add(usize::from(closes_poll))
        .and_then(|value| value.checked_add(usize::from(seals_base)))
        .and_then(|value| value.checked_add(days.len()))
        .and_then(|value| value.checked_add(stops.len()))
        .and_then(|value| value.checked_add(new_places.len()))
        .and_then(|value| value.checked_add(new_stop_ids.len()))
        .and_then(|value| value.checked_add(changes.len().checked_mul(3)?))
        .ok_or(ProposalRepoError::SafetyLimitExceeded)
}

fn validate_decision(
    proposal: &Proposal,
    actor: &UserId,
    decision: &ProposalDecision,
) -> Result<(), ProposalRepoError> {
    match (proposal.route, &proposal.decided_by, decision) {
        (ProposalRoute::LeaderApproval, None, ProposalDecision::Leader { user_id })
            if user_id == &actor.0 =>
        {
            Ok(())
        }
        (
            ProposalRoute::Poll,
            Some(ProposalDecision::Poll { poll_id: stored }),
            ProposalDecision::Poll { poll_id },
        ) if stored == poll_id => Ok(()),
        _ => Err(ProposalRepoError::Conflict),
    }
}

async fn current_pointer(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
) -> Result<(String, u32, i64), ProposalRepoError> {
    let (plan_id, version, revision): (Option<String>, Option<i64>, i64) = sqlx::query_as(
        "SELECT current_plan_id, current_plan_version, revision FROM trips WHERE id = ?",
    )
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let revision = crate::sqlite::codec::checked_revision(revision).map_err(corrupt)?;
    match (plan_id, version) {
        (Some(plan_id), Some(version)) => {
            validate_id(&plan_id).map_err(corrupt)?;
            let version = u32::try_from(version)
                .ok()
                .filter(|version| *version > 0)
                .ok_or(ProposalRepoError::CorruptData)?;
            Ok((plan_id, version, revision))
        }
        (None, None) => Err(ProposalRepoError::Conflict),
        _ => Err(ProposalRepoError::CorruptData),
    }
}

async fn load_operation_places(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    operations: &[ChangeOp],
) -> Result<HashMap<String, Place>, ProposalRepoError> {
    let ids = operations
        .iter()
        .filter_map(|operation| match operation {
            ChangeOp::AddStop { place_id, .. } => Some(place_id),
            ChangeOp::SwapPlace { new_place_id, .. } => Some(new_place_id),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut places = HashMap::with_capacity(ids.len());
    for id in ids {
        let place = load_place(transaction, trip_id, id)
            .await
            .map_err(map_trip_error)?
            .ok_or(ProposalRepoError::NotFound)?;
        places.insert(id.clone(), place);
    }
    Ok(places)
}

async fn candidate_changes(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    resulting_stops: &[Stop],
    candidates: &mut [CandidateWithPlace],
) -> Result<Vec<CandidateChange>, ProposalRepoError> {
    let in_plan = resulting_stops
        .iter()
        .map(|stop| stop.place_id.as_str())
        .collect::<HashSet<_>>();
    let mut changes = Vec::new();
    for candidate in candidates {
        let adopted = in_plan.contains(candidate.candidate.place_id.as_str());
        if adopted && candidate.candidate.status == CandidateStatus::Rejected {
            return Err(ProposalRepoError::InvalidChange);
        }
        let desired = if adopted {
            CandidateStatus::InPlan
        } else if candidate.candidate.status == CandidateStatus::InPlan {
            CandidateStatus::Shortlisted
        } else {
            candidate.candidate.status
        };
        if desired == candidate.candidate.status {
            continue;
        }
        let revision: i64 = sqlx::query_scalar(
            "SELECT revision FROM candidates WHERE trip_id = ? AND id = ? AND place_id = ?",
        )
        .bind(trip_id)
        .bind(&candidate.candidate.id)
        .bind(&candidate.candidate.place_id)
        .fetch_one(&mut **transaction)
        .await
        .map_err(unavailable)?;
        crate::sqlite::codec::checked_revision(revision).map_err(corrupt)?;
        changes.push(CandidateChange {
            candidate_id: candidate.candidate.id.clone(),
            place_id: candidate.candidate.place_id.clone(),
            old_status: candidate.candidate.status,
            new_status: desired,
            revision,
        });
        candidate.candidate.status = desired;
    }
    Ok(changes)
}

async fn validate_generated_collisions(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    plan_id: &str,
    new_places: &[Place],
    new_day_ids: &[String],
    new_stop_ids: &[String],
) -> Result<(), ProposalRepoError> {
    let plan_collision: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM plans WHERE trip_id = ? AND id = ?")
            .bind(trip_id)
            .bind(plan_id)
            .fetch_one(&mut **transaction)
            .await
            .map_err(unavailable)?;
    if plan_collision != 0 {
        return Err(ProposalRepoError::CorruptData);
    }
    for place in new_places {
        let collision: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trip_places WHERE trip_id = ? AND id = ?")
                .bind(trip_id)
                .bind(&place.id)
                .fetch_one(&mut **transaction)
                .await
                .map_err(unavailable)?;
        if collision != 0 {
            return Err(ProposalRepoError::CorruptData);
        }
    }
    for day_id in new_day_ids {
        let collision: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM plan_days WHERE trip_id = ? AND id = ?")
                .bind(trip_id)
                .bind(day_id)
                .fetch_one(&mut **transaction)
                .await
                .map_err(unavailable)?;
        if collision != 0 {
            return Err(ProposalRepoError::CorruptData);
        }
    }
    for stop_id in new_stop_ids {
        let collision: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stop_identities WHERE trip_id = ? AND id = ?")
                .bind(trip_id)
                .bind(stop_id)
                .fetch_one(&mut **transaction)
                .await
                .map_err(unavailable)?;
        if collision != 0 {
            return Err(ProposalRepoError::CorruptData);
        }
    }
    Ok(())
}

fn encode_application_provenance(
    plan: &itinera_core::domain::trip::Plan,
    change_set: &itinera_core::domain::proposal::ChangeSet,
    application_entity_ids: &[String],
    structural_audits: &[StructuralAuditBinding],
    base_structure_hash: &str,
    structure_hash: &str,
) -> Result<EncodedApplicationProvenance, ProposalRepoError> {
    Ok(EncodedApplicationProvenance {
        change_set_json: serde_json::to_string(change_set).map_err(corrupt)?,
        entity_ids_json: serde_json::to_string(application_entity_ids).map_err(corrupt)?,
        structural_audits_json: encode_structural_audits(plan, structural_audits)
            .map_err(map_trip_error)?,
        base_structure_hash: base_structure_hash.to_string(),
        structure_hash: structure_hash.to_string(),
    })
}

async fn ensure_plan_provenance_projection(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    provenance: &EncodedApplicationProvenance,
) -> Result<(), ProposalRepoError> {
    let current: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM( \
             length(CAST(COALESCE(applied_change_set_json, '') AS BLOB)) + \
             length(CAST(COALESCE(application_entity_ids_json, '') AS BLOB)) + \
             length(CAST(COALESCE(structural_audits_json, '') AS BLOB)) + \
             length(CAST(COALESCE(base_structure_hash, '') AS BLOB)) + \
             length(CAST(COALESCE(structure_hash, '') AS BLOB)) \
         ), 0) FROM plans WHERE trip_id = ?",
    )
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    let current = usize::try_from(current).map_err(corrupt)?;
    checked_plan_provenance_projection(current, provenance.byte_len()?)
}

fn checked_plan_provenance_projection(
    current: usize,
    additional: usize,
) -> Result<(), ProposalRepoError> {
    if current
        .checked_add(additional)
        .is_some_and(|projected| projected <= MAX_PLAN_PROVENANCE_BYTES)
    {
        Ok(())
    } else {
        Err(ProposalRepoError::SafetyLimitExceeded)
    }
}

async fn insert_application(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    rows: ApplicationRows<'_>,
) -> Result<(), ProposalRepoError> {
    for place in rows.new_places {
        let encoded = encode_place(place).map_err(map_trip_error)?;
        insert_place(transaction, trip_id, place, encoded)
            .await
            .map_err(map_trip_error)?;
    }
    sqlx::query(
        "INSERT INTO plans ( \
             trip_id, version, id, created_from_proposal_id, created_at, \
             applied_change_set_json, application_entity_ids_json, structural_audits_json, \
             base_structure_hash, structure_hash, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
    )
    .bind(trip_id)
    .bind(i64::from(rows.plan.version))
    .bind(&rows.plan.id)
    .bind(&rows.plan.created_from_proposal_id)
    .bind(&rows.plan.created_at)
    .bind(&rows.provenance.change_set_json)
    .bind(&rows.provenance.entity_ids_json)
    .bind(&rows.provenance.structural_audits_json)
    .bind(&rows.provenance.base_structure_hash)
    .bind(&rows.provenance.structure_hash)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    for day in rows.days {
        sqlx::query(
            "INSERT INTO plan_days ( \
                 trip_id, plan_version, id, plan_id, date, city_hint, tz, \
                 window_start, window_end, revision \
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
        )
        .bind(trip_id)
        .bind(i64::from(rows.plan.version))
        .bind(&day.id)
        .bind(&day.plan_id)
        .bind(&day.date)
        .bind(&day.city_hint)
        .bind(&day.tz)
        .bind(&day.window_start)
        .bind(&day.window_end)
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?;
    }
    for stop_id in rows.new_stop_ids {
        sqlx::query("INSERT INTO stop_identities (trip_id, id) VALUES (?, ?)")
            .bind(trip_id)
            .bind(stop_id)
            .execute(&mut **transaction)
            .await
            .map_err(unavailable)?;
    }
    for stop in rows.stops {
        let booking = encode_booking_columns(stop.booking.as_ref()).map_err(map_trip_error)?;
        sqlx::query(
            "INSERT INTO plan_stops ( \
                 trip_id, plan_version, id, day_id, seq, place_id, stop_kind, \
                 planned_arrival, duration_min, booking_ref, booking_url, \
                 booking_cost_amount, booking_cost_currency, notes, revision \
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
        )
        .bind(trip_id)
        .bind(i64::from(rows.plan.version))
        .bind(&stop.id)
        .bind(&stop.day_id)
        .bind(stop.seq)
        .bind(&stop.place_id)
        .bind(stop.stop_kind.as_ref())
        .bind(&stop.planned_arrival)
        .bind(i64::from(stop.duration_min))
        .bind(booking.reference)
        .bind(booking.url)
        .bind(booking.amount)
        .bind(booking.currency)
        .bind(&stop.notes)
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?;
    }
    Ok(())
}

async fn ensure_base_structure_hash(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    version: u32,
    expected: &str,
) -> Result<bool, ProposalRepoError> {
    let sealed = if version == 1 {
        sqlx::query(
            "UPDATE plans SET structure_hash = ? \
             WHERE trip_id = ? AND version = 1 AND structure_hash IS NULL",
        )
        .bind(expected)
        .bind(trip_id)
        .execute(&mut **transaction)
        .await
        .map_err(unavailable)?
        .rows_affected()
            == 1
    } else {
        false
    };
    let stored: Option<String> =
        sqlx::query_scalar("SELECT structure_hash FROM plans WHERE trip_id = ? AND version = ?")
            .bind(trip_id)
            .bind(i64::from(version))
            .fetch_one(&mut **transaction)
            .await
            .map_err(unavailable)?;
    if stored.as_deref() == Some(expected) {
        Ok(sealed)
    } else {
        Err(ProposalRepoError::CorruptData)
    }
}

async fn base_structure_hash_requires_write(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    version: u32,
) -> Result<bool, ProposalRepoError> {
    if version != 1 {
        return Ok(false);
    }
    let missing: i64 = sqlx::query_scalar(
        "SELECT structure_hash IS NULL FROM plans WHERE trip_id = ? AND version = 1",
    )
    .bind(trip_id)
    .fetch_one(&mut **transaction)
    .await
    .map_err(unavailable)?;
    match missing {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(ProposalRepoError::CorruptData),
    }
}

async fn update_trip_pointer(
    transaction: &mut Transaction<'static, Sqlite>,
    trip_id: &str,
    old_plan_id: &str,
    old_version: u32,
    old_revision: i64,
    new_plan_id: &str,
    new_version: u32,
) -> Result<(), ProposalRepoError> {
    let updated = sqlx::query(
        "UPDATE trips SET current_plan_id = ?, current_plan_version = ?, revision = ? \
         WHERE id = ? AND current_plan_id = ? AND current_plan_version = ? AND revision = ?",
    )
    .bind(new_plan_id)
    .bind(i64::from(new_version))
    .bind(next_revision(old_revision).map_err(corrupt)?)
    .bind(trip_id)
    .bind(old_plan_id)
    .bind(i64::from(old_version))
    .bind(old_revision)
    .execute(&mut **transaction)
    .await
    .map_err(unavailable)?;
    if updated.rows_affected() == 1 {
        Ok(())
    } else {
        Err(ProposalRepoError::Conflict)
    }
}

fn publication_bytes(
    payload: PublicationPayload<'_>,
    terminal_poll_write_bytes: usize,
) -> Result<usize, ProposalRepoError> {
    serde_json::to_vec(&(
        payload.proposal,
        &payload.proposal.change_set,
        payload.application_entity_ids,
        payload.plan,
        payload.days,
        payload.stops,
        payload.places,
        payload.changes,
        payload.edits,
        payload.structural_audits,
        payload.base_structure_hash,
        payload.structure_hash,
    ))
    .map(|bytes| bytes.len())
    .map_err(corrupt)
    .and_then(|bytes| {
        bytes
            .checked_add(terminal_poll_write_bytes)
            .ok_or(ProposalRepoError::SafetyLimitExceeded)
    })
}

fn ensure_publication_byte_limit(bytes: usize) -> Result<(), ProposalRepoError> {
    if bytes <= MAX_PUBLICATION_BYTES {
        Ok(())
    } else {
        Err(ProposalRepoError::SafetyLimitExceeded)
    }
}

pub(in crate::sqlite) fn terminal_poll_write_bytes(
    poll: &Poll,
    created_at: &str,
) -> Result<usize, ProposalRepoError> {
    serde_json::to_vec(&(
        poll.trip_id.as_str(),
        poll.id.as_str(),
        poll.created_by.as_str(),
        poll.kind,
        poll.title.as_str(),
        poll.description.as_str(),
        created_at,
        poll.opens_at.as_deref(),
        poll.closes_at.as_str(),
        poll.decided_at.as_deref(),
        poll.quorum,
        poll.allow_multi,
        poll.status,
        poll.resolution_note.as_deref(),
    ))
    .map(|bytes| bytes.len())
    .map_err(corrupt)
}

fn consumed_application_entity_ids<'a>(
    change_set: &itinera_core::domain::proposal::ChangeSet,
    reserved: &'a [String],
) -> Result<&'a [String], ProposalRepoError> {
    reserved
        .get(..application_entity_id_count(change_set))
        .ok_or(ProposalRepoError::CorruptData)
}

fn canonical_utc(value: &str) -> Result<chrono::DateTime<chrono::FixedOffset>, ProposalRepoError> {
    if value.len() > 64 || !value.ends_with('Z') {
        return Err(ProposalRepoError::CorruptData);
    }
    let timestamp = chrono::DateTime::parse_from_rfc3339(value).map_err(corrupt)?;
    if timestamp.offset().local_minus_utc() == 0 {
        Ok(timestamp)
    } else {
        Err(ProposalRepoError::CorruptData)
    }
}

fn application_error(
    error: itinera_core::services::proposals::ChangeApplicationError,
) -> ProposalRepoError {
    match error {
        itinera_core::services::proposals::ChangeApplicationError::CorruptData => {
            ProposalRepoError::CorruptData
        }
        itinera_core::services::proposals::ChangeApplicationError::NotFound => {
            ProposalRepoError::NotFound
        }
        itinera_core::services::proposals::ChangeApplicationError::InvalidChange => {
            ProposalRepoError::InvalidChange
        }
    }
}

fn map_history_error(error: ContentHistoryRepoError) -> ProposalRepoError {
    match error {
        ContentHistoryRepoError::Unavailable => ProposalRepoError::Unavailable,
        ContentHistoryRepoError::SafetyLimitExceeded => ProposalRepoError::SafetyLimitExceeded,
        ContentHistoryRepoError::Conflict => ProposalRepoError::Conflict,
        ContentHistoryRepoError::CorruptData
        | ContentHistoryRepoError::NotFound
        | ContentHistoryRepoError::Forbidden
        | ContentHistoryRepoError::Unsupported => ProposalRepoError::CorruptData,
    }
}

fn unavailable<T>(_error: T) -> ProposalRepoError {
    ProposalRepoError::Unavailable
}

fn corrupt<T>(_error: T) -> ProposalRepoError {
    ProposalRepoError::CorruptData
}

#[cfg(test)]
mod tests {
    use itinera_core::ports::proposal::ProposalRepoError;

    use super::{
        MAX_PLAN_PROVENANCE_BYTES, MAX_PUBLICATION_BYTES, checked_plan_provenance_projection,
        ensure_publication_byte_limit,
    };

    #[test]
    fn plan_provenance_projection_accepts_the_exact_limit_only() {
        assert_eq!(
            checked_plan_provenance_projection(MAX_PLAN_PROVENANCE_BYTES - 1, 1),
            Ok(())
        );
        assert_eq!(
            checked_plan_provenance_projection(MAX_PLAN_PROVENANCE_BYTES, 1),
            Err(ProposalRepoError::SafetyLimitExceeded)
        );
        assert_eq!(
            checked_plan_provenance_projection(usize::MAX, 1),
            Err(ProposalRepoError::SafetyLimitExceeded)
        );
    }

    #[test]
    fn publication_payload_accepts_three_mebibytes_exactly() {
        assert_eq!(ensure_publication_byte_limit(MAX_PUBLICATION_BYTES), Ok(()));
        assert_eq!(
            ensure_publication_byte_limit(MAX_PUBLICATION_BYTES + 1),
            Err(ProposalRepoError::SafetyLimitExceeded)
        );
    }
}
