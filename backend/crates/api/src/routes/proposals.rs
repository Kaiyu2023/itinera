use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
};
use itinera_core::{
    domain::proposal::{ChangeSet, Proposal, ProposalRoute},
    services::proposals::{self, CreateProposalInput},
};
use serde::Deserialize;

use crate::{auth::AuthenticatedUser, error::ApiError, state::AppState};

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
    AuthenticatedUser(actor): AuthenticatedUser,
    Path(trip_id): Path<String>,
) -> Result<Json<Vec<Proposal>>, ApiError> {
    Ok(Json(
        proposals::list_proposals(&*state.proposals, &trip_id, &actor.id).await?,
    ))
}

pub async fn create_proposal(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path(trip_id): Path<String>,
    payload: Result<Json<CreateProposalRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Proposal>), ApiError> {
    let Json(request) = payload?;
    let proposal = proposals::create_proposal(
        &*state.proposals,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &actor.id,
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

pub async fn approve_proposal(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path((trip_id, proposal_id)): Path<(String, String)>,
) -> Result<Json<Proposal>, ApiError> {
    Ok(Json(
        proposals::approve_proposal(
            &*state.proposals,
            &*state.id_gen,
            &*state.clock,
            &trip_id,
            &actor.id,
            &proposal_id,
        )
        .await?,
    ))
}

pub async fn reject_proposal(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path((trip_id, proposal_id)): Path<(String, String)>,
    payload: Result<Json<RejectProposalRequest>, JsonRejection>,
) -> Result<Json<Proposal>, ApiError> {
    let Json(request) = payload?;
    Ok(Json(
        proposals::reject_proposal(
            &*state.proposals,
            &trip_id,
            &actor.id,
            &proposal_id,
            request.reason,
        )
        .await?,
    ))
}
