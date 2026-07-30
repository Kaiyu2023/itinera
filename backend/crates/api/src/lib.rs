use axum::{Json, Router, routing::get};
use serde_json::json;

use crate::{routes::me::get_me, state::AppState};

pub mod auth;
pub mod error;
pub mod routes;
pub mod state;

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/me", get(get_me))
        .with_state(state)
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}
