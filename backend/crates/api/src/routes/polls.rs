use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection, rejection::JsonRejection},
    http::StatusCode,
};
use itinera_core::{
    domain::poll::Poll,
    services::polls::{self, CreatePollInput},
};
use serde::Deserialize;

use crate::{
    auth::AuthenticatedUser, error::ApiError, routes::require_empty_body, state::AppState,
};

pub const POLL_BODYLESS_LIMIT_BYTES: usize = 1_024;
pub const POLL_VOTE_BODY_LIMIT_BYTES: usize = 8 * 1_024;
pub const POLL_CREATE_BODY_LIMIT_BYTES: usize = 32 * 1_024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PublicPollKind {
    Decision,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePollOptionRequest {
    label: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreatePollRequest {
    kind: PublicPollKind,
    title: String,
    description: String,
    options: Vec<CreatePollOptionRequest>,
    closes_at: String,
    allow_multi: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VoteRequest {
    option_ids: Vec<String>,
}

pub async fn list_polls(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path(trip_id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Vec<Poll>>, ApiError> {
    require_empty_body(body)?;
    Ok(Json(
        polls::list_polls(&*state.polls, &trip_id, &actor.id).await?,
    ))
}

pub async fn create_poll(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path(trip_id): Path<String>,
    payload: Result<Json<CreatePollRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Poll>), ApiError> {
    let Json(request) = payload?;
    let PublicPollKind::Decision = request.kind;
    let poll = polls::create_poll(
        &*state.polls,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &actor.id,
        CreatePollInput {
            title: request.title,
            description: request.description,
            options: request
                .options
                .into_iter()
                .map(|option| option.label)
                .collect(),
            closes_at: request.closes_at,
            allow_multi: request.allow_multi,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(poll)))
}

pub async fn open_poll(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path((trip_id, poll_id)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Poll>, ApiError> {
    require_empty_body(body)?;
    Ok(Json(
        polls::open_poll(&*state.polls, &*state.clock, &trip_id, &actor.id, &poll_id).await?,
    ))
}

pub async fn vote(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path((trip_id, poll_id)): Path<(String, String)>,
    payload: Result<Json<VoteRequest>, JsonRejection>,
) -> Result<Json<Poll>, ApiError> {
    let Json(request) = payload?;
    Ok(Json(
        polls::vote(
            &*state.polls,
            &*state.clock,
            &trip_id,
            &actor.id,
            &poll_id,
            request.option_ids,
        )
        .await?,
    ))
}

pub async fn close_poll(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path((trip_id, poll_id)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Poll>, ApiError> {
    require_empty_body(body)?;
    Ok(Json(
        polls::close_poll(
            &*state.polls,
            &*state.id_gen,
            &*state.clock,
            &trip_id,
            &actor.id,
            &poll_id,
        )
        .await?,
    ))
}
