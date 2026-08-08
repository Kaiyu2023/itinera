//! Query-shaped codecs for polls, options, and normalized ballots.

use itinera_core::{
    domain::poll::{Poll, PollKind, PollOption, PollStatus, PollVote},
    ports::poll::PollRepoError,
    services::polls::validate_stored_poll,
};
use sqlx::FromRow;

use crate::sqlite::codec::{checked_revision, validate_id};

pub(super) const MAX_POLLS: usize = 1_000;
pub(super) const POLL_QUERY_LIMIT: i64 = 1_001;
pub(super) const MAX_POLL_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, FromRow)]
pub(super) struct PollRow {
    poll_trip_id: String,
    poll_id: String,
    poll_created_by: String,
    poll_kind: String,
    poll_replaces_poll_id: Option<String>,
    poll_title: String,
    poll_description: String,
    poll_created_at: String,
    poll_opens_at: Option<String>,
    poll_closes_at: String,
    poll_decided_at: Option<String>,
    poll_quorum: i64,
    poll_allow_multi: i64,
    poll_status: String,
    poll_resolution_note: Option<String>,
    poll_revision: i64,
}

#[derive(Debug, FromRow)]
pub(super) struct PollOptionRow {
    option_trip_id: String,
    option_poll_id: String,
    option_id: String,
    option_position: i64,
    option_label: String,
    option_proposal_id: Option<String>,
}

#[derive(Debug, FromRow)]
pub(super) struct BallotRow {
    ballot_trip_id: String,
    ballot_poll_id: String,
    ballot_user_id: String,
    ballot_voted_at: String,
    ballot_revision: i64,
}

#[derive(Debug, FromRow)]
pub(super) struct BallotOptionRow {
    selection_trip_id: String,
    selection_poll_id: String,
    selection_user_id: String,
    selection_option_id: String,
}

#[derive(Debug, Clone)]
pub(in crate::sqlite) struct LoadedPoll {
    pub(in crate::sqlite) poll: Poll,
    pub(in crate::sqlite) created_at: String,
    pub(in crate::sqlite) replaces_poll_id: Option<String>,
    pub(in crate::sqlite) revision: i64,
}

#[derive(Debug, Clone)]
pub(in crate::sqlite) struct LoadedBallot {
    pub(in crate::sqlite) user_id: String,
    pub(in crate::sqlite) option_ids: Vec<String>,
    pub(in crate::sqlite) voted_at: String,
    pub(in crate::sqlite) revision: i64,
}

impl PollRow {
    pub(super) fn id(&self) -> &str {
        &self.poll_id
    }

    pub(super) fn into_loaded(
        self,
        expected_trip_id: &str,
        options: Vec<PollOption>,
        votes: Vec<PollVote>,
    ) -> Result<LoadedPoll, PollRepoError> {
        if self.poll_trip_id != expected_trip_id {
            return Err(PollRepoError::CorruptData);
        }
        let revision = checked_revision(self.poll_revision).map_err(corrupt)?;
        let quorum = u32::try_from(self.poll_quorum).map_err(corrupt)?;
        let allow_multi = decode_bool(self.poll_allow_multi)?;
        let kind = decode_kind(&self.poll_kind)?;
        if self
            .poll_replaces_poll_id
            .as_deref()
            .is_some_and(|poll_id| {
                kind != PollKind::PlanChange
                    || poll_id == self.poll_id
                    || validate_id(poll_id).is_err()
            })
        {
            return Err(PollRepoError::CorruptData);
        }
        let poll = Poll {
            id: self.poll_id,
            trip_id: self.poll_trip_id,
            created_by: self.poll_created_by,
            kind,
            title: self.poll_title,
            description: self.poll_description,
            options,
            opens_at: self.poll_opens_at,
            closes_at: self.poll_closes_at,
            decided_at: self.poll_decided_at,
            quorum,
            allow_multi,
            status: decode_status(&self.poll_status)?,
            votes,
            resolution_note: self.poll_resolution_note,
        };
        validate_stored_poll(expected_trip_id, &poll, &self.poll_created_at).map_err(corrupt)?;
        Ok(LoadedPoll {
            poll,
            created_at: self.poll_created_at,
            replaces_poll_id: self.poll_replaces_poll_id,
            revision,
        })
    }
}

impl PollOptionRow {
    pub(super) fn poll_id(&self) -> &str {
        &self.option_poll_id
    }

    pub(super) fn into_option(
        self,
        expected_trip_id: &str,
        expected_poll_id: &str,
        expected_position: usize,
    ) -> Result<PollOption, PollRepoError> {
        if self.option_trip_id != expected_trip_id
            || self.option_poll_id != expected_poll_id
            || usize::try_from(self.option_position).ok() != Some(expected_position)
        {
            return Err(PollRepoError::CorruptData);
        }
        Ok(PollOption {
            id: self.option_id,
            label: self.option_label,
            proposal_id: self.option_proposal_id,
        })
    }
}

impl BallotRow {
    pub(super) fn poll_id(&self) -> &str {
        &self.ballot_poll_id
    }

    pub(super) fn user_id(&self) -> &str {
        &self.ballot_user_id
    }

    pub(super) fn into_ballot(
        self,
        expected_trip_id: &str,
        option_ids: Vec<String>,
    ) -> Result<LoadedBallot, PollRepoError> {
        if self.ballot_trip_id != expected_trip_id || option_ids.is_empty() {
            return Err(PollRepoError::CorruptData);
        }
        validate_id(&self.ballot_poll_id).map_err(corrupt)?;
        validate_id(&self.ballot_user_id).map_err(corrupt)?;
        let revision = checked_revision(self.ballot_revision).map_err(corrupt)?;
        Ok(LoadedBallot {
            user_id: self.ballot_user_id,
            option_ids,
            voted_at: self.ballot_voted_at,
            revision,
        })
    }
}

impl BallotOptionRow {
    pub(super) fn poll_id(&self) -> &str {
        &self.selection_poll_id
    }

    pub(super) fn user_id(&self) -> &str {
        &self.selection_user_id
    }

    pub(super) fn into_selection(
        self,
        expected_trip_id: &str,
        expected_poll_id: &str,
        expected_user_id: &str,
    ) -> Result<String, PollRepoError> {
        if self.selection_trip_id != expected_trip_id
            || self.selection_poll_id != expected_poll_id
            || self.selection_user_id != expected_user_id
        {
            return Err(PollRepoError::CorruptData);
        }
        validate_id(&self.selection_option_id).map_err(corrupt)?;
        Ok(self.selection_option_id)
    }
}

pub(super) fn encode_kind(kind: PollKind) -> &'static str {
    match kind {
        PollKind::Decision => "decision",
        PollKind::PlanChange => "plan_change",
    }
}

pub(super) fn encode_status(status: PollStatus) -> &'static str {
    match status {
        PollStatus::Draft => "draft",
        PollStatus::Scheduled => "scheduled",
        PollStatus::Open => "open",
        PollStatus::Passed => "passed",
        PollStatus::Failed => "failed",
        PollStatus::Expired => "expired",
    }
}

fn decode_kind(value: &str) -> Result<PollKind, PollRepoError> {
    match value {
        "decision" => Ok(PollKind::Decision),
        "plan_change" => Ok(PollKind::PlanChange),
        _ => Err(PollRepoError::CorruptData),
    }
}

fn decode_status(value: &str) -> Result<PollStatus, PollRepoError> {
    match value {
        "draft" => Ok(PollStatus::Draft),
        "scheduled" => Ok(PollStatus::Scheduled),
        "open" => Ok(PollStatus::Open),
        "passed" => Ok(PollStatus::Passed),
        "failed" => Ok(PollStatus::Failed),
        "expired" => Ok(PollStatus::Expired),
        _ => Err(PollRepoError::CorruptData),
    }
}

fn decode_bool(value: i64) -> Result<bool, PollRepoError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(PollRepoError::CorruptData),
    }
}

fn corrupt<T>(_error: T) -> PollRepoError {
    PollRepoError::CorruptData
}
