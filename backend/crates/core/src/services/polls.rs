use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Days, SecondsFormat};

use crate::{
    domain::poll::{Poll, PollKind, PollOption, PollStatus},
    ports::{
        authorization::TripAuthorizationContext,
        clock::Clock,
        id_gen::IdGen,
        poll::{NewDecisionPoll, NewPlanChangePoll, PollRepo, PollRepoError},
    },
};

use super::{
    proposals::reserve_application_ids,
    validation::{ValidationError, required_text},
};

pub const MAX_POLL_OPTIONS: usize = 6;
pub const MAX_POLL_VOTES: usize = 6_000;
pub const ADOPT_LABEL: &str = "Adopt the proposed plan change";
pub const KEEP_LABEL: &str = "Keep the current plan";
pub const BELOW_QUORUM_NOTE: &str = "Closed below quorum - no decision recorded.";
pub const TIE_NOTE: &str = "Closed with a tied result - no decision recorded.";
pub const NO_MAJORITY_NOTE: &str = "No option reached a majority - no decision recorded.";
pub const KEEP_NOTE: &str = "The group chose to keep the current plan.";
pub const STALE_NOTE: &str =
    "The winning proposal was based on an outdated plan - no plan change was applied.";
const PLAN_CHANGE_POLL_DAYS: u64 = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePollInput {
    pub title: String,
    pub description: String,
    pub options: Vec<String>,
    pub closes_at: String,
    pub allow_multi: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PollServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] PollRepoError),
}

pub async fn list_polls(
    repo: &dyn PollRepo,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<Poll>, PollServiceError> {
    validate_id(trip_id, "tripId is invalid")?;
    Ok(repo.list_polls(trip_id, authorization).await?)
}

pub async fn create_poll(
    repo: &dyn PollRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    input: CreatePollInput,
) -> Result<Poll, PollServiceError> {
    require_human(authorization)?;
    validate_id(trip_id, "tripId is invalid")?;
    let (created, created_at) = canonical_input_utc(&clock.now(), "server time is invalid")?;
    let (closes, closes_at) = canonical_input_utc(
        &input.closes_at,
        "closesAt must be an RFC 3339 UTC timestamp",
    )?;
    if closes <= created {
        return Err(ValidationError("closesAt must be in the future").into());
    }
    if !(2..=MAX_POLL_OPTIONS).contains(&input.options.len()) {
        return Err(ValidationError("a poll must have between 2 and 6 options").into());
    }
    let title = required_text(
        input.title,
        "title is required and must be at most 200 characters",
        200,
    )?;
    let description = normalise_optional_text(input.description, 4_000)?;
    let mut labels = HashSet::new();
    let mut options = Vec::with_capacity(input.options.len());
    for label in input.options {
        let label = required_text(
            label,
            "option labels are required and must be at most 200 characters",
            200,
        )?;
        if !labels.insert(label.clone()) {
            return Err(ValidationError("poll option labels must be unique").into());
        }
        options.push(PollOption {
            id: ids.new_id(),
            label,
            proposal_id: None,
        });
    }
    Ok(repo
        .create_decision_poll(
            trip_id,
            authorization,
            NewDecisionPoll {
                id: ids.new_id(),
                title,
                description,
                options,
                closes_at,
                allow_multi: input.allow_multi,
                created_at,
            },
        )
        .await?)
}

pub async fn proposal_to_poll(
    repo: &dyn PollRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    proposal_id: &str,
) -> Result<Poll, PollServiceError> {
    require_human(authorization)?;
    validate_id(trip_id, "tripId is invalid")?;
    validate_id(proposal_id, "proposalId is invalid")?;
    let poll = new_plan_change_poll(ids, &clock.now())?;
    Ok(repo
        .route_proposal_to_poll(
            trip_id,
            authorization,
            proposal_id,
            poll,
            reserve_application_ids(ids),
        )
        .await?)
}

pub async fn open_poll(
    repo: &dyn PollRepo,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    poll_id: &str,
) -> Result<Poll, PollServiceError> {
    require_human(authorization)?;
    validate_route_ids(trip_id, poll_id)?;
    Ok(repo
        .open_poll(trip_id, authorization, poll_id, clock)
        .await?)
}

pub async fn vote(
    repo: &dyn PollRepo,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    poll_id: &str,
    option_ids: Vec<String>,
) -> Result<Poll, PollServiceError> {
    require_human(authorization)?;
    validate_route_ids(trip_id, poll_id)?;
    if option_ids.len() > MAX_POLL_OPTIONS {
        return Err(ValidationError("optionIds contains too many choices").into());
    }
    let mut seen = HashSet::new();
    for option_id in &option_ids {
        validate_id(option_id, "optionIds contains an invalid id")?;
        if !seen.insert(option_id) {
            return Err(ValidationError("optionIds must be unique").into());
        }
    }
    Ok(repo
        .cast_vote(trip_id, authorization, poll_id, &option_ids, clock)
        .await?)
}

pub async fn close_poll(
    repo: &dyn PollRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    poll_id: &str,
) -> Result<Poll, PollServiceError> {
    require_human(authorization)?;
    validate_route_ids(trip_id, poll_id)?;
    Ok(repo
        .close_poll(
            trip_id,
            authorization,
            poll_id,
            &validated_now(clock)?,
            reserve_application_ids(ids),
        )
        .await?)
}

fn require_human(authorization: &TripAuthorizationContext) -> Result<(), PollServiceError> {
    authorization
        .human_user_id()
        .map(|_| ())
        .ok_or_else(|| PollRepoError::Forbidden.into())
}

pub fn new_plan_change_poll(
    ids: &dyn IdGen,
    created_at: &str,
) -> Result<NewPlanChangePoll, ValidationError> {
    let (created, created_at) = canonical_input_utc(created_at, "server time is invalid")?;
    let closes = created
        .checked_add_days(Days::new(PLAN_CHANGE_POLL_DAYS))
        .ok_or(ValidationError(
            "server time is outside the supported range",
        ))?;
    Ok(NewPlanChangePoll {
        poll_id: ids.new_id(),
        adopt_option_id: ids.new_id(),
        keep_option_id: ids.new_id(),
        created_at,
        closes_at: closes.to_rfc3339_opts(SecondsFormat::Millis, true),
    })
}

/// Validates the canonical domain graph decoded from one stored poll and its
/// normalized ballot rows. Relational proposal and membership checks remain in
/// the repository, but every scalar, lifecycle, tally, and ballot invariant is
/// shared with application-domain types here.
pub fn validate_stored_poll(
    expected_trip_id: &str,
    poll: &Poll,
    created_at: &str,
) -> Result<(), ValidationError> {
    if poll.trip_id != expected_trip_id
        || !valid_id(&poll.id)
        || !valid_id(&poll.created_by)
        || !exact_required_text(&poll.title, 200)
        || !exact_optional_text(&poll.description, 4_000)
        || !(2..=MAX_POLL_OPTIONS).contains(&poll.options.len())
        || !(1..=1_000).contains(&poll.quorum)
        || poll.votes.len() > MAX_POLL_VOTES
    {
        return Err(ValidationError("stored poll is invalid"));
    }

    let mut option_ids = HashSet::new();
    let mut labels = HashSet::new();
    if poll.options.iter().any(|option| {
        !valid_id(&option.id)
            || !exact_required_text(&option.label, 200)
            || !option_ids.insert(option.id.as_str())
            || !labels.insert(option.label.as_str())
            || option
                .proposal_id
                .as_deref()
                .is_some_and(|id| !valid_id(id))
    }) {
        return Err(ValidationError("stored poll options are invalid"));
    }
    match poll.kind {
        PollKind::Decision
            if poll
                .options
                .iter()
                .all(|option| option.proposal_id.is_none()) => {}
        PollKind::PlanChange
            if !poll.allow_multi
                && poll.options.len() == 2
                && poll
                    .options
                    .iter()
                    .filter(|option| option.proposal_id.is_some())
                    .count()
                    == 1
                && poll
                    .options
                    .iter()
                    .any(|option| option.proposal_id.is_some() && option.label == ADOPT_LABEL)
                && poll
                    .options
                    .iter()
                    .any(|option| option.proposal_id.is_none() && option.label == KEEP_LABEL) => {}
        _ => return Err(ValidationError("stored poll option provenance is invalid")),
    }

    let created = canonical_utc(created_at)?;
    let closes = canonical_utc(&poll.closes_at)?;
    if closes <= created {
        return Err(ValidationError("stored poll deadline is invalid"));
    }
    let opens = poll.opens_at.as_deref().map(canonical_utc).transpose()?;
    let decided = poll.decided_at.as_deref().map(canonical_utc).transpose()?;
    if opens.is_some_and(|opens| opens <= created || opens >= closes)
        || decided.is_some_and(|decided| decided < created)
    {
        return Err(ValidationError("stored poll lifecycle time is invalid"));
    }

    let lifecycle_valid = match poll.status {
        PollStatus::Draft => {
            poll.opens_at.is_none() && poll.decided_at.is_none() && poll.resolution_note.is_none()
        }
        PollStatus::Scheduled => {
            poll.opens_at.is_some() && poll.decided_at.is_none() && poll.resolution_note.is_none()
        }
        PollStatus::Open => poll.decided_at.is_none() && poll.resolution_note.is_none(),
        PollStatus::Passed => poll.decided_at.is_some() && poll.resolution_note.is_none(),
        PollStatus::Failed | PollStatus::Expired => {
            poll.decided_at.is_some()
                && poll
                    .resolution_note
                    .as_deref()
                    .is_some_and(|note| exact_required_text(note, 2_000))
        }
    };
    if !lifecycle_valid {
        return Err(ValidationError("stored poll lifecycle is invalid"));
    }

    let mut ballot_times = HashMap::new();
    let mut ballot_options = HashSet::new();
    let mut ballot_counts = HashMap::<&str, usize>::new();
    for vote in &poll.votes {
        let voted = canonical_utc(&vote.at)?;
        if !valid_id(&vote.user_id)
            || !option_ids.contains(vote.option_id.as_str())
            || voted < created
            || voted >= closes
            || decided.as_ref().is_some_and(|decided| voted > *decided)
            || !ballot_options.insert((vote.user_id.as_str(), vote.option_id.as_str()))
        {
            return Err(ValidationError("stored poll ballot is invalid"));
        }
        match ballot_times.insert(vote.user_id.as_str(), vote.at.as_str()) {
            Some(previous) if previous != vote.at => {
                return Err(ValidationError("stored poll ballot time is inconsistent"));
            }
            _ => {}
        }
        *ballot_counts.entry(vote.user_id.as_str()).or_default() += 1;
    }
    if ballot_counts.values().any(|count| {
        *count == 0 || *count > poll.options.len() || (!poll.allow_multi && *count != 1)
    }) || matches!(poll.status, PollStatus::Draft | PollStatus::Scheduled)
        && !poll.votes.is_empty()
    {
        return Err(ValidationError("stored poll ballot shape is invalid"));
    }

    let voters = ballot_counts.len();
    let quorum = usize::try_from(poll.quorum)
        .map_err(|_| ValidationError("stored poll quorum is invalid"))?;
    match poll.status {
        PollStatus::Expired
            if voters < quorum && poll.resolution_note.as_deref() == Some(BELOW_QUORUM_NOTE) => {}
        PollStatus::Expired => return Err(ValidationError("stored expired poll is invalid")),
        PollStatus::Passed | PollStatus::Failed if voters < quorum => {
            return Err(ValidationError("stored decided poll is below quorum"));
        }
        PollStatus::Passed if decisive_winner(poll)?.is_none() => {
            return Err(ValidationError("stored passing poll has no majority"));
        }
        PollStatus::Failed => {
            let winner = decisive_winner(poll)?;
            match (poll.kind, winner, poll.resolution_note.as_deref()) {
                (PollKind::Decision, None, Some(note)) if non_decision_note(poll) == Some(note) => {
                }
                (PollKind::PlanChange, None, Some(note))
                    if non_decision_note(poll) == Some(note) => {}
                (PollKind::PlanChange, Some(option), Some(STALE_NOTE))
                    if option.proposal_id.is_some() => {}
                (PollKind::PlanChange, Some(option), Some(KEEP_NOTE))
                    if option.proposal_id.is_none() => {}
                _ => return Err(ValidationError("stored failed poll result is invalid")),
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn decisive_winner(poll: &Poll) -> Result<Option<&PollOption>, ValidationError> {
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
            .ok_or(ValidationError(
                "stored poll vote references an unknown option",
            ))?;
        *count = count
            .checked_add(1)
            .ok_or(ValidationError("stored poll tally overflowed"))?;
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
        Ok(None)
    } else {
        Ok(winner)
    }
}

fn non_decision_note(poll: &Poll) -> Option<&'static str> {
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
        *counts.get_mut(vote.option_id.as_str())? += 1;
    }
    let top = counts.values().copied().max().unwrap_or(0);
    let winners = counts.values().filter(|count| **count == top).count();
    if top == 0 || winners != 1 {
        Some(TIE_NOTE)
    } else if top.saturating_mul(2) <= voters {
        Some(NO_MAJORITY_NOTE)
    } else {
        None
    }
}

fn canonical_utc(value: &str) -> Result<DateTime<chrono::FixedOffset>, ValidationError> {
    if value.len() > 64 || !value.ends_with('Z') {
        return Err(ValidationError("stored poll timestamp is invalid"));
    }
    let timestamp = DateTime::parse_from_rfc3339(value)
        .map_err(|_| ValidationError("stored poll timestamp is invalid"))?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err(ValidationError("stored poll timestamp is invalid"));
    }
    Ok(timestamp)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && value.chars().count() <= 200
}

fn exact_required_text(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.trim() == value && value.chars().count() <= maximum
}

fn exact_optional_text(value: &str, maximum: usize) -> bool {
    value.trim() == value && value.chars().count() <= maximum
}

fn validate_route_ids(trip_id: &str, poll_id: &str) -> Result<(), ValidationError> {
    validate_id(trip_id, "tripId is invalid")?;
    validate_id(poll_id, "pollId is invalid")
}

fn validated_now(clock: &dyn Clock) -> Result<String, ValidationError> {
    canonical_input_utc(&clock.now(), "server time is invalid").map(|(_, canonical)| canonical)
}

fn canonical_input_utc(
    value: &str,
    error: &'static str,
) -> Result<(DateTime<chrono::FixedOffset>, String), ValidationError> {
    let timestamp = parse_utc(value, error)?;
    let canonical = value.strip_suffix("+00:00").map_or_else(
        || {
            if value.ends_with('Z') {
                value.to_string()
            } else {
                timestamp.to_rfc3339_opts(SecondsFormat::AutoSi, true)
            }
        },
        |prefix| format!("{prefix}Z"),
    );
    Ok((timestamp, canonical))
}

fn parse_utc(
    value: &str,
    error: &'static str,
) -> Result<DateTime<chrono::FixedOffset>, ValidationError> {
    let timestamp = DateTime::parse_from_rfc3339(value).map_err(|_| ValidationError(error))?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err(ValidationError(error));
    }
    Ok(timestamp)
}

fn normalise_optional_text(value: String, max_len: usize) -> Result<String, ValidationError> {
    let value = value.trim().to_string();
    if value.chars().count() > max_len {
        return Err(ValidationError("text exceeds the allowed length"));
    }
    Ok(value)
}

fn validate_id(value: &str, error: &'static str) -> Result<(), ValidationError> {
    if value.is_empty() || value.trim() != value || value.chars().count() > 200 {
        Err(ValidationError(error))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use crate::{
        domain::{
            poll::{Poll, PollKind, PollOption, PollStatus, PollVote},
            proposal::Proposal,
        },
        ports::{
            authorization::TripAuthorizationContext,
            clock::Clock,
            id_gen::IdGen,
            poll::{NewDecisionPoll, NewPlanChangePoll, PollRepo, PollRepoError},
            proposal::ProposalApplicationIds,
        },
    };

    use super::{
        ADOPT_LABEL, CreatePollInput, KEEP_LABEL, KEEP_NOTE, PollServiceError, STALE_NOTE,
        create_poll, new_plan_change_poll, validate_stored_poll,
    };

    struct Ids(Mutex<u32>);

    impl IdGen for Ids {
        fn new_id(&self) -> String {
            let mut value = self.0.lock().expect("id lock");
            *value += 1;
            format!("id-{value}")
        }
    }

    struct FixedClock;

    impl Clock for FixedClock {
        fn now(&self) -> String {
            "2026-08-06T12:00:00Z".into()
        }
    }

    struct CapturingRepo(Mutex<Option<NewDecisionPoll>>);

    #[async_trait]
    impl PollRepo for CapturingRepo {
        async fn list_polls(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
        ) -> Result<Vec<Poll>, PollRepoError> {
            unreachable!()
        }
        async fn create_decision_poll(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
            poll: NewDecisionPoll,
        ) -> Result<Poll, PollRepoError> {
            *self.0.lock().expect("capture lock") = Some(poll);
            Err(PollRepoError::Conflict)
        }
        async fn create_proposal_poll(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
            _: Proposal,
            _: NewPlanChangePoll,
            _: ProposalApplicationIds,
        ) -> Result<Proposal, PollRepoError> {
            unreachable!()
        }
        async fn route_proposal_to_poll(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
            _: &str,
            _: NewPlanChangePoll,
            _: ProposalApplicationIds,
        ) -> Result<Poll, PollRepoError> {
            unreachable!()
        }
        async fn open_poll(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
            _: &str,
            _: &dyn Clock,
        ) -> Result<Poll, PollRepoError> {
            unreachable!()
        }
        async fn cast_vote(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
            _: &str,
            _: &[String],
            _: &dyn Clock,
        ) -> Result<Poll, PollRepoError> {
            unreachable!()
        }
        async fn close_poll(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
            _: &str,
            _: &str,
            _: ProposalApplicationIds,
        ) -> Result<Poll, PollRepoError> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn creation_normalises_text_and_rejects_ambiguous_options() {
        let repo = CapturingRepo(Mutex::new(None));
        let ids = Ids(Mutex::new(0));
        let authorization =
            TripAuthorizationContext::human(crate::domain::user::UserId("user-a".into()));
        let result = create_poll(
            &repo,
            &ids,
            &FixedClock,
            "trip-a",
            &authorization,
            CreatePollInput {
                title: " Dinner? ".into(),
                description: " context ".into(),
                options: vec![" Ramen ".into(), " Sushi ".into()],
                closes_at: "2026-08-07T12:00:00+00:00".into(),
                allow_multi: false,
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(PollServiceError::Repository(PollRepoError::Conflict))
        ));
        let captured = repo
            .0
            .lock()
            .expect("capture lock")
            .clone()
            .expect("captured");
        assert_eq!(captured.title, "Dinner?");
        assert_eq!(captured.description, "context");
        assert_eq!(captured.closes_at, "2026-08-07T12:00:00Z");
        assert_eq!(
            captured
                .options
                .iter()
                .map(|option| option.label.as_str())
                .collect::<Vec<_>>(),
            ["Ramen", "Sushi"]
        );

        let duplicate = create_poll(
            &repo,
            &ids,
            &FixedClock,
            "trip-a",
            &authorization,
            CreatePollInput {
                title: "Dinner?".into(),
                description: String::new(),
                options: vec!["Ramen".into(), " Ramen ".into()],
                closes_at: "2026-08-07T12:00:00Z".into(),
                allow_multi: false,
            },
        )
        .await;
        assert!(matches!(duplicate, Err(PollServiceError::Validation(_))));
    }

    #[test]
    fn plan_change_window_is_server_owned() {
        let ids = Ids(Mutex::new(0));
        let poll = new_plan_change_poll(&ids, "2026-08-06T12:00:00Z").expect("window");
        assert_eq!(poll.created_at, "2026-08-06T12:00:00Z");
        assert_eq!(poll.closes_at, "2026-08-13T12:00:00.000Z");
        assert_ne!(poll.adopt_option_id, poll.keep_option_id);
    }

    fn stored_decision_poll() -> Poll {
        Poll {
            id: "poll-a".into(),
            trip_id: "trip-a".into(),
            created_by: "user-a".into(),
            kind: PollKind::Decision,
            title: "Dinner".into(),
            description: String::new(),
            options: vec![
                PollOption {
                    id: "ramen".into(),
                    label: "Ramen".into(),
                    proposal_id: None,
                },
                PollOption {
                    id: "pizza".into(),
                    label: "Pizza".into(),
                    proposal_id: None,
                },
            ],
            opens_at: None,
            closes_at: "2026-08-08T12:00:00Z".into(),
            decided_at: None,
            quorum: 1,
            allow_multi: false,
            status: PollStatus::Open,
            votes: Vec::new(),
            resolution_note: None,
        }
    }

    #[test]
    fn stored_poll_validation_covers_ballot_time_shape_and_terminal_majority() {
        let created_at = "2026-08-07T12:00:00Z";
        let mut poll = stored_decision_poll();
        assert!(validate_stored_poll("trip-a", &poll, created_at).is_ok());

        poll.votes.push(PollVote {
            user_id: "user-a".into(),
            option_id: "ramen".into(),
            at: poll.closes_at.clone(),
        });
        assert!(validate_stored_poll("trip-a", &poll, created_at).is_err());

        poll.votes[0].at = "2026-08-07T13:00:00Z".into();
        poll.votes.push(PollVote {
            user_id: "user-a".into(),
            option_id: "pizza".into(),
            at: "2026-08-07T13:01:00Z".into(),
        });
        poll.allow_multi = true;
        assert!(validate_stored_poll("trip-a", &poll, created_at).is_err());

        poll.votes = vec![
            PollVote {
                user_id: "user-a".into(),
                option_id: "ramen".into(),
                at: "2026-08-07T13:00:00Z".into(),
            },
            PollVote {
                user_id: "user-b".into(),
                option_id: "pizza".into(),
                at: "2026-08-07T13:00:00Z".into(),
            },
        ];
        poll.allow_multi = false;
        poll.status = PollStatus::Passed;
        poll.decided_at = Some("2026-08-07T14:00:00Z".into());
        assert!(validate_stored_poll("trip-a", &poll, created_at).is_err());

        poll.votes[1].option_id = "ramen".into();
        assert!(validate_stored_poll("trip-a", &poll, created_at).is_ok());
    }

    #[test]
    fn stored_plan_poll_result_must_match_the_winning_option() {
        let created_at = "2026-08-07T12:00:00Z";
        let mut poll = stored_decision_poll();
        poll.kind = PollKind::PlanChange;
        poll.options[0] = PollOption {
            id: "adopt".into(),
            label: ADOPT_LABEL.into(),
            proposal_id: Some("proposal-a".into()),
        };
        poll.options[1] = PollOption {
            id: "keep".into(),
            label: KEEP_LABEL.into(),
            proposal_id: None,
        };
        poll.votes = vec![PollVote {
            user_id: "user-a".into(),
            option_id: "adopt".into(),
            at: "2026-08-07T13:00:00Z".into(),
        }];
        poll.status = PollStatus::Failed;
        poll.decided_at = Some("2026-08-07T14:00:00Z".into());
        poll.resolution_note = Some(KEEP_NOTE.into());
        assert!(validate_stored_poll("trip-a", &poll, created_at).is_err());

        poll.resolution_note = Some(STALE_NOTE.into());
        assert!(validate_stored_poll("trip-a", &poll, created_at).is_ok());
    }
}
