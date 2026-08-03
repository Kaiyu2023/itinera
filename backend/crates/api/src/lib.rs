use axum::{Json, Router, middleware, routing::get};
use serde_json::json;

use crate::{origin::OriginGuard, routes::me::get_me, state::AppState};

pub mod auth;
pub mod error;
pub mod origin;
pub mod routes;
pub mod state;

pub fn create_app(state: AppState, origin_guard: OriginGuard) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/me", get(get_me))
        .with_state(state)
        // This is the outermost application middleware. A direct-origin
        // request is rejected before Axum extracts credentials or a body.
        .layer(middleware::from_fn_with_state(
            origin_guard,
            origin::require_verified_origin,
        ))
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}
