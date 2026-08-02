use axum::Router;
#[cfg(feature = "dev-auth")]
use itinera_adapters::insecure::identity::DevIdentityProvider;
use itinera_adapters::{
    cloudflare_access::{CloudflareAccessBuildError, CloudflareAccessIdentityProvider},
    memory::user_repo::InMemoryUserRepo,
    uuid_ids::UuidIdGen,
};
use itinera_api::{create_app, state::AppState};
use itinera_core::ports::auth::IdentityProvider;
use std::{error::Error, sync::Arc};
use thiserror::Error;

const TEAM_DOMAIN_ENV: &str = "ITINERA_CF_ACCESS_TEAM_DOMAIN";
const AUDIENCE_ENV: &str = "ITINERA_CF_ACCESS_AUDIENCE";

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

fn create_app_state() -> Result<AppState, StartupError> {
    Ok(AppState {
        identity: create_identity_provider()?,
        users: Arc::new(InMemoryUserRepo::new()),
        id_gen: Arc::new(UuidIdGen),
    })
}

fn create_identity_provider() -> Result<Arc<dyn IdentityProvider>, StartupError> {
    identity_provider_from_config(
        is_dev_auth_enabled(),
        std::env::var(TEAM_DOMAIN_ENV).ok(),
        std::env::var(AUDIENCE_ENV).ok(),
    )
}

fn identity_provider_from_config(
    dev_auth_enabled: bool,
    team_domain: Option<String>,
    audience: Option<String>,
) -> Result<Arc<dyn IdentityProvider>, StartupError> {
    if dev_auth_enabled {
        #[cfg(feature = "dev-auth")]
        return Ok(Arc::new(DevIdentityProvider));

        #[cfg(not(feature = "dev-auth"))]
        return Err(StartupError::DevAuthNotCompiled);
    }

    let team_domain = required_environment(TEAM_DOMAIN_ENV, team_domain)?;
    let audience = required_environment(AUDIENCE_ENV, audience)?;
    Ok(Arc::new(CloudflareAccessIdentityProvider::new(
        &team_domain,
        &audience,
    )?))
}

fn required_environment(name: &'static str, value: Option<String>) -> Result<String, StartupError> {
    value
        .filter(|value| !value.trim().is_empty())
        .ok_or(StartupError::MissingEnvironment(name))
}

#[derive(Debug, Error)]
enum StartupError {
    #[cfg(not(feature = "dev-auth"))]
    #[error(
        "ITINERA_DEV_AUTH_ENABLED=1 requires a build compiled with the `dev-auth` Cargo feature"
    )]
    DevAuthNotCompiled,
    #[error("{0} must be set when ITINERA_DEV_AUTH_ENABLED is not 1")]
    MissingEnvironment(&'static str),
    #[error(transparent)]
    CloudflareAccess(#[from] CloudflareAccessBuildError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_auth_requires_the_team_domain_and_audience() {
        assert!(matches!(
            identity_provider_from_config(false, None, None),
            Err(StartupError::MissingEnvironment(TEAM_DOMAIN_ENV))
        ));
        assert!(matches!(
            identity_provider_from_config(
                false,
                Some("https://itinera-test.cloudflareaccess.com".to_owned()),
                None,
            ),
            Err(StartupError::MissingEnvironment(AUDIENCE_ENV))
        ));
    }

    #[cfg(feature = "dev-auth")]
    #[test]
    fn development_auth_requires_the_explicit_switch_but_no_cloudflare_config() {
        assert!(identity_provider_from_config(true, None, None).is_ok());
    }

    #[cfg(not(feature = "dev-auth"))]
    #[test]
    fn production_builds_reject_the_development_auth_switch() {
        assert!(matches!(
            identity_provider_from_config(true, None, None),
            Err(StartupError::DevAuthNotCompiled)
        ));
    }

    #[test]
    fn valid_production_auth_configuration_builds_the_cloudflare_provider() {
        assert!(
            identity_provider_from_config(
                false,
                Some("https://itinera-test.cloudflareaccess.com".to_owned()),
                Some("itinera-audience".to_owned()),
            )
            .is_ok()
        );
    }
}
