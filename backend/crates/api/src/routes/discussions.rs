use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection, rejection::JsonRejection},
    http::StatusCode,
};
use itinera_core::{
    domain::discussion::{Comment, DiscussionThread, ThreadAnchor},
    services::discussions::{self, CreateThreadInput},
};
use serde::Deserialize;

use crate::{
    auth::AuthenticatedPrincipal, error::ApiError, routes::require_empty_body, state::AppState,
};

pub const DISCUSSION_BODYLESS_LIMIT_BYTES: usize = 1_024;
pub const DISCUSSION_WRITE_BODY_LIMIT_BYTES: usize = 64 * 1_024;
pub const DISCUSSION_REACTION_BODY_LIMIT_BYTES: usize = 1_024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateThreadRequest {
    anchor: ThreadAnchor,
    title: String,
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddCommentRequest {
    body: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetReactionRequest {
    emoji: String,
    active: bool,
}

pub async fn list_threads(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Vec<DiscussionThread>>, ApiError> {
    let actor = principal.require_trip_read(&trip_id)?;
    require_empty_body(body)?;
    Ok(Json(
        discussions::list_threads(&*state.discussions, &trip_id, &actor.id).await?,
    ))
}

pub async fn create_thread(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
    payload: Result<Json<CreateThreadRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<DiscussionThread>), ApiError> {
    let actor = principal.require_human()?;
    let Json(request) = payload?;
    let thread = discussions::create_thread(
        &*state.discussions,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &actor.id,
        CreateThreadInput {
            anchor: request.anchor,
            title: request.title,
            body: request.body,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(thread)))
}

pub async fn get_comments(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, thread_id)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Vec<Comment>>, ApiError> {
    let actor = principal.require_trip_read(&trip_id)?;
    require_empty_body(body)?;
    Ok(Json(
        discussions::get_comments(&*state.discussions, &trip_id, &actor.id, &thread_id).await?,
    ))
}

pub async fn add_comment(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, thread_id)): Path<(String, String)>,
    payload: Result<Json<AddCommentRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Comment>), ApiError> {
    let actor = principal.require_human()?;
    let Json(request) = payload?;
    let comment = discussions::add_comment(
        &*state.discussions,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &actor.id,
        &thread_id,
        request.body,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(comment)))
}

pub async fn set_reaction(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, thread_id, comment_id)): Path<(String, String, String)>,
    payload: Result<Json<SetReactionRequest>, JsonRejection>,
) -> Result<Json<Comment>, ApiError> {
    let actor = principal.require_human()?;
    let Json(request) = payload?;
    Ok(Json(
        discussions::set_reaction(
            &*state.discussions,
            &trip_id,
            &actor.id,
            &thread_id,
            &comment_id,
            request.emoji,
            request.active,
        )
        .await?,
    ))
}
