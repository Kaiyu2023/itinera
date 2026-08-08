use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection, rejection::JsonRejection},
    http::StatusCode,
};
use itinera_core::{
    domain::poll::Poll,
    domain::proposal::{ChangeSet, Proposal, ProposalRoute},
    services::{
        polls,
        proposals::{self, CreateProposalInput},
    },
};
use serde::Deserialize;

use crate::{
    auth::AuthenticatedPrincipal, error::ApiError, routes::require_empty_body, state::AppState,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateProposalRequest {
    title: String,
    rationale: String,
    change_set: ChangeSet,
    route: ProposalRoute,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectProposalRequest {
    reason: String,
}

pub async fn list_proposals(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
) -> Result<Json<Vec<Proposal>>, ApiError> {
    let authorization = principal.require_trip_read(&trip_id)?;
    Ok(Json(
        proposals::list_proposals(&*state.proposals, &trip_id, &authorization).await?,
    ))
}

pub async fn create_proposal(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
    payload: Result<Json<CreateProposalRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Proposal>), ApiError> {
    let authorization = principal.require_human_trip()?;
    let Json(request) = payload?;
    let proposal = proposals::create_proposal(
        &*state.proposals,
        &*state.polls,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &authorization,
        CreateProposalInput {
            title: request.title,
            rationale: request.rationale,
            change_set: request.change_set,
            route: request.route,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(proposal)))
}

pub async fn proposal_to_poll(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, proposal_id)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<(StatusCode, Json<Poll>), ApiError> {
    let authorization = principal.require_human_trip()?;
    require_empty_body(body)?;
    let poll = polls::proposal_to_poll(
        &*state.polls,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &authorization,
        &proposal_id,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(poll)))
}

pub async fn approve_proposal(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, proposal_id)): Path<(String, String)>,
) -> Result<Json<Proposal>, ApiError> {
    let authorization = principal.require_human_trip()?;
    Ok(Json(
        proposals::approve_proposal(
            &*state.proposals,
            &*state.id_gen,
            &*state.clock,
            &trip_id,
            &authorization,
            &proposal_id,
        )
        .await?,
    ))
}

pub async fn reject_proposal(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, proposal_id)): Path<(String, String)>,
    payload: Result<Json<RejectProposalRequest>, JsonRejection>,
) -> Result<Json<Proposal>, ApiError> {
    let authorization = principal.require_human_trip()?;
    let Json(request) = payload?;
    Ok(Json(
        proposals::reject_proposal(
            &*state.proposals,
            &trip_id,
            &authorization,
            &proposal_id,
            request.reason,
        )
        .await?,
    ))
}
