use axum::Router;
#[cfg(feature = "dev-auth")]
use itinera_adapters::insecure::identity::DevIdentityProvider;
#[cfg(feature = "dev-auth")]
use itinera_adapters::memory::user_repo::InMemoryUserRepo;
use itinera_adapters::{
    cloudflare_access::{CloudflareAccessBuildError, CloudflareAccessIdentityProvider},
    dynamodb::{DynamoUserRepo, DynamoUserRepoBuildError},
    uuid_ids::UuidIdGen,
};
use itinera_api::{
    create_app,
    origin::{OriginGuard, OriginGuardBuildError},
    state::AppState,
};
use itinera_core::ports::{auth::IdentityProvider, user::UserRepo};
#[cfg(feature = "dev-auth")]
use sha2::Digest;
use std::{error::Error, sync::Arc};
use thiserror::Error;

const TEAM_DOMAIN_ENV: &str = "ITINERA_CF_ACCESS_TEAM_DOMAIN";
const AUDIENCE_ENV: &str = "ITINERA_CF_ACCESS_AUDIENCE";
const DYNAMODB_TABLE_ENV: &str = "ITINERA_DYNAMODB_TABLE";
const ORIGIN_SECRET_HASHES_ENV: &str = "ITINERA_ORIGIN_SECRET_SHA256_HASHES";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let dev_auth_enabled = is_dev_auth_enabled();
    let origin_guard = create_origin_guard(dev_auth_enabled)?;
    let state = create_app_state(dev_auth_enabled).await?;
    let app = create_app(state, origin_guard);

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

async fn create_app_state(dev_auth_enabled: bool) -> Result<AppState, StartupError> {
    Ok(AppState {
        identity: create_identity_provider(dev_auth_enabled)?,
        users: create_user_repo(dev_auth_enabled).await?,
        id_gen: Arc::new(UuidIdGen),
    })
}

fn create_origin_guard(dev_auth_enabled: bool) -> Result<OriginGuard, StartupError> {
    origin_guard_from_config(
        dev_auth_enabled,
        std::env::var(ORIGIN_SECRET_HASHES_ENV).ok(),
    )
}

fn origin_guard_from_config(
    dev_auth_enabled: bool,
    hashes: Option<String>,
) -> Result<OriginGuard, StartupError> {
    if dev_auth_enabled {
        #[cfg(feature = "dev-auth")]
        return OriginGuard::from_sha256_hashes(&format!(
            "{:x}",
            sha2::Sha256::digest(b"itinera-local-development-origin-secret")
        ))
        .map_err(Into::into);

        #[cfg(not(feature = "dev-auth"))]
        return Err(StartupError::DevAuthNotCompiled);
    }

    let hashes = required_environment(ORIGIN_SECRET_HASHES_ENV, hashes)?;
    Ok(OriginGuard::from_sha256_hashes(&hashes)?)
}

fn create_identity_provider(
    dev_auth_enabled: bool,
) -> Result<Arc<dyn IdentityProvider>, StartupError> {
    identity_provider_from_config(
        dev_auth_enabled,
        std::env::var(TEAM_DOMAIN_ENV).ok(),
        std::env::var(AUDIENCE_ENV).ok(),
    )
}

async fn create_user_repo(dev_auth_enabled: bool) -> Result<Arc<dyn UserRepo>, StartupError> {
    match user_store_from_config(dev_auth_enabled, std::env::var(DYNAMODB_TABLE_ENV).ok())? {
        #[cfg(feature = "dev-auth")]
        UserStoreConfig::InMemory => Ok(Arc::new(InMemoryUserRepo::new())),
        UserStoreConfig::DynamoDb(table_name) => Ok(Arc::new(
            DynamoUserRepo::from_environment(table_name).await?,
        )),
    }
}

#[derive(Debug, Eq, PartialEq)]
enum UserStoreConfig {
    #[cfg(feature = "dev-auth")]
    InMemory,
    DynamoDb(String),
}

fn user_store_from_config(
    dev_auth_enabled: bool,
    table_name: Option<String>,
) -> Result<UserStoreConfig, StartupError> {
    if dev_auth_enabled {
        #[cfg(feature = "dev-auth")]
        return Ok(UserStoreConfig::InMemory);

        #[cfg(not(feature = "dev-auth"))]
        return Err(StartupError::DevAuthNotCompiled);
    }

    Ok(UserStoreConfig::DynamoDb(required_environment(
        DYNAMODB_TABLE_ENV,
        table_name,
    )?))
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
    #[error(transparent)]
    DynamoDb(#[from] DynamoUserRepoBuildError),
    #[error(transparent)]
    OriginGuard(#[from] OriginGuardBuildError),
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

    #[test]
    fn production_storage_requires_a_dynamodb_table() {
        assert!(matches!(
            user_store_from_config(false, None),
            Err(StartupError::MissingEnvironment(DYNAMODB_TABLE_ENV))
        ));
        assert_eq!(
            user_store_from_config(false, Some("itinera-prod".to_owned()))
                .expect("valid storage config"),
            UserStoreConfig::DynamoDb("itinera-prod".to_owned())
        );
    }

    #[test]
    fn production_requires_a_valid_origin_secret_hash() {
        assert!(matches!(
            origin_guard_from_config(false, None),
            Err(StartupError::MissingEnvironment(ORIGIN_SECRET_HASHES_ENV))
        ));
        assert!(matches!(
            origin_guard_from_config(false, Some("not-a-hash".to_owned())),
            Err(StartupError::OriginGuard(
                OriginGuardBuildError::InvalidHash
            ))
        ));
        assert!(
            origin_guard_from_config(false, Some("a".repeat(64))).is_ok(),
            "a canonical digest should build the production guard"
        );
    }

    #[cfg(feature = "dev-auth")]
    #[test]
    fn development_auth_requires_the_explicit_switch_but_no_cloudflare_config() {
        assert!(identity_provider_from_config(true, None, None).is_ok());
        assert_eq!(
            user_store_from_config(true, None).expect("development storage"),
            UserStoreConfig::InMemory
        );
    }

    #[cfg(not(feature = "dev-auth"))]
    #[test]
    fn production_builds_reject_the_development_auth_switch() {
        assert!(matches!(
            identity_provider_from_config(true, None, None),
            Err(StartupError::DevAuthNotCompiled)
        ));
        assert!(matches!(
            user_store_from_config(true, None),
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
