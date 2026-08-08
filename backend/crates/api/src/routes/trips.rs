use axum::{
    Json,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
};
use itinera_core::{
    domain::{
        trip::{Invite, Trip, TripStatus, TripSummary},
        user::UserId,
    },
    services::trips::{self, CreateTripInput},
};
use serde::Deserialize;

use crate::{
    auth::AuthenticatedPrincipal, error::ApiError, routes::me::UserResponse, state::AppState,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateTripRequest {
    name: String,
    start_date: String,
    end_date: String,
    base_currency: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TripStatusRequest {
    status: TripStatus,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InviteRequest {
    email: String,
}

pub async fn list_trips(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
) -> Result<Json<Vec<TripSummary>>, ApiError> {
    let (authorization, scoped_trip_ids) = principal.require_trip_list()?;
    let mut trips = trips::list_trips(&*state.trips, &authorization).await?;
    if let Some(scoped_trip_ids) = scoped_trip_ids {
        trips.retain(|trip| scoped_trip_ids.iter().any(|trip_id| trip_id == &trip.id));
    }
    Ok(Json(trips))
}

pub async fn create_trip(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    payload: Result<Json<CreateTripRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Trip>), ApiError> {
    let authorization = principal.require_human_trip()?;
    let Json(request) = payload?;
    let trip = trips::create_trip(
        &*state.trips,
        &*state.id_gen,
        &*state.clock,
        &authorization,
        CreateTripInput {
            name: request.name,
            start_date: request.start_date,
            end_date: request.end_date,
            base_currency: request.base_currency,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(trip)))
}

pub async fn get_trip(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
) -> Result<Json<Trip>, ApiError> {
    let authorization = principal.require_trip_read(&trip_id)?;
    Ok(Json(
        trips::get_trip(&*state.trips, &trip_id, &authorization).await?,
    ))
}

pub async fn set_trip_status(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
    payload: Result<Json<TripStatusRequest>, JsonRejection>,
) -> Result<Json<Trip>, ApiError> {
    let authorization = principal.require_human_trip()?;
    let Json(request) = payload?;
    Ok(Json(
        trips::set_trip_status(
            &*state.trips,
            &*state.id_gen,
            &*state.clock,
            &trip_id,
            &authorization,
            request.status,
        )
        .await?,
    ))
}

pub async fn get_users(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
) -> Result<Json<Vec<UserResponse>>, ApiError> {
    let authorization = principal.require_trip_read(&trip_id)?;
    let users = trips::get_members(&*state.trips, &trip_id, &authorization)
        .await?
        .into_iter()
        .map(UserResponse::from)
        .collect();
    Ok(Json(users))
}

pub async fn invite(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
    payload: Result<Json<InviteRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Invite>), ApiError> {
    let authorization = principal.require_human_trip()?;
    let Json(request) = payload?;
    let invite = trips::invite(
        &*state.trips,
        &*state.access_policy,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &authorization,
        &request.email,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(invite)))
}

pub async fn remove_member(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, user_id)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let authorization = principal.require_human_trip()?;
    trips::remove_member(&*state.trips, &trip_id, &authorization, &UserId(user_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}
