//! Query-shaped proposal codecs. Domain validation remains shared with core.

use chrono::DateTime;
use itinera_core::{
    domain::{
        content_history::ChangeSource,
        proposal::{ChangeSet, Proposal, ProposalDecision, ProposalRoute, ProposalStatus},
    },
    ports::proposal::ProposalRepoError,
    services::proposals::validate_stored_proposal,
};
use sqlx::FromRow;

use crate::sqlite::codec::{checked_revision, decode_json, encode_json};

pub(super) const MAX_PROPOSALS: usize = 1_000;
pub(super) const PROPOSAL_QUERY_LIMIT: i64 = 1_001;
pub(super) const MAX_PROPOSAL_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub(super) const MAX_CHANGE_SET_BYTES: usize = 256 * 1024;

#[derive(Debug, FromRow)]
pub(super) struct ProposalRow {
    proposal_trip_id: String,
    proposal_id: String,
    proposal_created_by: String,
    source_kind: String,
    source_service_id: Option<String>,
    source_service_name: Option<String>,
    proposal_title: String,
    proposal_rationale: String,
    change_set_json: String,
    proposal_route: String,
    proposal_status: String,
    decision_kind: Option<String>,
    decision_user_id: Option<String>,
    decision_poll_id: Option<String>,
    rejection_reason: Option<String>,
    proposal_created_at: String,
    proposal_revision: i64,
}

#[derive(Debug, Clone)]
pub(in crate::sqlite) struct LoadedProposal {
    pub(in crate::sqlite) value: Proposal,
    pub(in crate::sqlite) revision: i64,
}

pub(super) struct EncodedProposal {
    pub(super) source_kind: &'static str,
    pub(super) source_service_id: Option<String>,
    pub(super) source_service_name: Option<String>,
    pub(super) change_set_json: String,
    pub(super) route: &'static str,
    pub(super) status: &'static str,
    pub(super) decision_kind: Option<&'static str>,
    pub(super) decision_user_id: Option<String>,
    pub(super) decision_poll_id: Option<String>,
}

impl ProposalRow {
    pub(super) fn into_proposal(
        self,
        expected_trip_id: &str,
    ) -> Result<LoadedProposal, ProposalRepoError> {
        let revision = checked_revision(self.proposal_revision).map_err(corrupt)?;
        let source = match (
            self.source_kind.as_str(),
            self.source_service_id,
            self.source_service_name,
        ) {
            ("web", None, None) => ChangeSource::Web {},
            ("service", Some(service_identity_id), Some(service_identity_name)) => {
                ChangeSource::Service {
                    service_identity_id,
                    service_identity_name,
                }
            }
            _ => return Err(ProposalRepoError::CorruptData),
        };
        let decision = match (
            self.decision_kind.as_deref(),
            self.decision_user_id,
            self.decision_poll_id,
        ) {
            (None, None, None) => None,
            (Some("leader"), Some(user_id), None) => Some(ProposalDecision::Leader { user_id }),
            (Some("poll"), None, Some(poll_id)) => Some(ProposalDecision::Poll { poll_id }),
            _ => return Err(ProposalRepoError::CorruptData),
        };
        let change_set = decode_canonical_change_set(self.change_set_json)?;
        let proposal = Proposal {
            id: self.proposal_id,
            trip_id: self.proposal_trip_id,
            created_by: self.proposal_created_by,
            source,
            title: self.proposal_title,
            rationale: self.proposal_rationale,
            change_set,
            route: decode_route(&self.proposal_route)?,
            status: decode_status(&self.proposal_status)?,
            decided_by: decision,
            rejection_reason: self.rejection_reason,
            created_at: self.proposal_created_at,
        };
        validate_stored_proposal(expected_trip_id, &proposal).map_err(corrupt)?;
        Ok(LoadedProposal {
            value: proposal,
            revision,
        })
    }
}

pub(super) fn encode_proposal(
    expected_trip_id: &str,
    proposal: &Proposal,
) -> Result<EncodedProposal, ProposalRepoError> {
    validate_stored_proposal(expected_trip_id, proposal).map_err(corrupt)?;
    let (source_kind, source_service_id, source_service_name) = match &proposal.source {
        ChangeSource::Web {} => ("web", None, None),
        ChangeSource::Service {
            service_identity_id,
            service_identity_name,
        } => (
            "service",
            Some(service_identity_id.clone()),
            Some(service_identity_name.clone()),
        ),
    };
    let (decision_kind, decision_user_id, decision_poll_id) = match &proposal.decided_by {
        None => (None, None, None),
        Some(ProposalDecision::Leader { user_id }) => (Some("leader"), Some(user_id.clone()), None),
        Some(ProposalDecision::Poll { poll_id }) => (Some("poll"), None, Some(poll_id.clone())),
    };
    let change_set_json = encode_json(&proposal.change_set).map_err(corrupt)?;
    if change_set_json.len() > MAX_CHANGE_SET_BYTES {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    Ok(EncodedProposal {
        source_kind,
        source_service_id,
        source_service_name,
        change_set_json,
        route: encode_route(proposal.route),
        status: encode_status(proposal.status)?,
        decision_kind,
        decision_user_id,
        decision_poll_id,
    })
}

pub(super) fn sort_newest(proposals: &mut [LoadedProposal]) -> Result<(), ProposalRepoError> {
    let mut timestamps = proposals
        .iter()
        .map(|proposal| DateTime::parse_from_rfc3339(&proposal.value.created_at).map_err(corrupt))
        .collect::<Result<Vec<_>, _>>()?;
    let mut indexed = proposals
        .iter()
        .cloned()
        .zip(timestamps.drain(..))
        .collect::<Vec<_>>();
    indexed.sort_by(|(left, left_time), (right, right_time)| {
        right_time
            .cmp(left_time)
            .then_with(|| right.value.id.cmp(&left.value.id))
    });
    for (slot, (proposal, _)) in proposals.iter_mut().zip(indexed) {
        *slot = proposal;
    }
    Ok(())
}

pub(super) fn encoded_response_size(proposals: &[Proposal]) -> Result<usize, ProposalRepoError> {
    serde_json::to_vec(proposals)
        .map(|bytes| bytes.len())
        .map_err(corrupt)
}

fn decode_canonical_change_set(value: String) -> Result<ChangeSet, ProposalRepoError> {
    if value.len() > MAX_CHANGE_SET_BYTES {
        return Err(ProposalRepoError::SafetyLimitExceeded);
    }
    let change_set = decode_json::<ChangeSet>(&value).map_err(corrupt)?;
    if encode_json(&change_set).map_err(corrupt)? != value {
        return Err(ProposalRepoError::CorruptData);
    }
    Ok(change_set)
}

fn encode_route(route: ProposalRoute) -> &'static str {
    match route {
        ProposalRoute::LeaderApproval => "leader_approval",
        ProposalRoute::Poll => "poll",
    }
}

fn decode_route(value: &str) -> Result<ProposalRoute, ProposalRepoError> {
    match value {
        "leader_approval" => Ok(ProposalRoute::LeaderApproval),
        "poll" => Ok(ProposalRoute::Poll),
        _ => Err(ProposalRepoError::CorruptData),
    }
}

fn encode_status(status: ProposalStatus) -> Result<&'static str, ProposalRepoError> {
    match status {
        ProposalStatus::Pending => Ok("pending"),
        ProposalStatus::Rejected => Ok("rejected"),
        ProposalStatus::Applied => Ok("applied"),
        ProposalStatus::Stale => Ok("stale"),
        ProposalStatus::Draft | ProposalStatus::Approved => Err(ProposalRepoError::CorruptData),
    }
}

fn decode_status(value: &str) -> Result<ProposalStatus, ProposalRepoError> {
    match value {
        "pending" => Ok(ProposalStatus::Pending),
        "rejected" => Ok(ProposalStatus::Rejected),
        "applied" => Ok(ProposalStatus::Applied),
        "stale" => Ok(ProposalStatus::Stale),
        _ => Err(ProposalRepoError::CorruptData),
    }
}

fn corrupt<T>(_error: T) -> ProposalRepoError {
    ProposalRepoError::CorruptData
}
