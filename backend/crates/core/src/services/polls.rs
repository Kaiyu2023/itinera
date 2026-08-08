use std::collections::HashSet;

use chrono::{DateTime, Days, SecondsFormat};

use crate::{
    domain::poll::{Poll, PollOption},
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
    let created_at = clock.now();
    let created = parse_utc(&created_at, "server time is invalid")?;
    let closes = parse_utc(
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
                closes_at: input.closes_at,
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
        .open_poll(trip_id, authorization, poll_id, &validated_now(clock)?)
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
        .cast_vote(
            trip_id,
            authorization,
            poll_id,
            &option_ids,
            &validated_now(clock)?,
        )
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
    let created = parse_utc(created_at, "server time is invalid")?;
    let closes = created
        .checked_add_days(Days::new(PLAN_CHANGE_POLL_DAYS))
        .ok_or(ValidationError(
            "server time is outside the supported range",
        ))?;
    Ok(NewPlanChangePoll {
        poll_id: ids.new_id(),
        adopt_option_id: ids.new_id(),
        keep_option_id: ids.new_id(),
        created_at: created_at.to_string(),
        closes_at: closes.to_rfc3339_opts(SecondsFormat::Millis, true),
    })
}

fn validate_route_ids(trip_id: &str, poll_id: &str) -> Result<(), ValidationError> {
    validate_id(trip_id, "tripId is invalid")?;
    validate_id(poll_id, "pollId is invalid")
}

fn validated_now(clock: &dyn Clock) -> Result<String, ValidationError> {
    let now = clock.now();
    parse_utc(&now, "server time is invalid")?;
    Ok(now)
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
        domain::{poll::Poll, proposal::Proposal},
        ports::{
            id_gen::IdGen,
            poll::{NewDecisionPoll, NewPlanChangePoll, PollRepo, PollRepoError},
            proposal::ProposalApplicationIds,
        },
    };

    use super::*;

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
            _: &str,
        ) -> Result<Poll, PollRepoError> {
            unreachable!()
        }
        async fn cast_vote(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
            _: &str,
            _: &[String],
            _: &str,
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
                closes_at: "2026-08-07T12:00:00Z".into(),
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
}
