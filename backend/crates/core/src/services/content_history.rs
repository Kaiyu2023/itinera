use std::collections::HashSet;

use chrono::DateTime;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    domain::{
        content_history::{ChangeSource, Edit, EditEntity, EditStatus},
        notice::NoticeStatus,
        trip::{Booking, CandidateStatus, Place, TripStatus},
    },
    ports::{
        authorization::TripAuthorizationContext,
        clock::Clock,
        content_history::{ContentHistoryRepo, ContentHistoryRepoError},
        id_gen::IdGen,
    },
};

use super::{
    candidates::validate_candidate_place_revision,
    notices::{MAX_NOTICE_AUDIENCE, MAX_NOTICE_BODY_CHARS, MAX_NOTICE_TITLE_CHARS},
    validation::{
        ValidationError, duration_min, exact_bounded_strings, exact_required_text, http_url,
        local_time, required_text, text_len, validate_booking,
    },
};

const INVALID_STORED_EDIT: ValidationError = ValidationError("stored content edit is invalid");

/// Validate the canonical, typed shape shared by every persisted content edit.
///
/// Repositories still own relational checks (trip membership, service mapping,
/// live entity revisions, and reciprocal revert links), but raw storage rows
/// are converted directly into this domain value and passed through the same
/// field validators used by application services.
pub fn validate_stored_edit(expected_trip_id: &str, edit: &Edit) -> Result<(), ValidationError> {
    if edit.trip_id != expected_trip_id
        || exact_required_text(&edit.id, "edit id is invalid", 200).is_err()
        || exact_required_text(&edit.trip_id, "trip id is invalid", 200).is_err()
        || exact_required_text(&edit.entity_id, "entity id is invalid", 200).is_err()
        || exact_required_text(&edit.field, "edit field is invalid", 200).is_err()
        || exact_required_text(&edit.author, "edit author is invalid", 200).is_err()
        || !is_canonical_utc(&edit.created_at)
        || edit.old_value == edit.new_value
    {
        return Err(INVALID_STORED_EDIT);
    }

    match &edit.source {
        ChangeSource::Web {} => {}
        ChangeSource::Service {
            service_identity_id,
            service_identity_name,
        } if exact_required_text(service_identity_id, "service identity id is invalid", 200)
            .is_ok()
            && exact_required_text(
                service_identity_name,
                "service identity name is invalid",
                200,
            )
            .is_ok() => {}
        ChangeSource::Service { .. } => return Err(INVALID_STORED_EDIT),
    }

    let complete_revert =
        edit.reverted_by.as_deref().is_some_and(|value| {
            exact_required_text(value, "revert actor is invalid", 200).is_ok()
        }) && edit.reverted_at.as_deref().is_some_and(is_canonical_utc)
            && edit.revert_edit_id.as_deref().is_some_and(|value| {
                exact_required_text(value, "revert edit id is invalid", 200).is_ok()
            });
    match edit.status {
        EditStatus::Applied
            if edit.reverted_by.is_some()
                || edit.reverted_at.is_some()
                || edit.revert_edit_id.is_some() =>
        {
            return Err(INVALID_STORED_EDIT);
        }
        EditStatus::Applied => {}
        EditStatus::Reverted if complete_revert => {}
        EditStatus::Reverted | EditStatus::PendingReview | EditStatus::Rejected => {
            return Err(INVALID_STORED_EDIT);
        }
    }
    if edit.revert_edit_id.as_deref().is_some_and(|value| {
        value == edit.id || exact_required_text(value, "revert edit id is invalid", 200).is_err()
    }) || edit.reverts_edit_id.as_deref().is_some_and(|value| {
        value == edit.id || exact_required_text(value, "original edit id is invalid", 200).is_err()
    }) {
        return Err(INVALID_STORED_EDIT);
    }

    validate_typed_values(edit)
}

fn validate_typed_values(edit: &Edit) -> Result<(), ValidationError> {
    match (edit.entity, edit.field.as_str()) {
        (EditEntity::Trip, "status") => {
            if edit.entity_id != edit.trip_id {
                return Err(INVALID_STORED_EDIT);
            }
            parse_exact::<TripStatus>(&edit.old_value)?;
            parse_exact::<TripStatus>(&edit.new_value)?;
        }
        (EditEntity::Candidate, "status") => {
            parse_exact::<CandidateStatus>(&edit.old_value)?;
            parse_exact::<CandidateStatus>(&edit.new_value)?;
        }
        (EditEntity::Candidate, "pitch") => {
            validate_required_value::<2_000>(&edit.old_value)?;
            validate_required_value::<2_000>(&edit.new_value)?;
        }
        (EditEntity::Candidate, "tags") => {
            validate_tags(&edit.old_value)?;
            validate_tags(&edit.new_value)?;
        }
        (EditEntity::Candidate, "place") => {
            let old = parse_exact::<Place>(&edit.old_value)?;
            let new = parse_exact::<Place>(&edit.new_value)?;
            validate_candidate_place_revision(&old, &new).map_err(|_| INVALID_STORED_EDIT)?;
        }
        (EditEntity::Day, "windowStart" | "windowEnd") | (EditEntity::Stop, "plannedArrival") => {
            validate_time_value(&edit.old_value)?;
            validate_time_value(&edit.new_value)?;
        }
        (EditEntity::Day, "cityHint") => {
            validate_required_value::<120>(&edit.old_value)?;
            validate_required_value::<120>(&edit.new_value)?;
        }
        (EditEntity::Stop, "durationMin") => {
            validate_duration_value(&edit.old_value)?;
            validate_duration_value(&edit.new_value)?;
        }
        (EditEntity::Stop, "notes") => {
            validate_text_value::<10_000>(&edit.old_value)?;
            validate_text_value::<10_000>(&edit.new_value)?;
        }
        (EditEntity::Stop, "booking") => {
            let old = parse_exact::<Option<Booking>>(&edit.old_value)?;
            let new = parse_exact::<Option<Booking>>(&edit.new_value)?;
            validate_booking(old.as_ref()).map_err(|_| INVALID_STORED_EDIT)?;
            validate_booking(new.as_ref()).map_err(|_| INVALID_STORED_EDIT)?;
            if booking_link(old.as_ref()) != booking_link(new.as_ref()) {
                return Err(INVALID_STORED_EDIT);
            }
        }
        (EditEntity::Notice, "title") => {
            validate_required_value::<MAX_NOTICE_TITLE_CHARS>(&edit.old_value)?;
            validate_required_value::<MAX_NOTICE_TITLE_CHARS>(&edit.new_value)?;
        }
        (EditEntity::Notice, "body") => {
            validate_required_value::<MAX_NOTICE_BODY_CHARS>(&edit.old_value)?;
            validate_required_value::<MAX_NOTICE_BODY_CHARS>(&edit.new_value)?;
        }
        (EditEntity::Notice, "pinned") => {
            parse_exact::<bool>(&edit.old_value)?;
            parse_exact::<bool>(&edit.new_value)?;
        }
        (EditEntity::Notice, "sourceUrl") => {
            validate_url_value(&edit.old_value)?;
            validate_url_value(&edit.new_value)?;
        }
        (EditEntity::Notice, "status") => {
            parse_exact::<NoticeStatus>(&edit.old_value)?;
            parse_exact::<NoticeStatus>(&edit.new_value)?;
        }
        (EditEntity::Notice, "audience") => {
            validate_audience(&edit.old_value)?;
            validate_audience(&edit.new_value)?;
        }
        _ => return Err(INVALID_STORED_EDIT),
    }
    Ok(())
}

fn parse_exact<T>(value: &Value) -> Result<T, ValidationError>
where
    T: DeserializeOwned + Serialize,
{
    let parsed = serde_json::from_value::<T>(value.clone()).map_err(|_| INVALID_STORED_EDIT)?;
    if serde_json::to_value(&parsed).map_err(|_| INVALID_STORED_EDIT)? == *value {
        Ok(parsed)
    } else {
        Err(INVALID_STORED_EDIT)
    }
}

fn validate_required_value<const MAX: usize>(value: &Value) -> Result<(), ValidationError> {
    let value = parse_exact::<String>(value)?;
    exact_required_text(&value, "stored text is invalid", MAX).map_err(|_| INVALID_STORED_EDIT)
}

fn validate_text_value<const MAX: usize>(value: &Value) -> Result<(), ValidationError> {
    let value = parse_exact::<String>(value)?;
    text_len(&value, MAX).map_err(|_| INVALID_STORED_EDIT)
}

fn validate_tags(value: &Value) -> Result<(), ValidationError> {
    let tags = parse_exact::<Vec<String>>(value)?;
    exact_bounded_strings(&tags, 20, 60).map_err(|_| INVALID_STORED_EDIT)
}

fn validate_time_value(value: &Value) -> Result<(), ValidationError> {
    let value = parse_exact::<String>(value)?;
    local_time(&value).map_err(|_| INVALID_STORED_EDIT)
}

fn validate_duration_value(value: &Value) -> Result<(), ValidationError> {
    let value = parse_exact::<u32>(value)?;
    duration_min(value).map_err(|_| INVALID_STORED_EDIT)
}

fn validate_url_value(value: &Value) -> Result<(), ValidationError> {
    let value = parse_exact::<Option<String>>(value)?;
    if http_url(value.clone()).map_err(|_| INVALID_STORED_EDIT)? == value {
        Ok(())
    } else {
        Err(INVALID_STORED_EDIT)
    }
}

fn validate_audience(value: &Value) -> Result<(), ValidationError> {
    let Some(audience) = parse_exact::<Option<Vec<String>>>(value)? else {
        return Ok(());
    };
    if audience.is_empty() || audience.len() > MAX_NOTICE_AUDIENCE {
        return Err(INVALID_STORED_EDIT);
    }
    let mut unique = HashSet::new();
    if audience.iter().any(|user_id| {
        exact_required_text(user_id, "notice audience is invalid", 200).is_err()
            || !unique.insert(user_id.as_str())
    }) {
        return Err(INVALID_STORED_EDIT);
    }
    Ok(())
}

fn booking_link(booking: Option<&Booking>) -> Option<&str> {
    booking.and_then(|booking| booking.ledger_entry_id.as_deref())
}

fn is_canonical_utc(value: &str) -> bool {
    value.len() <= 64
        && value.ends_with('Z')
        && DateTime::parse_from_rfc3339(value)
            .is_ok_and(|timestamp| timestamp.offset().local_minus_utc() == 0)
}

#[derive(Debug, thiserror::Error)]
pub enum ContentHistoryServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] ContentHistoryRepoError),
}

pub async fn get_history(
    repo: &dyn ContentHistoryRepo,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
) -> Result<Vec<Edit>, ContentHistoryServiceError> {
    repo.list_history(trip_id, authorization)
        .await
        .map_err(Into::into)
}

pub async fn revert_edit(
    repo: &dyn ContentHistoryRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    authorization: &TripAuthorizationContext,
    edit_id: &str,
) -> Result<(), ContentHistoryServiceError> {
    if authorization.human_user_id().is_none() {
        return Err(ContentHistoryRepoError::Forbidden.into());
    }
    let edit_id = required_text(
        edit_id.to_string(),
        "editId is required and must be at most 200 characters",
        200,
    )?;
    repo.revert_edit(
        trip_id,
        authorization,
        &edit_id,
        &clock.now(),
        &ids.new_id(),
    )
    .await
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ChangeSource, Edit, EditEntity, EditStatus, validate_stored_edit};

    fn edit() -> Edit {
        Edit {
            id: "edit-a".into(),
            trip_id: "trip-a".into(),
            entity: EditEntity::Trip,
            entity_id: "trip-a".into(),
            field: "status".into(),
            old_value: json!("dreaming"),
            new_value: json!("planning"),
            author: "user-a".into(),
            source: ChangeSource::Web {},
            status: EditStatus::Applied,
            created_at: "2026-08-07T12:00:00.000Z".into(),
            reverted_by: None,
            reverted_at: None,
            revert_edit_id: None,
            reverts_edit_id: None,
        }
    }

    #[test]
    fn stored_edits_use_one_typed_canonical_validation_boundary() {
        assert!(validate_stored_edit("trip-a", &edit()).is_ok());

        let mut malformed = edit();
        malformed.new_value = json!("not-a-trip-status");
        assert!(validate_stored_edit("trip-a", &malformed).is_err());

        let mut malformed = edit();
        malformed.created_at = "2026-08-07T13:00:00+01:00".into();
        assert!(validate_stored_edit("trip-a", &malformed).is_err());

        let mut malformed = edit();
        malformed.reverted_by = Some("user-a".into());
        assert!(validate_stored_edit("trip-a", &malformed).is_err());

        let mut malformed = edit();
        malformed.new_value = malformed.old_value.clone();
        assert!(validate_stored_edit("trip-a", &malformed).is_err());

        let mut service = edit();
        service.source = ChangeSource::Service {
            service_identity_id: "service-a".into(),
            service_identity_name: "Assistant".into(),
        };
        assert!(validate_stored_edit("trip-a", &service).is_ok());
    }

    #[test]
    fn booking_history_cannot_change_the_server_owned_ledger_link() {
        let mut booking = edit();
        booking.entity = EditEntity::Stop;
        booking.entity_id = "stop-a".into();
        booking.field = "booking".into();
        booking.old_value = json!({
            "ref": "A",
            "url": null,
            "cost": null,
            "ledgerEntryId": "expense-a"
        });
        booking.new_value = json!({
            "ref": "B",
            "url": null,
            "cost": null,
            "ledgerEntryId": "expense-b"
        });
        assert!(validate_stored_edit("trip-a", &booking).is_err());

        booking.new_value["ledgerEntryId"] = json!("expense-a");
        assert!(validate_stored_edit("trip-a", &booking).is_ok());
    }
}
