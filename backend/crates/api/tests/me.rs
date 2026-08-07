//! End-to-end tests for `GET /me`, driven through the real router.
//!
//! These go through `create_app`, so they exercise routing, the extractor, the
//! identity provider, provisioning and the DTO together — the seams the unit
//! tests deliberately cut. No socket is involved: `oneshot` calls the router as
//! the `tower::Service` it is, which is what makes them fast enough to keep.

mod support;

use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use itinera_adapters::{
    clock::SystemClock,
    insecure::external::{DevAccessPolicy, EmptyPlaceCatalog},
    uuid_ids::UuidIdGen,
};
use itinera_api::{create_app, state::AppState};
use itinera_core::{
    domain::user::{Email, User},
    ports::{
        auth::{AuthError, Identity, IdentityProvider},
        user::{UserRepo, UserRepoError},
    },
};
use serde_json::Value;
use tower::ServiceExt;

use support::trip_repo::TestTripRepo;

/// Spelled out rather than imported so that renaming the constant in `auth.rs`
/// cannot silently move the wire contract with it.
const ASSERTION_HEADER: &str = "cf-access-jwt-assertion";
const EMAIL: &str = "cloud.strife@proton.me";

fn app() -> Router {
    app_with_identity(Arc::new(EmailIdentityProvider))
}

fn app_with_identity(identity: Arc<dyn IdentityProvider>) -> Router {
    let trips = Arc::new(TestTripRepo::new());
    create_app(AppState {
        identity,
        users: Arc::new(TestUserRepo::default()),
        trips: trips.clone(),
        content_history: trips.clone(),
        proposals: trips.clone(),
        polls: trips.clone(),
        discussions: trips,
        access_policy: Arc::new(DevAccessPolicy),
        place_catalog: Arc::new(EmptyPlaceCatalog),
        id_gen: Arc::new(UuidIdGen),
        clock: Arc::new(SystemClock),
    })
}

/// A route-test fake kept in the test target itself. Runtime builds expose no
/// alternate in-memory persistence implementation.
#[derive(Default)]
struct TestUserRepo {
    users: RwLock<HashMap<Email, User>>,
}

#[async_trait]
impl UserRepo for TestUserRepo {
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, UserRepoError> {
        Ok(self
            .users
            .read()
            .map_err(|_| UserRepoError::UserRepoUnavailable)?
            .get(email)
            .cloned())
    }

    async fn find_by_id(
        &self,
        user_id: &itinera_core::domain::user::UserId,
    ) -> Result<Option<User>, UserRepoError> {
        Ok(self
            .users
            .read()
            .map_err(|_| UserRepoError::UserRepoUnavailable)?
            .values()
            .find(|user| &user.id == user_id)
            .cloned())
    }

    async fn insert(&self, user: User) -> Result<(), UserRepoError> {
        let mut users = self
            .users
            .write()
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;
        if users.contains_key(&user.email) {
            return Err(UserRepoError::DuplicateEmail(user.email));
        }
        users.insert(user.email.clone(), user);
        Ok(())
    }
}

struct RejectingIdentityProvider(AuthError);

struct EmailIdentityProvider;

#[async_trait]
impl IdentityProvider for EmailIdentityProvider {
    async fn authenticate(&self, assertion: &str) -> Result<Identity, AuthError> {
        let email = Email::parse(assertion).map_err(|_| AuthError::InvalidToken)?;
        Ok(Identity { email })
    }
}

#[async_trait]
impl IdentityProvider for RejectingIdentityProvider {
    async fn authenticate(&self, _assertion: &str) -> Result<Identity, AuthError> {
        Err(self.0.clone())
    }
}

async fn get_me(app: &Router, assertion: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder().uri("/me");
    if let Some(assertion) = assertion {
        request = request.header(ASSERTION_HEADER, assertion);
    }
    let request = request.body(Body::empty()).expect("should build a request");

    // `oneshot` consumes the service, so each call takes a clone; the state
    // inside is an `Arc`, so every clone shares one user store.
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("the router is infallible");

    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("should read the body");

    (
        status,
        serde_json::from_slice(&body).expect("should be JSON"),
    )
}

#[tokio::test]
async fn a_request_without_credentials_is_rejected() {
    let (status, body) = get_me(&app(), None).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "missing_credentials");
    assert!(
        body["message"].is_string(),
        "the contract requires both `error` and `message`"
    );
}

#[tokio::test]
async fn a_credential_the_provider_rejects_is_unauthorized() {
    let (status, body) = get_me(&app(), Some("not-an-email")).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        body["error"], "invalid_token",
        "a rejected credential must be distinguishable from an absent one"
    );
}

#[tokio::test]
async fn an_expired_credential_has_a_distinct_unauthorized_response() {
    let app = app_with_identity(Arc::new(RejectingIdentityProvider(AuthError::ExpiredToken)));
    let (status, body) = get_me(&app, Some("expired-assertion")).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["error"], "expired_token");
}

#[tokio::test]
async fn a_jwks_outage_is_reported_as_service_unavailable() {
    let app = app_with_identity(Arc::new(RejectingIdentityProvider(
        AuthError::IdentityProviderUnavailable,
    )));
    let (status, body) = get_me(&app, Some("otherwise-valid-assertion")).await;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"], "identity_provider_unavailable");
}

#[tokio::test]
async fn an_authenticated_request_returns_the_whole_user_contract() {
    let (status, body) = get_me(&app(), Some(EMAIL)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["email"], EMAIL);
    assert_eq!(body["displayName"], "cloud.strife");
    assert!(
        body["id"].as_str().is_some_and(|id| !id.is_empty()),
        "the user should have been provisioned with a generated id"
    );
    assert!(
        body["avatarColor"]
            .as_str()
            .is_some_and(|colour| colour.starts_with('#')),
        "avatarColor should be a hex colour, got {}",
        body["avatarColor"]
    );
}

#[tokio::test]
async fn a_returning_user_keeps_their_identity() {
    // The second request must find the record the first one provisioned rather
    // than mint a new user — the whole point of get-or-provision.
    let app = app();

    let (_, first) = get_me(&app, Some(EMAIL)).await;
    let (status, second) = get_me(&app, Some(EMAIL)).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["id"], second["id"]);
    assert_eq!(first["avatarColor"], second["avatarColor"]);
}

#[tokio::test]
async fn one_address_spelled_two_ways_is_one_account() {
    // `Email::parse` canonicalises, so a caller whose header differs only in
    // case or padding must land on the account they already have.
    let app = app();

    let (_, canonical) = get_me(&app, Some(EMAIL)).await;
    let (status, shouted) = get_me(&app, Some("  Cloud.Strife@Proton.ME  ")).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        canonical["id"], shouted["id"],
        "case and padding must not create a second account"
    );
    assert_eq!(shouted["email"], EMAIL, "the response is canonicalised");
}

#[tokio::test]
async fn different_people_get_different_accounts() {
    let app = app();

    let (_, cloud) = get_me(&app, Some(EMAIL)).await;
    let (_, tifa) = get_me(&app, Some("tifa.lockhart@proton.me")).await;

    assert_ne!(cloud["id"], tifa["id"]);
    assert_eq!(tifa["displayName"], "tifa.lockhart");
}
