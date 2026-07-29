use axum::{Json, Router, routing::get};
use serde_json::json;

pub fn create_app() -> Router {
    Router::new().route("/healthz", get(healthz))
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}
