use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection},
    http::StatusCode,
};
use itinera_core::{domain::content_history::Edit, services::content_history};

use crate::{auth::AuthenticatedPrincipal, error::ApiError, state::AppState};

pub const REVERT_BODY_LIMIT_BYTES: usize = 1_024;

pub async fn get_history(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path(trip_id): Path<String>,
) -> Result<Json<Vec<Edit>>, ApiError> {
    let authorization = principal.require_trip_read(&trip_id)?;
    Ok(Json(
        content_history::get_history(&*state.content_history, &trip_id, &authorization).await?,
    ))
}

pub async fn revert_edit(
    State(state): State<AppState>,
    principal: AuthenticatedPrincipal,
    Path((trip_id, edit_id)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, ApiError> {
    let authorization = principal.require_human_trip()?;
    let body = body.map_err(|_| ApiError::payload_too_large())?;
    if !body.is_empty() {
        return Err(ApiError::bad_request(
            "A revert request must not contain a body; the editId selects the server-owned change.",
        ));
    }
    content_history::revert_edit(
        &*state.content_history,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &authorization,
        &edit_id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
