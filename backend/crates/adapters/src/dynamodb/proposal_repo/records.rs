//! Proposal record key and strict codec.

use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;
use chrono::DateTime;
use itinera_core::{
    domain::proposal::Proposal, ports::proposal::ProposalRepoError,
    services::proposals::validate_stored_proposal,
};

use crate::dynamodb::{
    SK,
    trip_repo::records::{encode_record, string, trip_pk},
};

use super::{access, record_error};

pub(in crate::dynamodb) const PROPOSAL_ENTITY: &str = "PROPOSAL";

pub(in crate::dynamodb) fn proposal_sk(proposal_id: &str) -> String {
    format!("PROPOSAL#{proposal_id}")
}

pub(in crate::dynamodb) fn encode_proposal(
    proposal: &Proposal,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, ProposalRepoError> {
    encode_record(
        trip_pk(&proposal.trip_id),
        proposal_sk(&proposal.id),
        PROPOSAL_ENTITY,
        proposal,
        revision,
    )
    .map_err(record_error)
}

pub(in crate::dynamodb) fn decode_proposal(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<access::Loaded<Proposal>, ProposalRepoError> {
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let proposal: access::Loaded<Proposal> =
        access::decode_loaded(item, &pk, &sk, PROPOSAL_ENTITY)?;
    if proposal.sort_key != proposal_sk(&proposal.value.id)
        || !valid_utc_timestamp(&proposal.value.created_at)
        || validate_stored_proposal(expected_trip_id, &proposal.value).is_err()
    {
        return Err(ProposalRepoError::CorruptData);
    }
    Ok(proposal)
}

fn valid_utc_timestamp(value: &str) -> bool {
    DateTime::parse_from_rfc3339(value)
        .is_ok_and(|timestamp| timestamp.offset().local_minus_utc() == 0)
}
