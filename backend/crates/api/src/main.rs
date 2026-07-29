use axum::Router;
use itinera_api::create_app;
use std::error::Error;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = create_app();

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
