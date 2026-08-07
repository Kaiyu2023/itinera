use std::collections::{HashMap, HashSet};

use aws_sdk_dynamodb::types::AttributeValue;
use chrono::DateTime;
use itinera_core::{
    domain::poll::{Poll, PollKind, PollOption, PollStatus, PollVote},
    ports::poll::PollRepoError,
};
use serde::{Deserialize, Serialize};

use crate::dynamodb::{
    SK,
    trip_repo::records::{decode_record, encode_record, string, trip_pk},
};

use super::record_error;

pub(super) const POLL_ENTITY: &str = "POLL";
pub(super) const BALLOT_ENTITY: &str = "POLL_BALLOT";
pub(super) const POLL_PREFIX: &str = "POLL#";
pub(super) const ADOPT_LABEL: &str = "Adopt the proposed plan change";
pub(super) const KEEP_LABEL: &str = "Keep the current plan";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PollRecord {
    id: String,
    trip_id: String,
    created_by: String,
    kind: PollKind,
    title: String,
    description: String,
    options: Vec<PollOption>,
    opens_at: Option<String>,
    closes_at: String,
    decided_at: Option<String>,
    quorum: u32,
    allow_multi: bool,
    status: PollStatus,
    resolution_note: Option<String>,
    created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BallotRecord {
    pub(super) poll_id: String,
    pub(super) user_id: String,
    pub(super) option_ids: Vec<String>,
    pub(super) at: String,
}

#[derive(Clone)]
pub(super) struct LoadedPoll {
    pub(super) poll: Poll,
    pub(super) revision: u64,
    pub(super) created_at: String,
}

#[derive(Clone)]
pub(super) struct LoadedBallot {
    pub(super) ballot: BallotRecord,
    pub(super) revision: u64,
}

pub(super) fn poll_sk(poll_id: &str) -> String {
    format!("{POLL_PREFIX}{poll_id}")
}

pub(super) fn ballot_sk(poll_id: &str, user_id: &str) -> String {
    format!("{}#VOTE#{user_id}", poll_sk(poll_id))
}

pub(super) fn encode_poll(
    poll: &Poll,
    created_at: &str,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, PollRepoError> {
    let record = PollRecord {
        id: poll.id.clone(),
        trip_id: poll.trip_id.clone(),
        created_by: poll.created_by.clone(),
        kind: poll.kind,
        title: poll.title.clone(),
        description: poll.description.clone(),
        options: poll.options.clone(),
        opens_at: poll.opens_at.clone(),
        closes_at: poll.closes_at.clone(),
        decided_at: poll.decided_at.clone(),
        quorum: poll.quorum,
        allow_multi: poll.allow_multi,
        status: poll.status,
        resolution_note: poll.resolution_note.clone(),
        created_at: created_at.to_string(),
    };
    validate_poll_record(&record, &poll.trip_id)?;
    encode_record(
        trip_pk(&poll.trip_id),
        poll_sk(&poll.id),
        POLL_ENTITY,
        &record,
        revision,
    )
    .map_err(record_error)
}

pub(super) fn decode_poll(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<LoadedPoll, PollRepoError> {
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored = decode_record::<PollRecord>(item, &pk, &sk, POLL_ENTITY).map_err(record_error)?;
    if stored.revision == 0
        || sk != poll_sk(&stored.value.id)
        || !validate_poll_record(&stored.value, expected_trip_id).is_ok()
    {
        return Err(PollRepoError::CorruptData);
    }
    let record = stored.value;
    Ok(LoadedPoll {
        poll: Poll {
            id: record.id,
            trip_id: record.trip_id,
            created_by: record.created_by,
            kind: record.kind,
            title: record.title,
            description: record.description,
            options: record.options,
            opens_at: record.opens_at,
            closes_at: record.closes_at,
            decided_at: record.decided_at,
            quorum: record.quorum,
            allow_multi: record.allow_multi,
            status: record.status,
            votes: vec![],
            resolution_note: record.resolution_note,
        },
        revision: stored.revision,
        created_at: record.created_at,
    })
}

pub(super) fn encode_ballot(
    trip_id: &str,
    ballot: &BallotRecord,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, PollRepoError> {
    validate_ballot_shape(ballot)?;
    encode_record(
        trip_pk(trip_id),
        ballot_sk(&ballot.poll_id, &ballot.user_id),
        BALLOT_ENTITY,
        ballot,
        revision,
    )
    .map_err(record_error)
}

pub(super) fn decode_ballot(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<LoadedBallot, PollRepoError> {
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored =
        decode_record::<BallotRecord>(item, &pk, &sk, BALLOT_ENTITY).map_err(record_error)?;
    if stored.revision == 0
        || sk != ballot_sk(&stored.value.poll_id, &stored.value.user_id)
        || validate_ballot_shape(&stored.value).is_err()
    {
        return Err(PollRepoError::CorruptData);
    }
    Ok(LoadedBallot {
        ballot: stored.value,
        revision: stored.revision,
    })
}

pub(super) fn attach_ballots(
    loaded: &mut LoadedPoll,
    ballots: impl IntoIterator<Item = LoadedBallot>,
) -> Result<(), PollRepoError> {
    let created = utc_timestamp(&loaded.created_at)?;
    let closes = utc_timestamp(&loaded.poll.closes_at)?;
    let decided = loaded
        .poll
        .decided_at
        .as_deref()
        .map(utc_timestamp)
        .transpose()?;
    let option_ids = loaded
        .poll
        .options
        .iter()
        .map(|option| option.id.as_str())
        .collect::<HashSet<_>>();
    let mut voters = HashSet::new();
    let mut votes = Vec::new();
    for loaded_ballot in ballots {
        let ballot = loaded_ballot.ballot;
        let voted_at = utc_timestamp(&ballot.at)?;
        if ballot.poll_id != loaded.poll.id
            || !voters.insert(ballot.user_id.clone())
            || ballot.option_ids.is_empty()
            || (!loaded.poll.allow_multi && ballot.option_ids.len() != 1)
            || voted_at < created
            || voted_at >= closes
            || decided.as_ref().is_some_and(|at| voted_at > *at)
            || ballot
                .option_ids
                .iter()
                .any(|option_id| !option_ids.contains(option_id.as_str()))
        {
            return Err(PollRepoError::CorruptData);
        }
        votes.extend(ballot.option_ids.into_iter().map(|option_id| PollVote {
            user_id: ballot.user_id.clone(),
            option_id,
            at: ballot.at.clone(),
        }));
    }
    votes.sort_by(|left, right| {
        left.user_id
            .cmp(&right.user_id)
            .then_with(|| left.option_id.cmp(&right.option_id))
    });
    loaded.poll.votes = votes;
    validate_poll_result(&loaded.poll)
}

fn validate_poll_result(poll: &Poll) -> Result<(), PollRepoError> {
    let voters = poll
        .votes
        .iter()
        .map(|vote| vote.user_id.as_str())
        .collect::<HashSet<_>>()
        .len();
    let quorum = usize::try_from(poll.quorum).map_err(|_| PollRepoError::CorruptData)?;
    if matches!(poll.status, PollStatus::Draft | PollStatus::Scheduled) && voters != 0 {
        return Err(PollRepoError::CorruptData);
    }
    if poll.status == PollStatus::Expired {
        return (voters < quorum)
            .then_some(())
            .ok_or(PollRepoError::CorruptData);
    }
    if matches!(poll.status, PollStatus::Passed | PollStatus::Failed) && voters < quorum {
        return Err(PollRepoError::CorruptData);
    }
    if !matches!(poll.status, PollStatus::Passed | PollStatus::Failed) {
        return Ok(());
    }

    let has_decisive_winner = decisive_winner(poll)?.is_some();
    if poll.status == PollStatus::Passed && !has_decisive_winner
        || poll.kind == PollKind::Decision
            && poll.status == PollStatus::Failed
            && has_decisive_winner
    {
        return Err(PollRepoError::CorruptData);
    }
    Ok(())
}

pub(super) fn decisive_winner(poll: &Poll) -> Result<Option<&PollOption>, PollRepoError> {
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
        let count = counts
            .get_mut(vote.option_id.as_str())
            .ok_or(PollRepoError::CorruptData)?;
        *count = count.checked_add(1).ok_or(PollRepoError::CorruptData)?;
    }
    let top = counts.values().copied().max().unwrap_or(0);
    if top == 0 || top.saturating_mul(2) <= voters {
        return Ok(None);
    }
    let mut winners = poll
        .options
        .iter()
        .filter(|option| counts.get(option.id.as_str()) == Some(&top));
    let winner = winners.next();
    if winners.next().is_some() {
        return Ok(None);
    }
    Ok(winner)
}

pub(super) fn validate_ballot_for_poll(
    poll: &Poll,
    option_ids: &[String],
) -> Result<(), PollRepoError> {
    if option_ids.len() > poll.options.len()
        || (!poll.allow_multi && option_ids.len() != 1)
        || (poll.allow_multi && option_ids.len() > poll.options.len())
    {
        return Err(PollRepoError::InvalidVote);
    }
    let available = poll
        .options
        .iter()
        .map(|option| option.id.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    if option_ids
        .iter()
        .any(|id| !available.contains(id.as_str()) || !seen.insert(id))
    {
        return Err(PollRepoError::InvalidVote);
    }
    Ok(())
}

fn validate_poll_record(record: &PollRecord, expected_trip_id: &str) -> Result<(), PollRepoError> {
    if !valid_id(&record.id)
        || record.trip_id != expected_trip_id
        || !valid_id(&record.created_by)
        || !exact_required_text(&record.title, 200)
        || !exact_optional_text(&record.description, 4_000)
        || !(2..=6).contains(&record.options.len())
        || !(1..=1_000).contains(&record.quorum)
    {
        return Err(PollRepoError::CorruptData);
    }
    let mut option_ids = HashSet::new();
    let mut labels = HashSet::new();
    if record.options.iter().any(|option| {
        !valid_id(&option.id)
            || !exact_required_text(&option.label, 200)
            || !option_ids.insert(option.id.as_str())
            || !labels.insert(option.label.as_str())
            || option
                .proposal_id
                .as_deref()
                .is_some_and(|id| !valid_id(id))
    }) {
        return Err(PollRepoError::CorruptData);
    }
    match record.kind {
        PollKind::Decision => {
            if record
                .options
                .iter()
                .any(|option| option.proposal_id.is_some())
            {
                return Err(PollRepoError::CorruptData);
            }
        }
        PollKind::PlanChange => {
            if record.allow_multi
                || record.options.len() != 2
                || record
                    .options
                    .iter()
                    .filter(|option| option.proposal_id.is_some())
                    .count()
                    != 1
                || !record
                    .options
                    .iter()
                    .any(|option| option.proposal_id.is_some() && option.label == ADOPT_LABEL)
                || !record
                    .options
                    .iter()
                    .any(|option| option.proposal_id.is_none() && option.label == KEEP_LABEL)
            {
                return Err(PollRepoError::CorruptData);
            }
        }
    }
    let created = utc_timestamp(&record.created_at)?;
    let closes = utc_timestamp(&record.closes_at)?;
    if closes <= created {
        return Err(PollRepoError::CorruptData);
    }
    let opens = record.opens_at.as_deref().map(utc_timestamp).transpose()?;
    let decided = record
        .decided_at
        .as_deref()
        .map(utc_timestamp)
        .transpose()?;
    if opens.is_some_and(|opens| opens <= created || opens >= closes)
        || decided.is_some_and(|decided| decided < created)
    {
        return Err(PollRepoError::CorruptData);
    }
    let lifecycle_valid = match record.status {
        PollStatus::Draft => {
            record.opens_at.is_none()
                && record.decided_at.is_none()
                && record.resolution_note.is_none()
        }
        PollStatus::Scheduled => {
            record.opens_at.is_some()
                && record.decided_at.is_none()
                && record.resolution_note.is_none()
        }
        PollStatus::Open => record.decided_at.is_none() && record.resolution_note.is_none(),
        PollStatus::Passed => record.decided_at.is_some() && record.resolution_note.is_none(),
        PollStatus::Failed | PollStatus::Expired => {
            record.decided_at.is_some()
                && record
                    .resolution_note
                    .as_deref()
                    .is_some_and(|note| exact_required_text(note, 2_000))
        }
    };
    lifecycle_valid
        .then_some(())
        .ok_or(PollRepoError::CorruptData)
}

fn validate_ballot_shape(ballot: &BallotRecord) -> Result<(), PollRepoError> {
    let mut options = HashSet::new();
    if !valid_id(&ballot.poll_id)
        || !valid_id(&ballot.user_id)
        || ballot.option_ids.is_empty()
        || ballot.option_ids.len() > 6
        || ballot
            .option_ids
            .iter()
            .any(|id| !valid_id(id) || !options.insert(id))
        || utc_timestamp(&ballot.at).is_err()
    {
        return Err(PollRepoError::CorruptData);
    }
    Ok(())
}

fn utc_timestamp(value: &str) -> Result<DateTime<chrono::FixedOffset>, PollRepoError> {
    let timestamp = DateTime::parse_from_rfc3339(value).map_err(|_| PollRepoError::CorruptData)?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err(PollRepoError::CorruptData);
    }
    Ok(timestamp)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && value.chars().count() <= 200
}

fn exact_required_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.trim() == value && value.chars().count() <= max
}

fn exact_optional_text(value: &str, max: usize) -> bool {
    value.trim() == value && value.chars().count() <= max
}
