//! Proposal record key and strict codec.

use chrono::DateTime;

use super::*;

pub(super) const PROPOSAL_ENTITY: &str = "PROPOSAL";

pub(super) fn proposal_sk(proposal_id: &str) -> String {
    format!("PROPOSAL#{proposal_id}")
}

pub(super) fn encode_proposal(
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

pub(super) fn decode_proposal(
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
