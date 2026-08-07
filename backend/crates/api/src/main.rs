use axum::Router;
#[cfg(feature = "dev-auth")]
use itinera_adapters::insecure::{
    external::{DevAccessPolicy, EmptyPlaceCatalog},
    identity::DevIdentityProvider,
};
use itinera_adapters::{
    clock::SystemClock,
    cloudflare_access::{CloudflareAccessBuildError, CloudflareAccessIdentityProvider},
    dynamodb::{DynamoDb, DynamoUserRepoBuildError},
    frankfurter::{FrankfurterBuildError, FrankfurterFxRateProvider},
    unavailable::{UnavailableAccessPolicy, UnavailablePlaceCatalog},
    uuid_ids::UuidIdGen,
};
use itinera_api::{create_app, state::AppState};
use itinera_core::ports::{
    access_policy::AccessPolicy, auth::IdentityProvider, content_history::ContentHistoryRepo,
    discussion::DiscussionRepo, fx_rate::FxRateProvider, ledger::LedgerRepo, notice::NoticeRepo,
    place_catalog::PlaceCatalog, poll::PollRepo, proposal::ProposalRepo,
    service_identity::ServiceIdentityRepo, trip::TripRepo, user::UserRepo,
};
use std::{error::Error, sync::Arc};
use thiserror::Error;

const TEAM_DOMAIN_ENV: &str = "ITINERA_CF_ACCESS_TEAM_DOMAIN";
const AUDIENCE_ENV: &str = "ITINERA_CF_ACCESS_AUDIENCE";
const DYNAMODB_TABLE_ENV: &str = "ITINERA_DYNAMODB_TABLE";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let app = create_app(create_app_state().await?);

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

async fn create_app_state() -> Result<AppState, StartupError> {
    let dev_auth_enabled = is_dev_auth_enabled();
    let identity = create_identity_provider(dev_auth_enabled)?;
    let storage = create_storage().await?;
    let integrations = create_integrations(dev_auth_enabled)?;
    Ok(AppState {
        identity,
        users: storage.users,
        trips: storage.trips,
        content_history: storage.content_history,
        proposals: storage.proposals,
        polls: storage.polls,
        discussions: storage.discussions,
        ledger: storage.ledger,
        notices: storage.notices,
        service_identities: storage.service_identities,
        access_policy: integrations.access_policy,
        place_catalog: integrations.place_catalog,
        fx_rates: integrations.fx_rates,
        id_gen: Arc::new(UuidIdGen),
        clock: Arc::new(SystemClock),
    })
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

struct Storage {
    users: Arc<dyn UserRepo>,
    trips: Arc<dyn TripRepo>,
    content_history: Arc<dyn ContentHistoryRepo>,
    proposals: Arc<dyn ProposalRepo>,
    polls: Arc<dyn PollRepo>,
    discussions: Arc<dyn DiscussionRepo>,
    ledger: Arc<dyn LedgerRepo>,
    notices: Arc<dyn NoticeRepo>,
    service_identities: Arc<dyn ServiceIdentityRepo>,
}

async fn create_storage() -> Result<Storage, StartupError> {
    let table_name = dynamodb_table_from_config(std::env::var(DYNAMODB_TABLE_ENV).ok())?;
    let database = Arc::new(DynamoDb::from_environment(table_name).await?);
    Ok(Storage {
        users: database.clone(),
        trips: database.clone(),
        content_history: database.clone(),
        proposals: database.clone(),
        polls: database.clone(),
        discussions: database.clone(),
        ledger: database.clone(),
        notices: database.clone(),
        service_identities: database,
    })
}

struct Integrations {
    access_policy: Arc<dyn AccessPolicy>,
    place_catalog: Arc<dyn PlaceCatalog>,
    fx_rates: Arc<dyn FxRateProvider>,
}

fn create_integrations(dev_auth_enabled: bool) -> Result<Integrations, StartupError> {
    #[cfg(feature = "dev-auth")]
    if dev_auth_enabled {
        return Ok(Integrations {
            access_policy: Arc::new(DevAccessPolicy),
            place_catalog: Arc::new(EmptyPlaceCatalog),
            fx_rates: Arc::new(FrankfurterFxRateProvider::new()?),
        });
    }

    let _ = dev_auth_enabled;
    Ok(Integrations {
        // Phase B step 4 replaces these with credentialled provider adapters.
        // Until then, external side effects fail closed rather than pretending
        // an invite or catalog lookup succeeded.
        access_policy: Arc::new(UnavailableAccessPolicy),
        place_catalog: Arc::new(UnavailablePlaceCatalog),
        fx_rates: Arc::new(FrankfurterFxRateProvider::new()?),
    })
}

fn dynamodb_table_from_config(table_name: Option<String>) -> Result<String, StartupError> {
    required_environment(DYNAMODB_TABLE_ENV, table_name)
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
    #[error("{0} must be set")]
    MissingEnvironment(&'static str),
    #[error(transparent)]
    CloudflareAccess(#[from] CloudflareAccessBuildError),
    #[error(transparent)]
    DynamoDb(#[from] DynamoUserRepoBuildError),
    #[error(transparent)]
    Frankfurter(#[from] FrankfurterBuildError),
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
            dynamodb_table_from_config(None),
            Err(StartupError::MissingEnvironment(DYNAMODB_TABLE_ENV))
        ));
        assert_eq!(
            dynamodb_table_from_config(Some("itinera-prod".to_owned()))
                .expect("valid storage config"),
            "itinera-prod"
        );
    }

    #[cfg(feature = "dev-auth")]
    #[test]
    fn development_auth_bypasses_cloudflare_but_not_persistent_storage() {
        assert!(identity_provider_from_config(true, None, None).is_ok());
        assert!(matches!(
            dynamodb_table_from_config(None),
            Err(StartupError::MissingEnvironment(DYNAMODB_TABLE_ENV))
        ));
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
