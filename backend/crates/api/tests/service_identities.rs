mod support;
#[path = "support/user_repo.rs"]
mod user_repo;

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use itinera_adapters::insecure::external::{DevAccessPolicy, EmptyPlaceCatalog};
use itinera_api::{create_app, state::AppState};
use itinera_core::{
    domain::{
        trip::{DateRange, Trip, TripData, TripMember, TripRole, TripStatus},
        user::{Email, User, UserId},
    },
    ports::{
        auth::{AuthError, Identity, IdentityProvider, ServiceCommonName},
        clock::Clock,
        id_gen::IdGen,
        trip::TripRepo,
        user::UserRepo,
    },
    services::service_identities::SERVICE_REQUESTS_PER_HOUR,
};
use serde_json::{Value, json};
use tower::ServiceExt;

use support::{
    FixedFxRateProvider, service_identity_repo::TestServiceIdentityRepo, trip_repo::TestTripRepo,
};
use user_repo::TestUserRepo;

const ASSERTION_HEADER: &str = "cf-access-jwt-assertion";
const NOW: &str = "2026-08-06T12:00:00Z";
const OWNER_EMAIL: &str = "owner@example.test";
const OTHER_EMAIL: &str = "other@example.test";
const CLIENT_ID: &str = "1234567890abcdef1234567890abcdef.access";
const UNKNOWN_CLIENT_ID: &str = "ffffffffffffffffffffffffffffffff.access";

struct MixedIdentityProvider;

#[async_trait]
impl IdentityProvider for MixedIdentityProvider {
    async fn authenticate(&self, assertion: &str) -> Result<Identity, AuthError> {
        if let Some(common_name) = assertion.strip_prefix("service:") {
            return Ok(Identity::Service {
                common_name: ServiceCommonName::parse(common_name)?,
            });
        }
        Ok(Identity::Human {
            email: Email::parse(assertion).map_err(|_| AuthError::InvalidToken)?,
        })
    }
}

struct TestClock(RwLock<String>);

impl TestClock {
    fn new() -> Self {
        Self(RwLock::new(NOW.into()))
    }

    fn set(&self, now: &str) {
        *self.0.write().expect("clock fake lock") = now.into();
    }
}

impl Clock for TestClock {
    fn now(&self) -> String {
        self.0.read().expect("clock fake lock").clone()
    }
}

struct FixedIds;
impl IdGen for FixedIds {
    fn new_id(&self) -> String {
        "service-generated".into()
    }
}

struct Harness {
    app: Router,
    users: Arc<TestUserRepo>,
    trips: Arc<TestTripRepo>,
    services: Arc<TestServiceIdentityRepo>,
    clock: Arc<TestClock>,
}

impl Harness {
    fn new() -> Self {
        let users = Arc::new(TestUserRepo::new());
        let trips = Arc::new(TestTripRepo::new());
        let services = Arc::new(TestServiceIdentityRepo::default());
        let clock = Arc::new(TestClock::new());
        let app = create_app(AppState {
            identity: Arc::new(MixedIdentityProvider),
            users: users.clone(),
            trips: trips.clone(),
            content_history: trips.clone(),
            proposals: trips.clone(),
            polls: trips.clone(),
            discussions: trips.clone(),
            ledger: trips.clone(),
            notices: trips.clone(),
            service_identities: services.clone(),
            access_policy: Arc::new(DevAccessPolicy),
            place_catalog: Arc::new(EmptyPlaceCatalog),
            fx_rates: Arc::new(FixedFxRateProvider(0.5)),
            id_gen: Arc::new(FixedIds),
            clock: clock.clone(),
        });
        Self {
            app,
            users,
            trips,
            services,
            clock,
        }
    }

    async fn seed(&self) {
        for (id, email) in [("owner", OWNER_EMAIL), ("other", OTHER_EMAIL)] {
            self.users
                .insert(User {
                    id: UserId(id.into()),
                    email: Email::parse(email).unwrap(),
                    display_name: Some(id.into()),
                })
                .await
                .unwrap();
        }
        for trip_id in ["trip-a", "trip-b"] {
            self.trips
                .seed_trip(
                    Trip::try_from(TripData {
                        id: trip_id.into(),
                        name: trip_id.into(),
                        cover_photo_url: None,
                        accent_color: None,
                        stop_kind_labels: None,
                        status: TripStatus::Dreaming,
                        dates: DateRange::try_new("2026-11-01".into(), "2026-11-02".into())
                            .expect("valid fixture dates"),
                        base_currency: "GBP".parse().expect("valid fixture currency"),
                        soft_budget: None,
                        members: vec![
                            TripMember::try_new("owner".into(), TripRole::Leader, NOW.into())
                                .expect("valid owner membership"),
                            TripMember::try_new("other".into(), TripRole::Leader, NOW.into())
                                .expect("valid other membership"),
                        ],
                        current_plan_id: None,
                        created_at: NOW.into(),
                    })
                    .expect("valid fixture trip"),
                )
                .unwrap();
        }
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        assertion: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let (body, content_type) = match body {
            Some(value) => (
                serde_json::to_vec(&value).unwrap(),
                Some("application/json"),
            ),
            None => (Vec::new(), None),
        };
        self.request_bytes(method, path, assertion, body, content_type)
            .await
    }

    async fn request_bytes(
        &self,
        method: Method,
        path: &str,
        assertion: &str,
        body: Vec<u8>,
        content_type: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(ASSERTION_HEADER, assertion);
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        let response = self
            .app
            .clone()
            .oneshot(builder.body(Body::from(body)).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, value)
    }

    async fn register(&self, scopes: Value) -> Value {
        let (status, body) = self
            .request(
                Method::POST,
                "/me/service-identities",
                OWNER_EMAIL,
                Some(json!({
                    "name": "claude",
                    "clientId": CLIENT_ID,
                    "scopes": scopes,
                    "tripIds": ["trip-a"],
                    "ttlHours": 24
                })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body
    }
}

#[tokio::test]
async fn human_management_contract_never_returns_or_accepts_a_custom_secret() {
    let harness = Harness::new();
    harness.seed().await;
    let created = harness.register(json!(["read", "propose"])).await;
    assert_eq!(created["id"], "service-generated");
    assert_eq!(created["clientIdHint"], "12345678");
    assert_eq!(created["tripIds"], json!(["trip-a"]));
    let encoded = created.to_string();
    assert!(!encoded.contains(CLIENT_ID));
    assert!(!encoded.contains("plaintext"));
    assert!(!encoded.contains("secret"));

    let (status, listed) = harness
        .request(Method::GET, "/me/service-identities", OWNER_EMAIL, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed, json!([created]));

    let (status, duplicate) = harness
        .request(
            Method::POST,
            "/me/service-identities",
            OWNER_EMAIL,
            Some(json!({
                "name": "duplicate",
                "clientId": CLIENT_ID,
                "scopes": ["read"],
                "tripIds": ["trip-a"],
                "ttlHours": 1
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(duplicate["error"], "duplicate_service_credential");

    let pasted_secret = "a".repeat(64);
    let (status, rejected_secret) = harness
        .request(
            Method::POST,
            "/me/service-identities",
            OWNER_EMAIL,
            Some(json!({
                "name": "pasted secret",
                "clientId": pasted_secret,
                "scopes": ["read"],
                "tripIds": ["trip-a"],
                "ttlHours": 1
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(rejected_secret["error"], "invalid_request");

    let (status, malformed) = harness
        .request(
            Method::POST,
            "/me/service-identities",
            OWNER_EMAIL,
            Some(json!({
                "name": "bad",
                "clientId": "client.access",
                "clientSecret": "must-never-be-accepted",
                "scopes": ["read"],
                "tripIds": ["trip-a"],
                "ttlHours": 24
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(malformed["error"], "invalid_request");

    let (status, cross_owner) = harness
        .request(
            Method::DELETE,
            "/me/service-identities/service-generated",
            OTHER_EMAIL,
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(cross_owner["error"], "not_found");

    for (method, path) in [
        (Method::GET, "/me/service-identities"),
        (Method::DELETE, "/me/service-identities/service-generated"),
    ] {
        let (status, nonempty) = harness
            .request(method, path, OWNER_EMAIL, Some(json!({})))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {nonempty}");
        assert_eq!(nonempty["error"], "invalid_request");
    }
    let (status, oversized) = harness
        .request_bytes(
            Method::GET,
            "/me/service-identities",
            OWNER_EMAIL,
            vec![b'x'; 1_025],
            None,
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(oversized["error"], "payload_too_large");
}

#[tokio::test]
async fn service_reads_are_scope_and_trip_bounded_while_every_direct_mutation_is_denied() {
    let harness = Harness::new();
    harness.seed().await;
    harness.register(json!(["read", "propose"])).await;
    let service_assertion = format!("service:{CLIENT_ID}");

    let candidate_place = json!({
        "name": "Temple",
        "kind": "sight",
        "city": "Kyoto",
        "address": "1 Test Street",
        "website": null,
        "phone": null,
        "openingHours": [],
        "photoUrls": [],
        "guide": null
    });
    let (status, candidate) = harness
        .request(
            Method::POST,
            "/trips/trip-b/candidates",
            OWNER_EMAIL,
            Some(json!({
                "sourcePlaceId": null,
                "place": candidate_place.clone(),
                "pitch": "Seed plan",
                "tags": []
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{candidate}");
    let place_id = candidate["place"]["id"].as_str().unwrap();
    let (status, plan) = harness
        .request(
            Method::POST,
            "/trips/trip-b/plan",
            OWNER_EMAIL,
            Some(json!({"anchorPlaceId": place_id})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    let (status, thread) = harness
        .request(
            Method::POST,
            "/trips/trip-b/threads",
            OWNER_EMAIL,
            Some(json!({
                "anchor": {"kind": "trip"},
                "title": "Seed thread",
                "body": "For isolation coverage"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{thread}");
    let thread_id = thread["id"].as_str().unwrap();

    let (status, trips) = harness
        .request(Method::GET, "/trips", &service_assertion, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(trips.as_array().unwrap().len(), 1);
    assert_eq!(trips[0]["id"], "trip-a");

    let (status, _) = harness
        .request(Method::GET, "/trips/trip-a", &service_assertion, None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let scoped_read_paths = vec![
        "/trips/trip-b".to_string(),
        "/trips/trip-b/members".to_string(),
        "/trips/trip-b/places/search?q=temple".to_string(),
        "/trips/trip-b/candidates".to_string(),
        "/trips/trip-b/plan".to_string(),
        "/trips/trip-b/plan/versions".to_string(),
        "/trips/trip-b/history".to_string(),
        "/trips/trip-b/proposals".to_string(),
        "/trips/trip-b/polls".to_string(),
        "/trips/trip-b/threads".to_string(),
        format!("/trips/trip-b/threads/{thread_id}/comments"),
        "/trips/trip-b/ledger".to_string(),
        "/trips/trip-b/notices".to_string(),
    ];
    for path in &scoped_read_paths {
        let (status, hidden) = harness
            .request(Method::GET, path, &service_assertion, None)
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {hidden}");
        assert_eq!(hidden["error"], "not_found", "{path}: {hidden}");
    }

    let proposal = json!({
        "title": "Change",
        "rationale": "Reason",
        "changeSet": {"basePlanVersion": 1, "ops": []},
        "route": "leader_approval"
    });
    let direct_mutations = vec![
        (
            Method::POST,
            "/trips".to_string(),
            Some(json!({
                "name": "Forbidden",
                "startDate": "2026-11-01",
                "endDate": "2026-11-02",
                "baseCurrency": "GBP"
            })),
        ),
        (
            Method::PATCH,
            "/trips/trip-a/status".to_string(),
            Some(json!({"status": "planning"})),
        ),
        (
            Method::DELETE,
            "/trips/trip-a/members/other".to_string(),
            None,
        ),
        (
            Method::POST,
            "/trips/trip-a/invites".to_string(),
            Some(json!({"email": "invitee@example.test"})),
        ),
        (
            Method::POST,
            "/trips/trip-a/candidates".to_string(),
            Some(json!({
                "sourcePlaceId": null,
                "place": candidate_place.clone(),
                "pitch": "Idea",
                "tags": []
            })),
        ),
        (
            Method::PATCH,
            "/trips/trip-a/candidates/candidate-a".to_string(),
            Some(json!({"place": candidate_place, "pitch": "Idea", "tags": []})),
        ),
        (
            Method::PATCH,
            "/trips/trip-a/candidates/candidate-a/status".to_string(),
            Some(json!({"status": "shortlisted"})),
        ),
        (
            Method::POST,
            "/trips/trip-a/plan".to_string(),
            Some(json!({"anchorPlaceId": "place-a"})),
        ),
        (
            Method::PATCH,
            "/trips/trip-a/stops/stop-a".to_string(),
            Some(json!({"notes": "changed"})),
        ),
        (
            Method::PATCH,
            "/trips/trip-a/days/day-a".to_string(),
            Some(json!({"cityHint": "Kyoto"})),
        ),
        (
            Method::POST,
            "/trips/trip-a/proposals".to_string(),
            Some(proposal),
        ),
        (
            Method::POST,
            "/trips/trip-a/proposals/proposal-a/approve".to_string(),
            None,
        ),
        (
            Method::POST,
            "/trips/trip-a/proposals/proposal-a/reject".to_string(),
            Some(json!({"reason": "No"})),
        ),
        (
            Method::POST,
            "/trips/trip-a/proposals/proposal-a/to-poll".to_string(),
            None,
        ),
        (
            Method::POST,
            "/trips/trip-a/polls".to_string(),
            Some(json!({
                "kind": "decision",
                "title": "Choose",
                "description": "Pick one",
                "options": [{"label": "A"}, {"label": "B"}],
                "closesAt": "2026-08-06T14:00:00Z",
                "allowMulti": false
            })),
        ),
        (
            Method::POST,
            "/trips/trip-a/polls/poll-a/open".to_string(),
            None,
        ),
        (
            Method::POST,
            "/trips/trip-a/polls/poll-a/votes".to_string(),
            Some(json!({"optionIds": []})),
        ),
        (
            Method::POST,
            "/trips/trip-a/polls/poll-a/close".to_string(),
            None,
        ),
        (
            Method::POST,
            "/trips/trip-a/threads".to_string(),
            Some(json!({
                "anchor": {"kind": "trip"},
                "title": "Thread",
                "body": "Body"
            })),
        ),
        (
            Method::POST,
            "/trips/trip-a/threads/thread-a/comments".to_string(),
            Some(json!({"body": "Comment"})),
        ),
        (
            Method::POST,
            "/trips/trip-a/threads/thread-a/comments/comment-a/reactions".to_string(),
            Some(json!({"emoji": "👍", "active": true})),
        ),
        (
            Method::POST,
            "/trips/trip-a/expenses".to_string(),
            Some(json!({
                "paidBy": "owner",
                "amount": 10,
                "currency": "GBP",
                "category": "other",
                "split": {"kind": "even", "participantIds": ["owner"]},
                "note": "Expense",
                "linkedStopId": null
            })),
        ),
        (
            Method::PATCH,
            "/trips/trip-a/expenses/expense-a".to_string(),
            Some(json!({"note": "Changed"})),
        ),
        (
            Method::DELETE,
            "/trips/trip-a/expenses/expense-a".to_string(),
            None,
        ),
        (
            Method::POST,
            "/trips/trip-a/settlements".to_string(),
            Some(json!({"fromUser": "owner", "toUser": "other", "amount": 1})),
        ),
        (
            Method::POST,
            "/trips/trip-a/notices".to_string(),
            Some(json!({"category": "custom", "title": "Notice", "body": "Body"})),
        ),
        (
            Method::PATCH,
            "/trips/trip-a/notices/notice-a".to_string(),
            Some(json!({"title": "Changed"})),
        ),
        (
            Method::POST,
            "/trips/trip-a/notices/notice-a/checklist/item-a/toggle".to_string(),
            None,
        ),
        (
            Method::POST,
            "/trips/trip-a/edits/edit-a/revert".to_string(),
            None,
        ),
        (
            Method::POST,
            "/me/service-identities".to_string(),
            Some(json!({
                "name": "forbidden",
                "clientId": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.access",
                "scopes": ["read"],
                "tripIds": ["trip-a"],
                "ttlHours": 1
            })),
        ),
        (
            Method::DELETE,
            "/me/service-identities/service-generated".to_string(),
            None,
        ),
    ];
    assert_eq!(
        direct_mutations.len(),
        31,
        "update this table for every Axum mutation route"
    );
    for (method, path, body) in direct_mutations {
        let (status, denied) = harness
            .request(method, &path, &service_assertion, body)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {denied}");
        assert_eq!(denied["error"], "forbidden");
    }
    for path in ["/me", "/me/service-identities"] {
        let (status, denied) = harness
            .request(Method::GET, path, &service_assertion, None)
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(denied["error"], "forbidden");
    }
}

#[tokio::test]
async fn propose_only_unknown_revoked_and_rate_limited_services_fail_closed_without_provisioning() {
    let harness = Harness::new();
    harness.seed().await;
    harness.register(json!(["propose"])).await;
    let initial_users = harness.users.len();
    let service_assertion = format!("service:{CLIENT_ID}");

    let (status, denied) = harness
        .request(Method::GET, "/trips/trip-a", &service_assertion, None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(denied["error"], "forbidden");

    let (status, unknown) = harness
        .request(
            Method::GET,
            "/trips",
            &format!("service:{UNKNOWN_CLIENT_ID}"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(unknown["error"], "invalid_service_identity");
    assert_eq!(harness.users.len(), initial_users);

    harness
        .services
        .set_uses(CLIENT_ID, SERVICE_REQUESTS_PER_HOUR);
    let (status, limited) = harness
        .request(Method::GET, "/trips", &service_assertion, None)
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited["error"], "service_rate_limited");

    harness.services.set_uses(CLIENT_ID, 0);
    for _ in 0..2 {
        let (status, body) = harness
            .request(
                Method::DELETE,
                "/me/service-identities/service-generated",
                OWNER_EMAIL,
                None,
            )
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    }
    let (status, revoked) = harness
        .request(Method::GET, "/trips", &service_assertion, None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(revoked["error"], "invalid_service_identity");
}

#[tokio::test]
async fn service_access_stops_at_mapping_expiry_and_after_owner_membership_removal() {
    let harness = Harness::new();
    harness.seed().await;
    let (status, created) = harness
        .request(
            Method::POST,
            "/me/service-identities",
            OWNER_EMAIL,
            Some(json!({
                "name": "short lived",
                "clientId": CLIENT_ID,
                "scopes": ["read"],
                "tripIds": ["trip-a"],
                "ttlHours": 1
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let service_assertion = format!("service:{CLIENT_ID}");

    harness
        .trips
        .remove_member("trip-a", &UserId("other".into()), &UserId("owner".into()))
        .await
        .expect("another leader can remove the mapped owner");
    let (status, hidden) = harness
        .request(Method::GET, "/trips/trip-a", &service_assertion, None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{hidden}");
    let (status, trips) = harness
        .request(Method::GET, "/trips", &service_assertion, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{trips}");
    assert_eq!(trips, json!([]));

    harness.clock.set("2026-08-06T13:00:00Z");
    let (status, expired) = harness
        .request(Method::GET, "/trips", &service_assertion, None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{expired}");
    assert_eq!(expired["error"], "invalid_service_identity");
}
