use std::collections::HashSet;

use chrono::DateTime;
use serde::Serialize;

use crate::{
    domain::{
        notice::{ChecklistItem, ChecklistMode, Notice, NoticeCategory, NoticeStatus},
        user::UserId,
    },
    ports::{
        clock::Clock,
        id_gen::IdGen,
        notice::{
            ChecklistToggle, NewNotice, NoticePatch, NoticeRepo, NoticeRepoError, NoticeUpdate,
        },
    },
};

use super::{
    idempotency::{request_hash, validate_idempotency_key},
    validation::{ValidationError, date, http_url, required_text},
};

pub const MAX_NOTICES: usize = 1_000;
pub const MAX_NOTICE_RESPONSE_BYTES: usize = 4 * 1_024 * 1_024;
pub const MAX_NOTICE_TITLE_CHARS: usize = 200;
pub const MAX_NOTICE_BODY_CHARS: usize = 10_000;
pub const MAX_CHECKLIST_ITEMS: usize = 100;
pub const MAX_CHECKLIST_TEXT_CHARS: usize = 500;
pub const MAX_NOTICE_AUDIENCE: usize = 90;
pub const MAX_CHECKLIST_COMPLETIONS: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNoticeInput {
    pub category: NoticeCategory,
    pub title: String,
    pub body: String,
    pub source_url: Option<String>,
    pub checklist_items: Vec<String>,
    pub audience: Option<Vec<String>>,
}

#[derive(Debug, thiserror::Error)]
pub enum NoticeServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] NoticeRepoError),
}

pub async fn list_notices(
    repo: &dyn NoticeRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<Vec<Notice>, NoticeServiceError> {
    validate_id(trip_id, "tripId is invalid")?;
    let notices = repo.list_notices(trip_id, actor).await?;
    if notices.len() > MAX_NOTICES
        || serde_json::to_vec(&notices)
            .map_err(|_| NoticeRepoError::CorruptData)?
            .len()
            > MAX_NOTICE_RESPONSE_BYTES
    {
        return Err(NoticeRepoError::SafetyLimitExceeded.into());
    }
    for notice in &notices {
        validate_stored_notice(trip_id, notice)?;
    }
    Ok(notices)
}

pub async fn create_notice(
    repo: &dyn NoticeRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    input: CreateNoticeInput,
) -> Result<Notice, NoticeServiceError> {
    validate_id(trip_id, "tripId is invalid")?;
    validate_idempotency_key(idempotency_key)?;
    let title = required_text(
        input.title,
        "title is required and must be at most 200 characters",
        MAX_NOTICE_TITLE_CHARS,
    )?;
    let body = required_text(
        input.body,
        "body is required and must be at most 10,000 characters",
        MAX_NOTICE_BODY_CHARS,
    )?;
    let source_url = normalise_url(input.source_url)?;
    let audience = normalise_audience(input.audience)?;
    if input.checklist_items.len() > MAX_CHECKLIST_ITEMS {
        return Err(ValidationError("a notice may contain at most 100 checklist items").into());
    }
    let mut checklist_texts = Vec::with_capacity(input.checklist_items.len());
    for text in input.checklist_items {
        let text = required_text(
            text,
            "checklist text is required and must be at most 500 characters",
            MAX_CHECKLIST_TEXT_CHARS,
        )?;
        checklist_texts.push(text);
    }
    let normalized = CreateNoticeInput {
        category: input.category,
        title,
        body,
        source_url,
        checklist_items: checklist_texts,
        audience,
    };
    let request_hash =
        notice_creation_request_hash(&normalized).map_err(|_| NoticeRepoError::CorruptData)?;
    let created_at = validated_now(clock)?;
    if let Some(notice) = repo
        .replay_notice_creation(trip_id, actor, idempotency_key, &request_hash, &created_at)
        .await?
    {
        validate_stored_notice(trip_id, &notice)?;
        return Ok(notice);
    }
    let mut checklist_items = Vec::with_capacity(normalized.checklist_items.len());
    for text in normalized.checklist_items {
        let id = ids.new_id();
        validate_id(&id, "generated checklist item id is invalid")?;
        checklist_items.push(ChecklistItem {
            id,
            text,
            done_by: vec![],
            due_date: None,
            mode: ChecklistMode::Each,
        });
    }
    let id = ids.new_id();
    validate_id(&id, "generated notice id is invalid")?;
    let notice = Notice {
        id,
        trip_id: trip_id.to_string(),
        created_by: actor.0.clone(),
        category: normalized.category,
        title: normalized.title,
        body: normalized.body,
        source_url: normalized.source_url,
        pinned: false,
        status: NoticeStatus::Active,
        audience: normalized.audience,
        checklist_items,
    };
    validate_stored_notice(trip_id, &notice)?;
    Ok(repo
        .create_notice(
            trip_id,
            actor,
            NewNotice {
                notice,
                created_at,
                idempotency_key: idempotency_key.to_string(),
                request_hash,
            },
        )
        .await?)
}

pub async fn update_notice(
    repo: &dyn NoticeRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    notice_id: &str,
    mut patch: NoticePatch,
) -> Result<Notice, NoticeServiceError> {
    validate_route_ids(trip_id, notice_id)?;
    if patch.is_empty() {
        return Err(ValidationError("notice patch must contain at least one field").into());
    }
    if let Some(title) = patch.title.take() {
        patch.title = Some(required_text(
            title,
            "title is required and must be at most 200 characters",
            MAX_NOTICE_TITLE_CHARS,
        )?);
    }
    if let Some(body) = patch.body.take() {
        patch.body = Some(required_text(
            body,
            "body is required and must be at most 10,000 characters",
            MAX_NOTICE_BODY_CHARS,
        )?);
    }
    if let Some(source_url) = patch.source_url.take() {
        patch.source_url = Some(normalise_url(source_url)?);
    }
    if let Some(audience) = patch.audience.take() {
        patch.audience = Some(normalise_audience(audience)?);
    }
    let change_id = ids.new_id();
    validate_id(&change_id, "generated notice change id is invalid")?;
    Ok(repo
        .update_notice(
            trip_id,
            actor,
            notice_id,
            NoticeUpdate {
                patch,
                changed_at: validated_now(clock)?,
                change_id,
            },
        )
        .await?)
}

pub async fn toggle_checklist_item(
    repo: &dyn NoticeRepo,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    notice_id: &str,
    item_id: &str,
    idempotency_key: &str,
) -> Result<Notice, NoticeServiceError> {
    validate_route_ids(trip_id, notice_id)?;
    validate_id(item_id, "itemId is invalid")?;
    validate_idempotency_key(idempotency_key)?;
    let request_hash = checklist_toggle_request_hash(notice_id, item_id)
        .map_err(|_| NoticeRepoError::CorruptData)?;
    let recorded_at = validated_now(clock)?;
    if let Some(notice) = repo
        .replay_checklist_toggle(trip_id, actor, idempotency_key, &request_hash, &recorded_at)
        .await?
    {
        validate_stored_notice(trip_id, &notice)?;
        return Ok(notice);
    }
    Ok(repo
        .toggle_checklist_item(
            trip_id,
            actor,
            notice_id,
            item_id,
            ChecklistToggle {
                idempotency_key: idempotency_key.to_string(),
                request_hash,
                recorded_at,
            },
        )
        .await?)
}

pub fn notice_creation_request_hash(
    input: &CreateNoticeInput,
) -> Result<String, serde_json::Error> {
    request_hash("notice-create", input)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChecklistToggleRequest<'a> {
    notice_id: &'a str,
    item_id: &'a str,
}

pub fn checklist_toggle_request_hash(
    notice_id: &str,
    item_id: &str,
) -> Result<String, serde_json::Error> {
    request_hash(
        "notice-checklist-toggle",
        &ChecklistToggleRequest { notice_id, item_id },
    )
}

pub fn validate_stored_notice(
    expected_trip_id: &str,
    notice: &Notice,
) -> Result<(), ValidationError> {
    validate_id(&notice.id, "stored notice id is invalid")?;
    if notice.trip_id != expected_trip_id {
        return Err(ValidationError("stored notice belongs to another trip"));
    }
    validate_id(&notice.created_by, "stored notice author is invalid")?;
    exact_required(
        &notice.title,
        MAX_NOTICE_TITLE_CHARS,
        "stored notice title is invalid",
    )?;
    exact_required(
        &notice.body,
        MAX_NOTICE_BODY_CHARS,
        "stored notice body is invalid",
    )?;
    if normalise_url(notice.source_url.clone())? != notice.source_url {
        return Err(ValidationError("stored notice source URL is invalid"));
    }
    if notice.checklist_items.len() > MAX_CHECKLIST_ITEMS {
        return Err(ValidationError(
            "stored notice has too many checklist items",
        ));
    }
    let audience = normalise_audience(notice.audience.clone())?;
    if audience != notice.audience {
        return Err(ValidationError("stored notice audience is not canonical"));
    }
    let audience_ids = audience
        .as_ref()
        .map(|values| values.iter().map(String::as_str).collect::<HashSet<_>>());
    let mut item_ids = HashSet::new();
    for item in &notice.checklist_items {
        validate_id(&item.id, "stored checklist item id is invalid")?;
        if !item_ids.insert(item.id.as_str()) {
            return Err(ValidationError("stored checklist item ids are duplicated"));
        }
        exact_required(
            &item.text,
            MAX_CHECKLIST_TEXT_CHARS,
            "stored checklist text is invalid",
        )?;
        if let Some(due_date) = item.due_date.as_deref() {
            date(due_date)?;
        }
        if item.done_by.len() > MAX_CHECKLIST_COMPLETIONS
            || (item.mode == ChecklistMode::Group && item.done_by.len() > 1)
        {
            return Err(ValidationError(
                "stored checklist completion state is invalid",
            ));
        }
        let mut done_by = HashSet::new();
        for user_id in &item.done_by {
            validate_id(user_id, "stored checklist completion user id is invalid")?;
            if !done_by.insert(user_id.as_str())
                || audience_ids
                    .as_ref()
                    .is_some_and(|audience| !audience.contains(user_id.as_str()))
            {
                return Err(ValidationError(
                    "stored checklist completion state is invalid",
                ));
            }
        }
    }
    Ok(())
}

/// Removes checklist acknowledgements that are no longer permitted by both
/// the notice's resulting audience and current direct trip membership. This is
/// server-derived cleanup: callers can choose the audience, but cannot choose
/// or forge another member's stamp.
pub fn retain_audience_completions(notice: &mut Notice, current_members: &HashSet<String>) {
    let permitted = notice
        .audience
        .as_ref()
        .map(|audience| audience.iter().collect::<HashSet<_>>());
    for item in &mut notice.checklist_items {
        item.done_by.retain(|user_id| {
            current_members.contains(user_id)
                && permitted
                    .as_ref()
                    .is_none_or(|audience| audience.contains(user_id))
        });
    }
}

fn normalise_audience(
    audience: Option<Vec<String>>,
) -> Result<Option<Vec<String>>, ValidationError> {
    let Some(audience) = audience else {
        return Ok(None);
    };
    if audience.is_empty() || audience.len() > MAX_NOTICE_AUDIENCE {
        return Err(ValidationError(
            "audience must contain between 1 and 90 members",
        ));
    }
    let mut seen = HashSet::new();
    for user_id in &audience {
        validate_id(user_id, "audience contains an invalid user id")?;
        if !seen.insert(user_id.as_str()) {
            return Err(ValidationError("audience contains duplicate user ids"));
        }
    }
    Ok(Some(audience))
}

fn normalise_url(value: Option<String>) -> Result<Option<String>, ValidationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().to_string();
    if value.is_empty() {
        return Err(ValidationError("source URL must not be empty"));
    }
    http_url(Some(value))
}

fn exact_required(value: &str, max: usize, error: &'static str) -> Result<(), ValidationError> {
    if required_text(value.to_string(), error, max)? == value {
        Ok(())
    } else {
        Err(ValidationError(error))
    }
}

fn validate_route_ids(trip_id: &str, notice_id: &str) -> Result<(), ValidationError> {
    validate_id(trip_id, "tripId is invalid")?;
    validate_id(notice_id, "noticeId is invalid")
}

fn validate_id(value: &str, error: &'static str) -> Result<(), ValidationError> {
    if value.is_empty() || value.trim() != value || value.chars().count() > 200 {
        Err(ValidationError(error))
    } else {
        Ok(())
    }
}

fn validated_now(clock: &dyn Clock) -> Result<String, ValidationError> {
    let now = clock.now();
    let timestamp = DateTime::parse_from_rfc3339(&now)
        .map_err(|_| ValidationError("server time is invalid"))?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err(ValidationError("server time is invalid"));
    }
    Ok(now)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;

    struct Ids(Mutex<Vec<String>>);

    impl IdGen for Ids {
        fn new_id(&self) -> String {
            self.0.lock().expect("ids lock").remove(0)
        }
    }

    struct TestClock;

    impl Clock for TestClock {
        fn now(&self) -> String {
            "2026-08-06T10:00:00Z".into()
        }
    }

    struct Repo(Mutex<Option<NewNotice>>);

    #[async_trait]
    impl NoticeRepo for Repo {
        async fn list_notices(
            &self,
            _trip_id: &str,
            _actor: &UserId,
        ) -> Result<Vec<Notice>, NoticeRepoError> {
            Ok(vec![])
        }

        async fn create_notice(
            &self,
            _trip_id: &str,
            _actor: &UserId,
            new: NewNotice,
        ) -> Result<Notice, NoticeRepoError> {
            *self.0.lock().expect("repo lock") = Some(new.clone());
            Ok(new.notice)
        }

        async fn replay_notice_creation(
            &self,
            _trip_id: &str,
            _actor: &UserId,
            _idempotency_key: &str,
            _request_hash: &str,
            _now: &str,
        ) -> Result<Option<Notice>, NoticeRepoError> {
            Ok(None)
        }

        async fn replay_checklist_toggle(
            &self,
            _trip_id: &str,
            _actor: &UserId,
            _idempotency_key: &str,
            _request_hash: &str,
            _now: &str,
        ) -> Result<Option<Notice>, NoticeRepoError> {
            Ok(None)
        }

        async fn update_notice(
            &self,
            _trip_id: &str,
            _actor: &UserId,
            _notice_id: &str,
            _update: NoticeUpdate,
        ) -> Result<Notice, NoticeRepoError> {
            Err(NoticeRepoError::NotFound)
        }

        async fn toggle_checklist_item(
            &self,
            _trip_id: &str,
            _actor: &UserId,
            _notice_id: &str,
            _item_id: &str,
            _toggle: ChecklistToggle,
        ) -> Result<Notice, NoticeRepoError> {
            Err(NoticeRepoError::NotFound)
        }
    }

    #[tokio::test]
    async fn creation_assigns_only_server_owned_ids_and_empty_completion_state() {
        let repo = Repo(Mutex::new(None));
        let ids = Ids(Mutex::new(vec!["item-a".into(), "notice-a".into()]));
        let notice = create_notice(
            &repo,
            &ids,
            &TestClock,
            "trip-a",
            &UserId("user-a".into()),
            "operation-a",
            CreateNoticeInput {
                category: NoticeCategory::Visa,
                title: " Visa check ".into(),
                body: " Requirements ".into(),
                source_url: Some(" https://example.com/visa ".into()),
                checklist_items: vec![" Apply ".into()],
                audience: Some(vec!["user-a".into()]),
            },
        )
        .await
        .expect("valid notice");

        assert_eq!(notice.id, "notice-a");
        assert_eq!(notice.title, "Visa check");
        assert_eq!(notice.checklist_items[0].id, "item-a");
        assert!(notice.checklist_items[0].done_by.is_empty());
        assert_eq!(notice.checklist_items[0].mode, ChecklistMode::Each);
        let guard = repo.0.lock().expect("repo lock");
        let stored = guard.as_ref().expect("create command recorded");
        assert_eq!(stored.created_at, "2026-08-06T10:00:00Z");
        assert_eq!(stored.idempotency_key, "operation-a");
        assert_eq!(stored.request_hash.len(), 64);
    }

    #[test]
    fn malformed_stored_completion_and_audience_state_fails_closed() {
        let mut notice = Notice {
            id: "notice-a".into(),
            trip_id: "trip-a".into(),
            created_by: "user-a".into(),
            category: NoticeCategory::Money,
            title: "Cash".into(),
            body: "Carry cash".into(),
            source_url: None,
            pinned: false,
            status: NoticeStatus::Active,
            audience: Some(vec!["user-a".into()]),
            checklist_items: vec![ChecklistItem {
                id: "item-a".into(),
                text: "Withdraw".into(),
                done_by: vec!["foreign-user".into()],
                due_date: None,
                mode: ChecklistMode::Each,
            }],
        };
        assert!(validate_stored_notice("trip-a", &notice).is_err());
        notice.checklist_items[0].done_by = vec!["user-a".into(), "user-a".into()];
        assert!(validate_stored_notice("trip-a", &notice).is_err());
    }

    #[test]
    fn audience_cleanup_removes_only_server_derived_excluded_stamps() {
        let mut notice = Notice {
            id: "notice-a".into(),
            trip_id: "trip-a".into(),
            created_by: "user-a".into(),
            category: NoticeCategory::Money,
            title: "Cash".into(),
            body: "Carry cash".into(),
            source_url: None,
            pinned: false,
            status: NoticeStatus::Active,
            audience: Some(vec!["user-a".into()]),
            checklist_items: vec![ChecklistItem {
                id: "item-a".into(),
                text: "Withdraw".into(),
                done_by: vec!["user-a".into(), "departed".into()],
                due_date: None,
                mode: ChecklistMode::Each,
            }],
        };
        retain_audience_completions(
            &mut notice,
            &["user-a".to_string(), "departed".to_string()]
                .into_iter()
                .collect(),
        );
        assert_eq!(notice.checklist_items[0].done_by, vec!["user-a"]);
        assert!(validate_stored_notice("trip-a", &notice).is_ok());
    }

    #[test]
    fn whole_group_cleanup_removes_stamps_from_departed_members() {
        let mut notice = Notice {
            id: "notice-a".into(),
            trip_id: "trip-a".into(),
            created_by: "user-a".into(),
            category: NoticeCategory::Money,
            title: "Cash".into(),
            body: "Carry cash".into(),
            source_url: None,
            pinned: false,
            status: NoticeStatus::Active,
            audience: None,
            checklist_items: vec![ChecklistItem {
                id: "item-a".into(),
                text: "Withdraw".into(),
                done_by: vec!["user-a".into(), "departed".into()],
                due_date: None,
                mode: ChecklistMode::Group,
            }],
        };
        retain_audience_completions(&mut notice, &["user-a".to_string()].into_iter().collect());
        assert_eq!(notice.checklist_items[0].done_by, vec!["user-a"]);
    }

    #[test]
    fn caller_must_use_null_or_omission_to_clear_a_source_url() {
        assert_eq!(normalise_url(None).expect("omission is allowed"), None);
        assert!(normalise_url(Some("   ".into())).is_err());
        assert_eq!(
            normalise_url(Some(" https://example.com/advice ".into())).expect("valid URL"),
            Some("https://example.com/advice".into())
        );
    }
}
