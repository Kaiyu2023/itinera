use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use itinera_core::{
    domain::notice::{Notice, NoticeCategory, NoticeStatus},
    ports::notice::NoticePatch,
    services::notices::{self, CreateNoticeInput},
};
use serde::{Deserialize, Deserializer};

use crate::{
    auth::AuthenticatedPrincipal,
    error::ApiError,
    routes::{require_empty_body, required_idempotency_key},
    state::AppState,
};

pub const NOTICE_BODYLESS_LIMIT_BYTES: usize = 1_024;
pub const NOTICE_WRITE_BODY_LIMIT_BYTES: usize = 64 * 1_024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateNoticeRequest {
    category: NoticeCategory,
    title: String,
    body: String,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    source_url: Option<String>,
    #[serde(default)]
    checklist_items: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    audience: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateNoticeRequest {
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    title: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    body: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    pinned: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    source_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_non_null")]
    status: Option<NoticeStatus>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    audience: Option<Option<Vec<String>>>,
}

pub async fn list_notices(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Vec<Notice>>, ApiError> {
    let actor = principal.require_trip_read(&trip_id)?;
    require_empty_body(body)?;
    Ok(Json(
        notices::list_notices(&*state.notices, &trip_id, &actor.id).await?,
    ))
}

pub async fn create_notice(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<CreateNoticeRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Notice>), ApiError> {
    let actor = principal.require_human()?;
    let Json(request) = payload?;
    let idempotency_key = required_idempotency_key(&headers)?;
    let notice = notices::create_notice(
        &*state.notices,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &actor.id,
        &idempotency_key,
        CreateNoticeInput {
            category: request.category,
            title: request.title,
            body: request.body,
            source_url: request.source_url,
            checklist_items: request.checklist_items,
            audience: request.audience,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(notice)))
}

pub async fn update_notice(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, notice_id)): Path<(String, String)>,
    payload: Result<Json<UpdateNoticeRequest>, JsonRejection>,
) -> Result<Json<Notice>, ApiError> {
    let actor = principal.require_human()?;
    let Json(request) = payload?;
    Ok(Json(
        notices::update_notice(
            &*state.notices,
            &*state.id_gen,
            &*state.clock,
            &trip_id,
            &actor.id,
            &notice_id,
            NoticePatch {
                title: request.title,
                body: request.body,
                pinned: request.pinned,
                source_url: request.source_url,
                status: request.status,
                audience: request.audience,
            },
        )
        .await?,
    ))
}

pub async fn toggle_checklist_item(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, notice_id, item_id)): Path<(String, String, String)>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Notice>, ApiError> {
    let actor = principal.require_human()?;
    require_empty_body(body)?;
    let idempotency_key = required_idempotency_key(&headers)?;
    Ok(Json(
        notices::toggle_checklist_item(
            &*state.notices,
            &*state.clock,
            &trip_id,
            &actor.id,
            &notice_id,
            &item_id,
            &idempotency_key,
        )
        .await?,
    ))
}

fn deserialize_present_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn deserialize_optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{CreateNoticeRequest, UpdateNoticeRequest};

    #[test]
    fn nullable_patch_fields_distinguish_omission_null_and_value() {
        let omitted: UpdateNoticeRequest =
            serde_json::from_value(json!({"pinned": true})).expect("valid patch");
        let cleared: UpdateNoticeRequest = serde_json::from_value(json!({
            "sourceUrl": null,
            "audience": null
        }))
        .expect("valid patch");
        let replaced: UpdateNoticeRequest = serde_json::from_value(json!({
            "sourceUrl": "https://example.com",
            "audience": ["user-a"]
        }))
        .expect("valid patch");
        assert_eq!(omitted.source_url, None);
        assert_eq!(omitted.audience, None);
        assert_eq!(cleared.source_url, Some(None));
        assert_eq!(cleared.audience, Some(None));
        assert_eq!(
            replaced.source_url,
            Some(Some("https://example.com".into()))
        );
        assert_eq!(replaced.audience, Some(Some(vec!["user-a".into()])));
    }

    #[test]
    fn non_nullable_optional_fields_reject_explicit_null() {
        for field in ["title", "body", "pinned", "status"] {
            let mut value = json!({});
            value[field] = serde_json::Value::Null;
            assert!(
                serde_json::from_value::<UpdateNoticeRequest>(value).is_err(),
                "{field} must reject null"
            );
        }
        for field in ["sourceUrl", "audience"] {
            let mut value = json!({
                "category": "safety",
                "title": "Title",
                "body": "Body"
            });
            value[field] = serde_json::Value::Null;
            assert!(
                serde_json::from_value::<CreateNoticeRequest>(value).is_err(),
                "create {field} must reject null"
            );
        }
    }
}
