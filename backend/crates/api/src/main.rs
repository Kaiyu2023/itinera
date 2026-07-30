use axum::Router;
use itinera_adapters::{
    insecure::identity::DevIdentityProvider, memory::user_repo::InMemoryUserRepo,
    uuid_ids::UuidIdGen,
};
use itinera_api::{create_app, state::AppState};
use std::{error::Error, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = create_app(create_app_state()?);

    if is_in_lambda_environment() {
        serve_in_lambda(app).await?;
    } else {
        serve_with_tcp_listener(app).await?;
    };
    Ok(())
}

fn is_in_lambda_environment() -> bool {
    std::env::var("AWS_LAMBDA_RUNTIME_API").is_ok()
}

fn is_dev_auth_enabled() -> bool {
    std::env::var("ITINERA_DEV_AUTH_ENABLED").is_ok_and(|v| v == "1")
}

async fn serve_in_lambda(app: Router) -> Result<(), Box<dyn Error + Send + Sync>> {
    println!("Starting Axum Lambda server...");
    lambda_http::run(app).await
}

async fn serve_with_tcp_listener(app: Router) -> Result<(), Box<dyn Error + Send + Sync>> {
    println!("Starting Axum TCP server...");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    println!("Listener started on 127.0.0.1:3000");
    axum::serve(listener, app).await?;
    Ok(())
}

fn create_app_state() -> Result<AppState, Box<dyn Error + Send + Sync>> {
    if !is_dev_auth_enabled() {
        return Err(
            "no identity provider configured — set ITINERA_DEV_AUTH_ENABLED=1 for local \
                development; the Cloudflare Access adapter arrives in the next slice"
                .into(),
        );
    }
    Ok(AppState {
        identity: Arc::new(DevIdentityProvider),
        users: Arc::new(InMemoryUserRepo::new()),
        id_gen: Arc::new(UuidIdGen),
    })
}
