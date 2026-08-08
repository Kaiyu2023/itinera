use std::collections::HashSet;

use chrono::DateTime;

use crate::{
    domain::discussion::{Comment, DiscussionThread, ThreadAnchor},
    ports::{
        authorization::TripAuthorizationContext,
        clock::Clock,
        discussion::{DiscussionRepo, DiscussionRepoError, NewComment, NewThread},
        id_gen::IdGen,
    },
};

use super::validation::{ValidationError, exact_required_text, required_text};

pub const MAX_THREAD_TITLE_CHARS: usize = 200;
pub const MAX_COMMENT_BODY_CHARS: usize = 10_000;
pub const MAX_REACTION_CHARS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateThreadInput {
    pub anchor: ThreadAnchor,
    pub title: String,
    pub body: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DiscussionServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] DiscussionRepoError),
}

pub async fn list_threads(
    repo: &dyn DiscussionRepo,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<DiscussionThread>, DiscussionServiceError> {
    validate_id(trip_id, "tripId is invalid")?;
    Ok(repo.list_threads(trip_id, authorization).await?)
}

pub async fn create_thread(
    repo: &dyn DiscussionRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    input: CreateThreadInput,
) -> Result<DiscussionThread, DiscussionServiceError> {
    require_human(authorization)?;
    validate_id(trip_id, "tripId is invalid")?;
    validate_thread_anchor(&input.anchor)?;
    let created_at = validated_now(clock)?;
    let title = required_text(
        input.title,
        "title is required and must be at most 200 characters",
        MAX_THREAD_TITLE_CHARS,
    )?;
    let body = normalise_body(input.body)?;
    let id = ids.new_id();
    let first_comment_id = ids.new_id();
    validate_id(&id, "generated thread id is invalid")?;
    validate_id(&first_comment_id, "generated comment id is invalid")?;
    Ok(repo
        .create_thread(
            trip_id,
            authorization,
            NewThread {
                id,
                first_comment_id,
                anchor: input.anchor,
                title,
                body,
                created_at,
            },
        )
        .await?)
}

pub async fn get_comments(
    repo: &dyn DiscussionRepo,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    thread_id: &str,
) -> Result<Vec<Comment>, DiscussionServiceError> {
    validate_route_ids(trip_id, thread_id)?;
    Ok(repo.get_comments(trip_id, authorization, thread_id).await?)
}

pub async fn add_comment(
    repo: &dyn DiscussionRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    thread_id: &str,
    body: String,
) -> Result<Comment, DiscussionServiceError> {
    require_human(authorization)?;
    validate_route_ids(trip_id, thread_id)?;
    let id = ids.new_id();
    validate_id(&id, "generated comment id is invalid")?;
    Ok(repo
        .add_comment(
            trip_id,
            authorization,
            thread_id,
            NewComment {
                id,
                body: normalise_body(body)?,
                created_at: validated_now(clock)?,
            },
        )
        .await?)
}

pub async fn set_reaction(
    repo: &dyn DiscussionRepo,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    thread_id: &str,
    comment_id: &str,
    emoji: String,
    active: bool,
) -> Result<Comment, DiscussionServiceError> {
    require_human(authorization)?;
    validate_route_ids(trip_id, thread_id)?;
    validate_id(comment_id, "commentId is invalid")?;
    let emoji = normalise_emoji(emoji)?;
    Ok(repo
        .set_reaction(
            trip_id,
            authorization,
            thread_id,
            comment_id,
            &emoji,
            active,
        )
        .await?)
}

fn require_human(authorization: &TripAuthorizationContext) -> Result<(), DiscussionServiceError> {
    authorization
        .human_user_id()
        .map(|_| ())
        .ok_or_else(|| DiscussionRepoError::Forbidden.into())
}

pub fn validate_stored_thread(
    expected_trip_id: &str,
    thread: &DiscussionThread,
) -> Result<(), ValidationError> {
    validate_id(&thread.id, "stored thread id is invalid")?;
    if thread.trip_id != expected_trip_id {
        return Err(ValidationError("stored thread belongs to another trip"));
    }
    validate_thread_anchor(&thread.anchor)?;
    exact_required_text(
        &thread.title,
        "stored thread title is invalid",
        MAX_THREAD_TITLE_CHARS,
    )?;
    if thread.comment_count == 0 || thread.comment_count > 1_000 {
        return Err(ValidationError("stored thread comment count is invalid"));
    }
    parse_utc(&thread.last_activity_at, "stored thread time is invalid")?;
    Ok(())
}

pub fn validate_stored_comment(
    expected_thread_id: &str,
    comment: &Comment,
) -> Result<(), ValidationError> {
    validate_id(&comment.id, "stored comment id is invalid")?;
    if comment.thread_id != expected_thread_id {
        return Err(ValidationError("stored comment belongs to another thread"));
    }
    validate_id(&comment.author, "stored comment author is invalid")?;
    exact_required_text(
        &comment.body,
        "stored comment body is invalid",
        MAX_COMMENT_BODY_CHARS,
    )?;
    parse_utc(&comment.created_at, "stored comment time is invalid")?;
    if comment.reactions.len() > 1_000 {
        return Err(ValidationError("stored reactions are invalid"));
    }
    let mut emojis = HashSet::new();
    for reaction in &comment.reactions {
        if normalise_emoji(reaction.emoji.clone()).as_ref() != Ok(&reaction.emoji)
            || reaction.user_ids.is_empty()
            || reaction.user_ids.len() > 1_000
            || !emojis.insert(reaction.emoji.as_str())
        {
            return Err(ValidationError("stored reactions are invalid"));
        }
        let mut users = HashSet::new();
        for user_id in &reaction.user_ids {
            validate_id(user_id, "stored reaction user id is invalid")?;
            if !users.insert(user_id.as_str()) {
                return Err(ValidationError("stored reaction users are duplicated"));
            }
        }
    }
    Ok(())
}

fn validate_route_ids(trip_id: &str, thread_id: &str) -> Result<(), ValidationError> {
    validate_id(trip_id, "tripId is invalid")?;
    validate_id(thread_id, "threadId is invalid")
}

pub fn validate_thread_anchor(anchor: &ThreadAnchor) -> Result<(), ValidationError> {
    match anchor {
        ThreadAnchor::Trip => Ok(()),
        ThreadAnchor::Day { day_id } => validate_id(day_id, "anchor dayId is invalid"),
        ThreadAnchor::Stop { stop_id } => validate_id(stop_id, "anchor stopId is invalid"),
        ThreadAnchor::Poll { poll_id } => validate_id(poll_id, "anchor pollId is invalid"),
        ThreadAnchor::Candidate { candidate_id } => {
            validate_id(candidate_id, "anchor candidateId is invalid")
        }
    }
}

fn normalise_body(body: String) -> Result<String, ValidationError> {
    required_text(
        body,
        "body is required and must be at most 10,000 characters",
        MAX_COMMENT_BODY_CHARS,
    )
}

fn normalise_emoji(emoji: String) -> Result<String, ValidationError> {
    let emoji = required_text(
        emoji,
        "emoji is required and must be at most 16 characters",
        MAX_REACTION_CHARS,
    )?;
    if emoji
        .chars()
        .any(|value| value.is_control() || value.is_whitespace())
    {
        Err(ValidationError(
            "emoji must not contain whitespace or controls",
        ))
    } else {
        Ok(emoji)
    }
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
        domain::discussion::Reaction,
        ports::discussion::{NewComment, NewThread},
    };

    use super::*;

    struct Ids(Mutex<Vec<String>>);

    impl IdGen for Ids {
        fn new_id(&self) -> String {
            self.0.lock().expect("id lock").remove(0)
        }
    }

    struct FixedClock(&'static str);

    impl Clock for FixedClock {
        fn now(&self) -> String {
            self.0.into()
        }
    }

    #[derive(Default)]
    struct CapturingRepo {
        thread: Mutex<Option<NewThread>>,
        comment: Mutex<Option<NewComment>>,
        reaction: Mutex<Option<(String, bool)>>,
    }

    #[async_trait]
    impl DiscussionRepo for CapturingRepo {
        async fn list_threads(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
        ) -> Result<Vec<DiscussionThread>, DiscussionRepoError> {
            Ok(vec![])
        }

        async fn create_thread(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
            new: NewThread,
        ) -> Result<DiscussionThread, DiscussionRepoError> {
            *self.thread.lock().expect("thread lock") = Some(new);
            Err(DiscussionRepoError::Conflict)
        }

        async fn get_comments(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
            _: &str,
        ) -> Result<Vec<Comment>, DiscussionRepoError> {
            Ok(vec![])
        }

        async fn add_comment(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
            _: &str,
            new: NewComment,
        ) -> Result<Comment, DiscussionRepoError> {
            *self.comment.lock().expect("comment lock") = Some(new);
            Err(DiscussionRepoError::Conflict)
        }

        async fn set_reaction(
            &self,
            _: &str,
            _: &TripAuthorizationContext,
            _: &str,
            _: &str,
            emoji: &str,
            active: bool,
        ) -> Result<Comment, DiscussionRepoError> {
            *self.reaction.lock().expect("reaction lock") = Some((emoji.into(), active));
            Err(DiscussionRepoError::Conflict)
        }
    }

    #[tokio::test]
    async fn thread_and_comment_commands_normalise_text_and_own_ids_and_times() {
        let repo = CapturingRepo::default();
        let ids = Ids(Mutex::new(vec!["thread-a".into(), "comment-a".into()]));
        let authorization =
            TripAuthorizationContext::human(crate::domain::user::UserId("user-a".into()));
        let result = create_thread(
            &repo,
            &ids,
            &FixedClock("2026-08-06T10:00:00Z"),
            "trip-a",
            &authorization,
            CreateThreadInput {
                anchor: ThreadAnchor::Trip,
                title: "  General  ".into(),
                body: "  First thought  ".into(),
            },
        )
        .await;
        assert!(matches!(
            result,
            Err(DiscussionServiceError::Repository(
                DiscussionRepoError::Conflict
            ))
        ));
        let captured = repo
            .thread
            .lock()
            .expect("thread lock")
            .clone()
            .expect("captured thread");
        assert_eq!(captured.id, "thread-a");
        assert_eq!(captured.first_comment_id, "comment-a");
        assert_eq!(captured.title, "General");
        assert_eq!(captured.body, "First thought");
        assert_eq!(captured.created_at, "2026-08-06T10:00:00Z");

        let comment_ids = Ids(Mutex::new(vec!["comment-b".into()]));
        let _ = add_comment(
            &repo,
            &comment_ids,
            &FixedClock("2026-08-06T10:01:00+00:00"),
            "trip-a",
            &authorization,
            "thread-a",
            "  Follow-up  ".into(),
        )
        .await;
        let captured = repo
            .comment
            .lock()
            .expect("comment lock")
            .clone()
            .expect("captured comment");
        assert_eq!(captured.body, "Follow-up");
        assert_eq!(captured.created_at, "2026-08-06T10:01:00+00:00");
    }

    #[tokio::test]
    async fn reaction_is_a_validated_desired_state_owned_by_the_actor() {
        let repo = CapturingRepo::default();
        let authorization =
            TripAuthorizationContext::human(crate::domain::user::UserId("user-a".into()));
        let result = set_reaction(
            &repo,
            "trip-a",
            &authorization,
            "thread-a",
            "comment-a",
            "  👍  ".into(),
            true,
        )
        .await;
        assert!(matches!(
            result,
            Err(DiscussionServiceError::Repository(
                DiscussionRepoError::Conflict
            ))
        ));
        assert_eq!(
            *repo.reaction.lock().expect("reaction lock"),
            Some(("👍".into(), true))
        );

        assert!(matches!(
            set_reaction(
                &repo,
                "trip-a",
                &authorization,
                "thread-a",
                "comment-a",
                "two words".into(),
                true,
            )
            .await,
            Err(DiscussionServiceError::Validation(_))
        ));
    }

    #[test]
    fn stored_discussion_validation_rejects_offsets_limits_and_duplicates() {
        let mut comment = Comment {
            id: "comment-a".into(),
            thread_id: "thread-a".into(),
            author: "user-a".into(),
            body: "hello".into(),
            created_at: "2026-08-06T10:00:00+01:00".into(),
            reactions: vec![],
        };
        assert!(validate_stored_comment("thread-a", &comment).is_err());
        comment.created_at = "2026-08-06T09:00:00Z".into();
        comment.reactions = vec![Reaction {
            emoji: "👍".into(),
            user_ids: vec!["user-a".into(), "user-a".into()],
        }];
        assert!(validate_stored_comment("thread-a", &comment).is_err());
        comment.reactions = (0..=1_000)
            .map(|index| Reaction {
                emoji: format!("x{index}"),
                user_ids: vec!["user-a".into()],
            })
            .collect();
        assert!(validate_stored_comment("thread-a", &comment).is_err());

        assert!(
            validate_thread_anchor(&ThreadAnchor::Day {
                day_id: " x ".into()
            })
            .is_err()
        );
    }
}
