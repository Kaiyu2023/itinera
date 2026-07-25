use axum::{Json, Router, routing::get};
use serde_json::json;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/healthz", get(healthz));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("Unable to start TCP listener.");
    println!("Listener started on 127.0.0.1:3000");
    axum::serve(listener, app)
        .await
        .expect("Unable to start Axum server.");
}

async fn healthz() -> Json<serde_json::Value> {
    Json(json!({"status": "ok"}))
}
