use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection, rejection::JsonRejection},
    http::StatusCode,
};
use itinera_core::{
    domain::service_identity::{ServiceIdentity, ServiceScope},
    services::service_identities::{self, RegisterServiceIdentityInput},
};
use serde::Deserialize;

use crate::{
    auth::AuthenticatedPrincipal, error::ApiError, routes::require_empty_body, state::AppState,
};

pub const SERVICE_IDENTITY_BODYLESS_LIMIT_BYTES: usize = 1_024;
pub const SERVICE_IDENTITY_WRITE_LIMIT_BYTES: usize = 16 * 1_024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterServiceIdentityRequest {
    name: String,
    client_id: String,
    scopes: Vec<ServiceScope>,
    trip_ids: Vec<String>,
    ttl_hours: u16,
}

pub async fn list_service_identities(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<Vec<ServiceIdentity>>, ApiError> {
    require_empty_body(body)?;
    let owner = principal.require_human()?;
    Ok(Json(
        service_identities::list_service_identities(&*state.service_identities, &owner.id).await?,
    ))
}

pub async fn register_service_identity(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    payload: Result<Json<RegisterServiceIdentityRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ServiceIdentity>), ApiError> {
    let owner = principal.require_human()?;
    let Json(request) = payload?;
    let identity = service_identities::register_service_identity(
        &*state.service_identities,
        &*state.id_gen,
        &*state.clock,
        &owner.id,
        RegisterServiceIdentityInput {
            name: request.name,
            client_id: request.client_id,
            scopes: request.scopes,
            trip_ids: request.trip_ids,
            ttl_hours: request.ttl_hours,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(identity)))
}

pub async fn revoke_service_identity(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(service_identity_id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, ApiError> {
    require_empty_body(body)?;
    let owner = principal.require_human()?;
    service_identities::revoke_service_identity(
        &*state.service_identities,
        &*state.clock,
        &owner.id,
        &service_identity_id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
