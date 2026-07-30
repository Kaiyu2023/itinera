use axum::{Json, Router, routing::get};
use serde_json::json;

use crate::state::AppState;

pub mod error;
pub mod state;

pub fn create_app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .with_state(state)
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}
