mod support;
#[path = "support/user_repo.rs"]
mod user_repo;

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode},
};
use itinera_adapters::insecure::external::{DevAccessPolicy, EmptyPlaceCatalog};
use itinera_api::{
    create_app,
    routes::{
        content_history::REVERT_BODY_LIMIT_BYTES,
        discussions::{
            DISCUSSION_BODYLESS_LIMIT_BYTES, DISCUSSION_REACTION_BODY_LIMIT_BYTES,
            DISCUSSION_WRITE_BODY_LIMIT_BYTES,
        },
        ledger::{LEDGER_BODYLESS_LIMIT_BYTES, LEDGER_WRITE_BODY_LIMIT_BYTES},
        notices::{NOTICE_BODYLESS_LIMIT_BYTES, NOTICE_WRITE_BODY_LIMIT_BYTES},
        polls::{
            POLL_BODYLESS_LIMIT_BYTES, POLL_CREATE_BODY_LIMIT_BYTES, POLL_VOTE_BODY_LIMIT_BYTES,
        },
    },
    state::AppState,
};
use itinera_core::{
    domain::{
        trip::{DateRange, Trip, TripData, TripMember, TripRole, TripStatus},
        user::{Email, User, UserId},
    },
    ports::{
        access_policy::{AccessPolicy, AccessPolicyError},
        auth::{AuthError, Identity, IdentityProvider},
        clock::Clock,
        id_gen::IdGen,
        user::UserRepo,
    },
};
use serde_json::{Value, json};
use tower::ServiceExt;

use support::{
    FixedFxRateProvider, service_identity_repo::TestServiceIdentityRepo, trip_repo::TestTripRepo,
};
use user_repo::TestUserRepo;

const ASSERTION_HEADER: &str = "cf-access-jwt-assertion";
const NOW: &str = "2026-08-05T12:00:00.000Z";

struct EmailIdentityProvider;

#[async_trait]
impl IdentityProvider for EmailIdentityProvider {
    async fn authenticate(&self, assertion: &str) -> Result<Identity, AuthError> {
        Ok(Identity::Human {
            email: Email::parse(assertion).map_err(|_| AuthError::InvalidToken)?,
        })
    }
}

#[tokio::test]
async fn ledger_routes_enforce_roles_contracts_frozen_fx_and_cross_trip_isolation() {
    let harness = Harness::new();
    let leader = harness.seed_user("leader", "leader@example.test").await;
    let member = harness.seed_user("member", "member@example.test").await;
    let viewer = harness.seed_user("viewer", "viewer@example.test").await;
    harness
        .seed_trip(vec![
            (&leader, TripRole::Leader),
            (&member, TripRole::Member),
            (&viewer, TripRole::Viewer),
        ])
        .await;

    let (status, empty) = harness
        .json(
            Method::GET,
            "/trips/trip-a/ledger",
            "viewer@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty["expenses"], json!([]));
    assert_eq!(empty["settlements"], json!([]));
    assert_eq!(empty["balances"].as_array().map(Vec::len), Some(3));

    let expense_input = json!({
        "paidBy": "member",
        "amount": 90,
        "currency": "EUR",
        "category": "food",
        "split": {"kind": "even", "participantIds": ["leader", "member", "viewer"]},
        "note": "Dinner"
    });
    let (status, body) = harness
        .json_with_idempotency(
            Method::POST,
            "/trips/trip-a/expenses",
            "viewer@example.test",
            Some(expense_input.clone()),
            Some("expense-viewer-1"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    let (status, expense) = harness
        .json_with_idempotency(
            Method::POST,
            "/trips/trip-a/expenses",
            "member@example.test",
            Some(expense_input.clone()),
            Some("expense-member-1"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(expense["id"], "generated-001");
    assert_eq!(expense["fxRateToBase"], 0.5);
    assert_eq!(expense["receiptPhotoUrl"], Value::Null);
    assert_eq!(expense["linkedStopId"], Value::Null);

    let (status, replayed) = harness
        .json_with_idempotency(
            Method::POST,
            "/trips/trip-a/expenses",
            "member@example.test",
            Some(expense_input.clone()),
            Some("expense-member-1"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed, expense, "a lost create response replays exactly");

    let mut conflicting_replay = expense_input.clone();
    conflicting_replay["note"] = json!("A different dinner");
    let (status, body) = harness
        .json_with_idempotency(
            Method::POST,
            "/trips/trip-a/expenses",
            "member@example.test",
            Some(conflicting_replay),
            Some("expense-member-1"),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "conflict");

    let (status, body) = harness
        .json_with_idempotency(
            Method::POST,
            "/trips/trip-a/expenses",
            "leader@example.test",
            Some(expense_input.clone()),
            Some("expense-member-1"),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "conflict", "keys are bound to their actor");

    let (status, body) = harness
        .json(
            Method::POST,
            "/trips/trip-a/expenses",
            "member@example.test",
            Some(expense_input.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");

    let mut forged = expense_input.clone();
    forged["fxRateToBase"] = json!(99);
    let (status, _) = harness
        .json_with_idempotency(
            Method::POST,
            "/trips/trip-a/expenses",
            "member@example.test",
            Some(forged),
            Some("expense-forged-1"),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = harness
        .json(
            Method::PATCH,
            "/trips/trip-a/expenses/generated-001",
            "member@example.test",
            Some(json!({"linkedStopId": null})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "explicit null is a valid clear patch"
    );

    let (status, updated) = harness
        .json(
            Method::PATCH,
            "/trips/trip-a/expenses/generated-001",
            "leader@example.test",
            Some(json!({"amount": 120, "note": "Dinner corrected"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(updated["amount"], 120.0);
    assert_eq!(updated["fxRateToBase"], 0.5);

    let (status, ledger) = harness
        .json(
            Method::GET,
            "/trips/trip-a/ledger",
            "viewer@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let balance = |user: &str| {
        ledger["balances"]
            .as_array()
            .and_then(|balances| balances.iter().find(|balance| balance["userId"] == user))
            .cloned()
            .expect("member balance")
    };
    assert_eq!(balance("member")["net"], 40.0);
    assert_eq!(balance("leader")["net"], -20.0);
    assert_eq!(balance("viewer")["net"], -20.0);

    let settlement = json!({"fromUser": "leader", "toUser": "member", "amount": 5});
    let (status, _) = harness
        .json_with_idempotency(
            Method::POST,
            "/trips/trip-a/settlements",
            "viewer@example.test",
            Some(settlement.clone()),
            Some("settlement-viewer-1"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let settlement_input = settlement.clone();
    let (status, created) = harness
        .json_with_idempotency(
            Method::POST,
            "/trips/trip-a/settlements",
            "member@example.test",
            Some(settlement),
            Some("settlement-member-1"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["amount"], 5.0);
    let (status, replayed) = harness
        .json_with_idempotency(
            Method::POST,
            "/trips/trip-a/settlements",
            "member@example.test",
            Some(settlement_input),
            Some("settlement-member-1"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed, created, "settlement retries replay exactly");

    let (status, _) = harness
        .json(
            Method::DELETE,
            "/trips/trip-a/expenses/generated-001",
            "viewer@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = harness
        .json(
            Method::DELETE,
            "/trips/trip-a/expenses/generated-001",
            "member@example.test",
            Some(json!({"forged": true})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, body) = harness
        .json(
            Method::DELETE,
            "/trips/trip-a/expenses/generated-001",
            "member@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);

    let (status, body) = harness
        .json(
            Method::GET,
            "/trips/trip-a/ledger",
            "outsider@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");

    let (status, _) = harness
        .json(
            Method::GET,
            "/trips/trip-a/ledger",
            "leader@example.test",
            Some(json!({"padding": "x".repeat(LEDGER_BODYLESS_LIMIT_BYTES + 1)})),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _) = harness
        .json_with_idempotency(
            Method::POST,
            "/trips/trip-a/expenses",
            "leader@example.test",
            Some(json!({"padding": "x".repeat(LEDGER_WRITE_BODY_LIMIT_BYTES + 1)})),
            Some("expense-oversized-1"),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn notice_http_contract_enforces_management_audience_and_caller_owned_checklists() {
    let harness = Harness::new();
    let leader = harness.seed_user("leader", "leader@example.test").await;
    let member = harness.seed_user("member", "member@example.test").await;
    let viewer = harness.seed_user("viewer", "viewer@example.test").await;
    let departed = harness.seed_user("departed", "departed@example.test").await;
    harness
        .seed_trip(vec![
            (&leader, TripRole::Leader),
            (&member, TripRole::Member),
            (&viewer, TripRole::Viewer),
            (&departed, TripRole::Member),
        ])
        .await;

    let notices_path = "/trips/trip-a/notices";
    let (status, notices) = harness
        .json(Method::GET, notices_path, "viewer@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(notices, json!([]));

    let input = json!({
        "category": "safety",
        "title": "  Register the trip  ",
        "body": "  Save the emergency number.  ",
        "sourceUrl": "https://example.com/advice",
        "checklistItems": ["  Register online  "],
        "audience": ["member", "viewer", "departed"]
    });
    let (status, body) = harness
        .json_with_idempotency(
            Method::POST,
            notices_path,
            "viewer@example.test",
            Some(input.clone()),
            Some("notice-viewer-create"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    let (status, body) = harness
        .json(
            Method::POST,
            notices_path,
            "member@example.test",
            Some(input.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");

    let (status, created) = harness
        .json_with_idempotency(
            Method::POST,
            notices_path,
            "member@example.test",
            Some(input.clone()),
            Some("notice-create-a"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(created["id"], "generated-002");
    assert_eq!(created["createdBy"], "member");
    assert_eq!(created["title"], "Register the trip");
    assert_eq!(created["body"], "Save the emergency number.");
    assert_eq!(created["status"], "active");
    assert_eq!(created["pinned"], false);
    assert_eq!(created["checklistItems"][0]["id"], "generated-001");
    assert_eq!(created["checklistItems"][0]["doneBy"], json!([]));
    assert_eq!(created["checklistItems"][0]["mode"], "each");
    let notice_id = created["id"].as_str().expect("notice id");
    let item_id = created["checklistItems"][0]["id"]
        .as_str()
        .expect("item id");

    let (status, managed) = harness
        .json(
            Method::PATCH,
            &format!("/trips/trip-a/notices/{notice_id}"),
            "leader@example.test",
            Some(json!({"pinned": true, "status": "resolved", "sourceUrl": null})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(managed["pinned"], true);
    assert_eq!(managed["status"], "resolved");
    assert_eq!(managed["sourceUrl"], Value::Null);

    let (status, authored) = harness
        .json(
            Method::PATCH,
            &format!("/trips/trip-a/notices/{notice_id}"),
            "member@example.test",
            Some(json!({"title": "Registration"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(authored["title"], "Registration");

    let (status, history) = harness
        .json(
            Method::GET,
            "/trips/trip-a/history",
            "viewer@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let title_edit = history
        .as_array()
        .expect("history array")
        .iter()
        .find(|edit| edit["entity"] == "notice" && edit["field"] == "title")
        .expect("notice title edit");
    assert_eq!(title_edit["oldValue"], "Register the trip");
    assert_eq!(title_edit["newValue"], "Registration");
    let title_edit_id = title_edit["id"].as_str().expect("edit id").to_string();
    let revert_path = format!("/trips/trip-a/edits/{title_edit_id}/revert");
    let (status, _) = harness
        .json(Method::POST, &revert_path, "viewer@example.test", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = harness
        .json(Method::POST, &revert_path, "member@example.test", None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = harness
        .json(Method::POST, &revert_path, "member@example.test", None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, notices) = harness
        .json(Method::GET, notices_path, "viewer@example.test", None)
        .await;
    assert_eq!(notices[0]["title"], "Register the trip");

    let (status, replayed_create) = harness
        .json_with_idempotency(
            Method::POST,
            notices_path,
            "member@example.test",
            Some(input.clone()),
            Some("notice-create-a"),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed_create["id"], notice_id);
    assert_eq!(replayed_create["title"], "Register the trip");
    assert_eq!(replayed_create["sourceUrl"], Value::Null);
    let mut changed_input = input.clone();
    changed_input["title"] = json!("Different request");
    let (status, body) = harness
        .json_with_idempotency(
            Method::POST,
            notices_path,
            "member@example.test",
            Some(changed_input),
            Some("notice-create-a"),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "conflict");

    let (status, body) = harness
        .json(
            Method::PATCH,
            &format!("/trips/trip-a/notices/{notice_id}"),
            "viewer@example.test",
            Some(json!({"pinned": false})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    let toggle_path = format!("/trips/trip-a/notices/{notice_id}/checklist/{item_id}/toggle");
    let (status, body) = harness
        .json(Method::POST, &toggle_path, "viewer@example.test", None)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");
    let (status, checked) = harness
        .json_with_idempotency(
            Method::POST,
            &toggle_path,
            "viewer@example.test",
            None,
            Some("notice-toggle-a"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(checked["checklistItems"][0]["doneBy"], json!(["viewer"]));
    let (status, replayed) = harness
        .json_with_idempotency(
            Method::POST,
            &toggle_path,
            "viewer@example.test",
            None,
            Some("notice-toggle-a"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replayed, checked);
    let (status, unchecked) = harness
        .json_with_idempotency(
            Method::POST,
            &toggle_path,
            "viewer@example.test",
            None,
            Some("notice-toggle-b"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(unchecked["checklistItems"][0]["doneBy"], json!([]));

    let (status, checked_again) = harness
        .json_with_idempotency(
            Method::POST,
            &toggle_path,
            "viewer@example.test",
            None,
            Some("notice-toggle-c"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        checked_again["checklistItems"][0]["doneBy"],
        json!(["viewer"])
    );

    let (status, checked_by_departed) = harness
        .json_with_idempotency(
            Method::POST,
            &toggle_path,
            "departed@example.test",
            None,
            Some("notice-toggle-departed"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        checked_by_departed["checklistItems"][0]["doneBy"],
        json!(["viewer", "departed"])
    );
    let (status, _) = harness
        .json(
            Method::DELETE,
            "/trips/trip-a/members/departed",
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, narrowed) = harness
        .json(
            Method::PATCH,
            &format!("/trips/trip-a/notices/{notice_id}"),
            "member@example.test",
            Some(json!({"audience": ["member"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(narrowed["checklistItems"][0]["doneBy"], json!([]));
    let (status, _) = harness
        .json_with_idempotency(
            Method::POST,
            &toggle_path,
            "viewer@example.test",
            None,
            Some("notice-toggle-excluded"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    for malformed in [
        json!({}),
        json!({"title": ""}),
        json!({"sourceUrl": "javascript:alert(1)"}),
        json!({"audience": ["member", "member"]}),
        json!({"replacementValue": "forged"}),
        json!({"title": null}),
        json!({"body": null}),
        json!({"pinned": null}),
        json!({"status": null}),
    ] {
        let (status, _) = harness
            .json(
                Method::PATCH,
                &format!("/trips/trip-a/notices/{notice_id}"),
                "member@example.test",
                Some(malformed),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    for (index, field) in ["sourceUrl", "audience"].into_iter().enumerate() {
        let mut invalid = input.clone();
        invalid[field] = Value::Null;
        let (status, _) = harness
            .json_with_idempotency(
                Method::POST,
                notices_path,
                "member@example.test",
                Some(invalid),
                Some(&format!("notice-null-{index}")),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    let (status, _) = harness
        .json(
            Method::POST,
            &toggle_path,
            "member@example.test",
            Some(json!({"forgedUserId": "viewer"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = harness
        .json(
            Method::POST,
            &toggle_path,
            "member@example.test",
            Some(json!({"padding": "x".repeat(NOTICE_BODYLESS_LIMIT_BYTES + 1)})),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _) = harness
        .json(
            Method::POST,
            notices_path,
            "member@example.test",
            Some(json!({"padding": "x".repeat(NOTICE_WRITE_BODY_LIMIT_BYTES + 1)})),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);

    let (status, other_trip) = harness
        .json(
            Method::POST,
            "/trips",
            "leader@example.test",
            Some(json!({
                "name": "Other",
                "startDate": "2027-01-01",
                "endDate": "2027-01-02",
                "baseCurrency": "GBP"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let other_trip_id = other_trip["id"].as_str().expect("other trip id");
    let (status, body) = harness
        .json(
            Method::PATCH,
            &format!("/trips/{other_trip_id}/notices/{notice_id}"),
            "leader@example.test",
            Some(json!({"pinned": false})),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");
    let (status, body) = harness
        .json(
            Method::POST,
            &format!("/trips/{other_trip_id}/edits/{title_edit_id}/revert"),
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");

    let (status, body) = harness
        .json(Method::GET, notices_path, "outsider@example.test", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");
}

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> String {
        NOW.to_string()
    }
}

#[derive(Default)]
struct RecordingAccessPolicy {
    grants: AtomicUsize,
}

#[async_trait]
impl AccessPolicy for RecordingAccessPolicy {
    async fn grant_login(&self, _email: &Email) -> Result<(), AccessPolicyError> {
        self.grants.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn revoke_login(&self, _email: &Email) -> Result<(), AccessPolicyError> {
        Ok(())
    }
}

struct SequenceIds(Mutex<VecDeque<String>>);

impl SequenceIds {
    fn new() -> Self {
        Self(Mutex::new(
            (1..=1_000)
                .map(|value| format!("generated-{value:03}"))
                .collect(),
        ))
    }
}

impl IdGen for SequenceIds {
    fn new_id(&self) -> String {
        self.0
            .lock()
            .expect("id lock poisoned")
            .pop_front()
            .expect("test exhausted its ids")
    }
}

struct Harness {
    app: Router,
    users: Arc<TestUserRepo>,
    trips: Arc<TestTripRepo>,
}

impl Harness {
    fn new() -> Self {
        Self::with_access_policy(Arc::new(DevAccessPolicy))
    }

    fn with_access_policy(access_policy: Arc<dyn AccessPolicy>) -> Self {
        let users = Arc::new(TestUserRepo::new());
        let trips = Arc::new(TestTripRepo::new());
        let app = create_app(AppState {
            identity: Arc::new(EmailIdentityProvider),
            users: users.clone(),
            trips: trips.clone(),
            content_history: trips.clone(),
            proposals: trips.clone(),
            polls: trips.clone(),
            discussions: trips.clone(),
            ledger: trips.clone(),
            notices: trips.clone(),
            service_identities: Arc::new(TestServiceIdentityRepo::default()),
            access_policy,
            place_catalog: Arc::new(EmptyPlaceCatalog),
            fx_rates: Arc::new(FixedFxRateProvider(0.5)),
            id_gen: Arc::new(SequenceIds::new()),
            clock: Arc::new(FixedClock),
        });
        Self { app, users, trips }
    }

    async fn seed_user(&self, id: &str, email: &str) -> User {
        let user = User {
            id: UserId(id.to_string()),
            email: Email::parse(email).expect("fixture email"),
            display_name: Some(id.to_string()),
        };
        self.users.insert(user.clone()).await.expect("seed user");
        user
    }

    async fn seed_trip(&self, members: Vec<(&User, TripRole)>) -> Trip {
        let trip = Trip::try_from(TripData {
            id: "trip-a".into(),
            name: "Japan".into(),
            cover_photo_url: None,
            accent_color: None,
            stop_kind_labels: None,
            status: TripStatus::Dreaming,
            dates: DateRange::try_new("2026-11-01".into(), "2026-11-03".into())
                .expect("valid fixture dates"),
            base_currency: "GBP".parse().expect("valid fixture currency"),
            soft_budget: None,
            members: members
                .into_iter()
                .map(|(user, role)| TripMember::try_new(user.id.0.clone(), role, NOW.into()))
                .collect::<Result<Vec<_>, _>>()
                .expect("valid fixture memberships"),
            current_plan_id: None,
            created_at: NOW.into(),
        })
        .expect("valid fixture trip");
        self.trips.seed_trip(trip.clone()).expect("seed trip");
        trip
    }

    async fn json(
        &self,
        method: Method,
        path: &str,
        email: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        self.json_with_idempotency(method, path, email, body, None)
            .await
    }

    async fn json_with_idempotency(
        &self,
        method: Method,
        path: &str,
        email: &str,
        body: Option<Value>,
        idempotency_key: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(ASSERTION_HEADER, email);
        if let Some(key) = idempotency_key {
            request = request.header("idempotency-key", key);
        }
        if body.is_some() {
            request = request.header("content-type", "application/json");
        }
        let request = request
            .body(Body::from(
                body.map_or_else(String::new, |value| value.to_string()),
            ))
            .expect("request");
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("router is infallible");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body");
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("API responses are JSON")
        };
        (status, value)
    }
}

#[tokio::test]
async fn trip_crud_uses_the_authenticated_actor_and_hides_cross_trip_data() {
    let harness = Harness::new();
    let (status, created) = harness
        .json(
            Method::POST,
            "/trips",
            "leader@example.test",
            Some(json!({
                "name": "  Japan  ",
                "startDate": "2026-11-01",
                "endDate": "2026-11-03",
                "baseCurrency": "gbp"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["name"], "Japan");
    assert_eq!(created["baseCurrency"], "GBP");
    assert_eq!(created["status"], "dreaming");
    assert_eq!(created["members"][0]["role"], "leader");
    let trip_id = created["id"].as_str().expect("trip id");

    let (status, trips) = harness
        .json(Method::GET, "/trips", "leader@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(trips.as_array().expect("trip list").len(), 1);
    assert_eq!(trips[0]["memberCount"], 1);

    let (status, _) = harness
        .json(
            Method::GET,
            &format!("/trips/{trip_id}"),
            "stranger@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn roles_are_enforced_and_the_last_leader_is_protected() {
    let harness = Harness::new();
    let leader = harness.seed_user("leader", "leader@example.test").await;
    let member = harness.seed_user("member", "member@example.test").await;
    let viewer = harness.seed_user("viewer", "viewer@example.test").await;
    harness
        .seed_trip(vec![
            (&leader, TripRole::Leader),
            (&member, TripRole::Member),
            (&viewer, TripRole::Viewer),
        ])
        .await;

    let (status, trip) = harness
        .json(
            Method::PATCH,
            "/trips/trip-a/status",
            "member@example.test",
            Some(json!({"status": "booked"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(trip["status"], "booked");

    let (status, _) = harness
        .json(
            Method::PATCH,
            "/trips/trip-a/status",
            "viewer@example.test",
            Some(json!({"status": "done"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = harness
        .json(
            Method::DELETE,
            "/trips/trip-a/members/leader",
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let (status, body) = harness
        .json(
            Method::DELETE,
            "/trips/trip-a/members/viewer",
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);

    let (status, _) = harness
        .json(Method::GET, "/trips/trip-a", "viewer@example.test", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_pending_invite_becomes_membership_on_the_invitees_me_request() {
    let harness = Harness::new();
    let leader = harness.seed_user("leader", "leader@example.test").await;
    harness.seed_trip(vec![(&leader, TripRole::Leader)]).await;

    let (status, invite) = harness
        .json(
            Method::POST,
            "/trips/trip-a/invites",
            "leader@example.test",
            Some(json!({"email": " Friend@Example.Test "})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(invite["email"], "friend@example.test");
    assert_eq!(invite["status"], "pending");

    let (status, me) = harness
        .json(Method::GET, "/me", "friend@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let friend_id = me["id"].as_str().expect("friend id");

    let (status, trip) = harness
        .json(Method::GET, "/trips/trip-a", "friend@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        trip["members"]
            .as_array()
            .expect("members")
            .iter()
            .any(|member| member["userId"] == friend_id && member["role"] == "member")
    );

    let (status, _) = harness
        .json(
            Method::DELETE,
            &format!("/trips/trip-a/members/{friend_id}"),
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, reinvite) = harness
        .json(
            Method::POST,
            "/trips/trip-a/invites",
            "leader@example.test",
            Some(json!({"email": "friend@example.test"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(reinvite["status"], "pending");
    assert_ne!(reinvite["id"], invite["id"]);

    let (status, _) = harness
        .json(Method::GET, "/me", "friend@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = harness
        .json(Method::GET, "/trips/trip-a", "friend@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn an_overlong_invite_email_is_rejected_before_access_is_granted() {
    let access_policy = Arc::new(RecordingAccessPolicy::default());
    let harness = Harness::with_access_policy(access_policy.clone());
    let leader = harness.seed_user("leader", "leader@example.test").await;
    harness.seed_trip(vec![(&leader, TripRole::Leader)]).await;
    let suffix = "@example.test";
    let overlong_email = format!("{}{}", "a".repeat(321 - suffix.len()), suffix);

    let (status, body) = harness
        .json(
            Method::POST,
            "/trips/trip-a/invites",
            "leader@example.test",
            Some(json!({"email": overlong_email})),
        )
        .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");
    assert_eq!(access_policy.grants.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn manual_candidates_copy_on_write_and_bootstrap_one_idempotent_plan() {
    let harness = Harness::new();
    let leader = harness.seed_user("leader", "leader@example.test").await;
    harness.seed_trip(vec![(&leader, TripRole::Leader)]).await;
    let input = json!({
        "sourcePlaceId": null,
        "place": {
            "name": " Fushimi Inari ",
            "kind": "sight",
            "city": " Kyoto ",
            "address": "Fushimi",
            "website": "https://example.test/place",
            "phone": null,
            "openingHours": ["  Always open  "],
            "photoUrls": [],
            "guide": null
        },
        "pitch": " Early morning walk ",
        "tags": [" must-see "]
    });
    let (status, candidate) = harness
        .json(
            Method::POST,
            "/trips/trip-a/candidates",
            "leader@example.test",
            Some(input),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(candidate["place"]["name"], "Fushimi Inari");
    assert_eq!(candidate["place"]["city"], "Kyoto");
    assert_eq!(candidate["place"]["tz"], "UTC");
    assert_eq!(candidate["sourcePlaceId"], Value::Null);
    assert_eq!(candidate["place"]["externalRef"], Value::Null);
    assert_eq!(candidate["place"]["lat"], 0.0);
    assert_eq!(candidate["place"]["lng"], 0.0);
    assert_eq!(candidate["place"]["rating"], Value::Null);
    assert_eq!(candidate["pitch"], "Early morning walk");
    let candidate_id = candidate["id"].as_str().expect("candidate id");
    let first_place_id = candidate["placeId"].as_str().expect("place id");

    let (status, saved_places) = harness
        .json(
            Method::GET,
            "/trips/trip-a/places/search?q=Fushimi",
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved_places, json!([]));

    let (status, updated) = harness
        .json(
            Method::PATCH,
            &format!("/trips/trip-a/candidates/{candidate_id}"),
            "leader@example.test",
            Some(json!({
                "place": {
                    "name": "Fushimi Inari Taisha",
                    "kind": "sight",
                    "city": "Kyoto",
                    "address": "Fushimi",
                    "website": "https://example.test/place",
                    "phone": null,
                    "openingHours": ["Always open"],
                    "photoUrls": [],
                    "guide": null
                },
                "pitch": "Go before breakfast",
                "tags": ["must-see"]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(updated["placeId"], first_place_id);

    let anchor_place_id = updated["placeId"].as_str().expect("new place id");
    let (status, first_plan) = harness
        .json(
            Method::POST,
            "/trips/trip-a/plan",
            "leader@example.test",
            Some(json!({"anchorPlaceId": anchor_place_id})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first_plan["days"].as_array().expect("days").len(), 3);
    assert!(
        first_plan["days"]
            .as_array()
            .expect("days")
            .iter()
            .all(|day| day["cityHint"] == "Kyoto" && day["tz"] == "UTC")
    );

    let (status, retry) = harness
        .json(
            Method::POST,
            "/trips/trip-a/plan",
            "leader@example.test",
            Some(json!({"anchorPlaceId": "ignored-on-retry"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(retry["plan"]["id"], first_plan["plan"]["id"]);

    let (status, _) = harness
        .json(
            Method::POST,
            "/trips/trip-a/plan",
            "leader@example.test",
            Some(json!({"anchorPlaceId": ""})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let day_id = first_plan["days"][0]["id"].as_str().expect("day id");
    let (status, _) = harness
        .json(
            Method::PATCH,
            &format!("/trips/trip-a/days/{day_id}"),
            "leader@example.test",
            Some(json!({"windowStart": "22:00", "windowEnd": "09:00"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn structural_proposal_http_contract_enforces_roles_and_publishes_one_immutable_version() {
    let harness = Harness::new();
    let leader = harness.seed_user("leader", "leader@example.test").await;
    let member = harness.seed_user("member", "member@example.test").await;
    let viewer = harness.seed_user("viewer", "viewer@example.test").await;
    harness
        .seed_trip(vec![
            (&leader, TripRole::Leader),
            (&member, TripRole::Member),
            (&viewer, TripRole::Viewer),
        ])
        .await;
    let (status, candidate) = harness
        .json(
            Method::POST,
            "/trips/trip-a/candidates",
            "member@example.test",
            Some(json!({
                "sourcePlaceId": null,
                "place": {
                    "name": "Fushimi Inari",
                    "kind": "sight",
                    "city": "Kyoto",
                    "address": "Fushimi",
                    "website": null,
                    "phone": null,
                    "openingHours": [],
                    "photoUrls": [],
                    "guide": null
                },
                "pitch": "Visit at dawn",
                "tags": ["must-see"]
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let candidate_id = candidate["id"].as_str().expect("candidate id");
    let place_id = candidate["placeId"].as_str().expect("place id");
    let (status, initial) = harness
        .json(
            Method::POST,
            "/trips/trip-a/plan",
            "member@example.test",
            Some(json!({"anchorPlaceId": place_id})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let day_id = initial["days"][0]["id"].as_str().expect("day id");
    let input = json!({
        "title": "Add Fushimi Inari",
        "rationale": "An early start avoids the crowds.",
        "changeSet": {
            "basePlanVersion": 1,
            "ops": [{
                "op": "add_stop",
                "dayId": day_id,
                "placeId": place_id,
                "seq": 1,
                "stopKind": "visit"
            }]
        },
        "route": "leader_approval"
    });

    let (status, body) = harness
        .json(
            Method::POST,
            "/trips/trip-a/proposals",
            "viewer@example.test",
            Some(input.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    let (status, pending) = harness
        .json(
            Method::POST,
            "/trips/trip-a/proposals",
            "member@example.test",
            Some(input),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(pending["tripId"], "trip-a");
    assert_eq!(pending["createdBy"], "member");
    assert_eq!(pending["source"], json!({"via": "web"}));
    assert_eq!(pending["status"], "pending");
    assert_eq!(pending["decidedBy"], Value::Null);
    assert_eq!(pending["rejectionReason"], Value::Null);
    let proposal_id = pending["id"].as_str().expect("proposal id");

    let (status, listed) = harness
        .json(
            Method::GET,
            "/trips/trip-a/proposals",
            "viewer@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "viewers may inspect governance");
    assert_eq!(listed.as_array().expect("proposal list").len(), 1);

    let approve_path = format!("/trips/trip-a/proposals/{proposal_id}/approve");
    let (status, _) = harness
        .json(Method::POST, &approve_path, "member@example.test", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, applied) = harness
        .json(Method::POST, &approve_path, "leader@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(applied["status"], "applied");
    assert_eq!(
        applied["decidedBy"],
        json!({"kind": "leader", "userId": "leader"})
    );
    let (_, current) = harness
        .json(
            Method::GET,
            "/trips/trip-a/plan",
            "viewer@example.test",
            None,
        )
        .await;
    assert_eq!(current["plan"]["version"], 2);
    assert_eq!(current["plan"]["createdFromProposalId"], proposal_id);
    assert_eq!(current["stops"][0]["placeId"], place_id);
    let (_, candidates) = harness
        .json(
            Method::GET,
            "/trips/trip-a/candidates",
            "viewer@example.test",
            None,
        )
        .await;
    let adopted = candidates
        .as_array()
        .expect("candidate list")
        .iter()
        .find(|candidate| candidate["id"] == candidate_id)
        .expect("candidate remains visible");
    assert_eq!(adopted["status"], "in_plan");

    let (status, repeated) = harness
        .json(Method::POST, &approve_path, "leader@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(repeated["status"], "applied");
    let (_, after_retry) = harness
        .json(
            Method::GET,
            "/trips/trip-a/plan",
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(after_retry["plan"]["version"], 2);

    let (status, _) = harness
        .json(
            Method::POST,
            &format!("/trips/trip-a/proposals/{proposal_id}/reject"),
            "leader@example.test",
            Some(json!({"reason": "Too late"})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn proposal_staleness_rejection_isolation_and_poll_routing_are_explicit() {
    let harness = Harness::new();
    let leader = harness.seed_user("leader", "leader@example.test").await;
    let member = harness.seed_user("member", "member@example.test").await;
    harness
        .seed_trip(vec![
            (&leader, TripRole::Leader),
            (&member, TripRole::Member),
        ])
        .await;
    let (_, candidate) = harness
        .json(
            Method::POST,
            "/trips/trip-a/candidates",
            "member@example.test",
            Some(json!({
                "sourcePlaceId": null,
                "place": {
                    "name": "Anchor",
                    "kind": "sight",
                    "city": "Kyoto",
                    "address": "",
                    "website": null,
                    "phone": null,
                    "openingHours": [],
                    "photoUrls": [],
                    "guide": null
                },
                "pitch": "Start here",
                "tags": []
            })),
        )
        .await;
    let (_, initial) = harness
        .json(
            Method::POST,
            "/trips/trip-a/plan",
            "member@example.test",
            Some(json!({"anchorPlaceId": candidate["placeId"]})),
        )
        .await;
    assert_eq!(initial["plan"]["version"], 1);

    let pending_input = |title: &str, route: &str| {
        json!({
            "title": title,
            "rationale": "Exercise lifecycle rules.",
            "changeSet": {
                "basePlanVersion": 1,
                "ops": [{"op": "add_day", "date": "2026-11-04", "cityHint": "Osaka"}]
            },
            "route": route
        })
    };
    let (status, pending) = harness
        .json(
            Method::POST,
            "/trips/trip-a/proposals",
            "member@example.test",
            Some(pending_input("Pending change", "leader_approval")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let stale_id = pending["id"].as_str().expect("proposal id");

    let (status, fast_path) = harness
        .json(
            Method::POST,
            "/trips/trip-a/proposals",
            "leader@example.test",
            Some(pending_input("Leader change", "leader_approval")),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(fast_path["status"], "applied");
    let (status, body) = harness
        .json(
            Method::POST,
            &format!("/trips/trip-a/proposals/{stale_id}/approve"),
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "conflict");
    let (_, proposals) = harness
        .json(
            Method::GET,
            "/trips/trip-a/proposals",
            "member@example.test",
            None,
        )
        .await;
    assert!(
        proposals
            .as_array()
            .expect("proposals")
            .iter()
            .any(|proposal| proposal["id"] == stale_id && proposal["status"] == "stale")
    );

    let before_poll = proposals.as_array().expect("proposals").len();
    let (status, poll_proposal) = harness
        .json(
            Method::POST,
            "/trips/trip-a/proposals",
            "member@example.test",
            Some(json!({
                "title": "Poll route",
                "rationale": "The proposal and its poll must be created together.",
                "changeSet": {
                    "basePlanVersion": 2,
                    "ops": [{"op": "add_day", "date": "2026-11-05", "cityHint": "Nara"}]
                },
                "route": "poll"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(poll_proposal["route"], "poll");
    assert_eq!(poll_proposal["status"], "pending");
    let poll_id = poll_proposal["decidedBy"]["pollId"]
        .as_str()
        .expect("server-owned poll id");
    let (_, after_poll) = harness
        .json(
            Method::GET,
            "/trips/trip-a/proposals",
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(
        after_poll.as_array().expect("proposals").len(),
        before_poll + 1
    );
    let (status, polls) = harness
        .json(
            Method::GET,
            "/trips/trip-a/polls",
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let routed_poll = polls
        .as_array()
        .expect("polls")
        .iter()
        .find(|poll| poll["id"] == poll_id)
        .expect("proposal poll");
    assert_eq!(routed_poll["kind"], "plan_change");
    assert_eq!(routed_poll["status"], "open");
    assert_eq!(routed_poll["quorum"], 1);
    let adopt_option_id = routed_poll["options"]
        .as_array()
        .and_then(|options| {
            options.iter().find(|option| {
                option["proposalId"] == poll_proposal["id"]
                    && option["label"] == "Adopt the proposed plan change"
            })
        })
        .and_then(|option| option["id"].as_str())
        .expect("server-owned adopt option id");

    let poll_proposal_id = poll_proposal["id"].as_str().expect("proposal id");
    let (status, body) = harness
        .json(
            Method::POST,
            &format!("/trips/trip-a/proposals/{poll_proposal_id}/approve"),
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "conflict");

    let vote_path = format!("/trips/trip-a/polls/{poll_id}/votes");
    let (status, voted) = harness
        .json(
            Method::POST,
            &vote_path,
            "member@example.test",
            Some(json!({"optionIds": [adopt_option_id]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(voted["votes"].as_array().expect("votes").len(), 1);
    let close_path = format!("/trips/trip-a/polls/{poll_id}/close");
    let (status, body) = harness
        .json(Method::POST, &close_path, "member@example.test", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");
    let (status, closed) = harness
        .json(Method::POST, &close_path, "leader@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(closed["status"], "passed");
    let (status, repeated) = harness
        .json(Method::POST, &close_path, "leader@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(repeated, closed);
    let (_, plan) = harness
        .json(
            Method::GET,
            "/trips/trip-a/plan",
            "member@example.test",
            None,
        )
        .await;
    assert_eq!(plan["plan"]["version"], 3);

    let (status, leader_routed) = harness
        .json(
            Method::POST,
            "/trips/trip-a/proposals",
            "member@example.test",
            Some(json!({
                "title": "Leader may send this to a poll",
                "rationale": "Exercise the bodyless transition contract.",
                "changeSet": {
                    "basePlanVersion": 3,
                    "ops": [{"op": "add_day", "date": "2026-11-07", "cityHint": "Kobe"}]
                },
                "route": "leader_approval"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let leader_routed_id = leader_routed["id"].as_str().expect("proposal id");
    let to_poll_path = format!("/trips/trip-a/proposals/{leader_routed_id}/to-poll");
    let (status, body) = harness
        .json(
            Method::POST,
            &to_poll_path,
            "leader@example.test",
            Some(json!({"option": "caller-controlled"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");
    let (status, body) = harness
        .json(
            Method::POST,
            &to_poll_path,
            "leader@example.test",
            Some(json!({"padding": "x".repeat(POLL_BODYLESS_LIMIT_BYTES)})),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"], "payload_too_large");
    let (status, leader_poll) = harness
        .json(Method::POST, &to_poll_path, "leader@example.test", None)
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(leader_poll["kind"], "plan_change");
    let (status, repeated_leader_poll) = harness
        .json(Method::POST, &to_poll_path, "leader@example.test", None)
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(repeated_leader_poll, leader_poll);

    let (status, rejected) = harness
        .json(
            Method::POST,
            "/trips/trip-a/proposals",
            "member@example.test",
            Some(json!({
                "title": "Reject me",
                "rationale": "Lifecycle coverage.",
                "changeSet": {
                    "basePlanVersion": 3,
                    "ops": [{"op": "add_day", "date": "2026-11-06", "cityHint": "Nara"}]
                },
                "route": "leader_approval"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let rejected_id = rejected["id"].as_str().expect("proposal id");
    let reject_path = format!("/trips/trip-a/proposals/{rejected_id}/reject");
    let (status, rejected) = harness
        .json(
            Method::POST,
            &reject_path,
            "leader@example.test",
            Some(json!({"reason": "  Keep the current route  "})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rejected["status"], "rejected");
    assert_eq!(rejected["rejectionReason"], "Keep the current route");
    let (status, repeated) = harness
        .json(
            Method::POST,
            &reject_path,
            "leader@example.test",
            Some(json!({"reason": "A retry cannot rewrite provenance"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(repeated["rejectionReason"], "Keep the current route");

    let (status, other_trip) = harness
        .json(
            Method::POST,
            "/trips",
            "leader@example.test",
            Some(json!({
                "name": "Other trip",
                "startDate": "2027-01-01",
                "endDate": "2027-01-02",
                "baseCurrency": "GBP"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let other_trip_id = other_trip["id"].as_str().expect("other trip id");
    let (status, body) = harness
        .json(
            Method::POST,
            &format!("/trips/{other_trip_id}/proposals/{rejected_id}/approve"),
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");
}

#[tokio::test]
async fn poll_http_contract_enforces_roles_quorum_idempotency_and_trip_isolation() {
    let harness = Harness::new();
    let leader = harness.seed_user("leader", "leader@example.test").await;
    let member_a = harness.seed_user("member-a", "member-a@example.test").await;
    let member_b = harness.seed_user("member-b", "member-b@example.test").await;
    let viewer = harness.seed_user("viewer", "viewer@example.test").await;
    harness
        .seed_trip(vec![
            (&leader, TripRole::Leader),
            (&member_a, TripRole::Member),
            (&member_b, TripRole::Member),
            (&viewer, TripRole::Viewer),
        ])
        .await;

    let (status, empty) = harness
        .json(
            Method::GET,
            "/trips/trip-a/polls",
            "viewer@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(empty, json!([]));
    let (status, body) = harness
        .json(
            Method::GET,
            "/trips/trip-a/polls",
            "viewer@example.test",
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");

    let create = json!({
        "kind": "decision",
        "title": "Dinner",
        "description": "Pick one.",
        "options": [{"label": "Ramen"}, {"label": "Sushi"}],
        "closesAt": "2026-08-10T12:00:00.000Z",
        "allowMulti": false
    });
    let (status, body) = harness
        .json(
            Method::POST,
            "/trips/trip-a/polls",
            "member-a@example.test",
            Some(json!({"padding": "x".repeat(POLL_CREATE_BODY_LIMIT_BYTES)})),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"], "payload_too_large");
    let (status, body) = harness
        .json(
            Method::POST,
            "/trips/trip-a/polls",
            "viewer@example.test",
            Some(create.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    let (status, poll) = harness
        .json(
            Method::POST,
            "/trips/trip-a/polls",
            "member-a@example.test",
            Some(create),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(poll["kind"], "decision");
    assert_eq!(poll["status"], "open");
    assert_eq!(poll["createdBy"], "member-a");
    assert_eq!(poll["quorum"], 2, "viewers do not increase quorum");
    assert_eq!(poll["decidedAt"], Value::Null);
    assert_eq!(poll["resolutionNote"], Value::Null);
    let poll_id = poll["id"].as_str().expect("poll id");
    let options = poll["options"].as_array().expect("options");
    let ramen = options[0]["id"].as_str().expect("option id");

    let vote_path = format!("/trips/trip-a/polls/{poll_id}/votes");
    let (status, body) = harness
        .json(
            Method::POST,
            &vote_path,
            "member-a@example.test",
            Some(json!({"optionIds": ["x".repeat(POLL_VOTE_BODY_LIMIT_BYTES)]})),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"], "payload_too_large");
    let (status, body) = harness
        .json(
            Method::POST,
            &vote_path,
            "member-a@example.test",
            Some(json!({"optionIds": [ramen, ramen]})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");
    let (status, body) = harness
        .json(
            Method::POST,
            &vote_path,
            "member-a@example.test",
            Some(json!({"optionIds": [ramen], "userId": "viewer"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");
    let (status, body) = harness
        .json(
            Method::POST,
            &vote_path,
            "viewer@example.test",
            Some(json!({"optionIds": [ramen]})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    let (status, first_vote) = harness
        .json(
            Method::POST,
            &vote_path,
            "member-a@example.test",
            Some(json!({"optionIds": [ramen]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, repeated_vote) = harness
        .json(
            Method::POST,
            &vote_path,
            "member-a@example.test",
            Some(json!({"optionIds": [ramen]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(repeated_vote, first_vote);
    let (status, second_vote) = harness
        .json(
            Method::POST,
            &vote_path,
            "member-b@example.test",
            Some(json!({"optionIds": [ramen]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second_vote["votes"].as_array().expect("votes").len(), 2);

    let open_path = format!("/trips/trip-a/polls/{poll_id}/open");
    let (status, body) = harness
        .json(Method::POST, &open_path, "member-b@example.test", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");
    let (status, body) = harness
        .json(
            Method::POST,
            &open_path,
            "leader@example.test",
            Some(json!({"pollId": poll_id})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");
    let (status, body) = harness
        .json(
            Method::POST,
            &open_path,
            "leader@example.test",
            Some(json!({"padding": "x".repeat(POLL_BODYLESS_LIMIT_BYTES)})),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"], "payload_too_large");
    let (status, opened) = harness
        .json(Method::POST, &open_path, "leader@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(opened["status"], "open");

    let close_path = format!("/trips/trip-a/polls/{poll_id}/close");
    let (status, body) = harness
        .json(Method::POST, &close_path, "member-a@example.test", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");
    let (status, body) = harness
        .json(
            Method::POST,
            &close_path,
            "leader@example.test",
            Some(json!({"winner": ramen})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");
    let (status, body) = harness
        .json(
            Method::POST,
            &close_path,
            "leader@example.test",
            Some(json!({"padding": "x".repeat(POLL_BODYLESS_LIMIT_BYTES)})),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"], "payload_too_large");
    let (status, closed) = harness
        .json(Method::POST, &close_path, "leader@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(closed["status"], "passed");
    assert_eq!(closed["decidedAt"], NOW);
    let (status, repeated_close) = harness
        .json(Method::POST, &close_path, "leader@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(repeated_close, closed);
    let (status, body) = harness
        .json(
            Method::POST,
            &vote_path,
            "member-a@example.test",
            Some(json!({"optionIds": [options[1]["id"]]})),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"], "conflict");

    let (status, body) = harness
        .json(
            Method::POST,
            "/trips/trip-a/polls",
            "member-a@example.test",
            Some(json!({
                "kind": "plan_change",
                "title": "Forged link",
                "description": "",
                "options": [
                    {"label": "Apply", "proposalId": "foreign"},
                    {"label": "Keep"}
                ],
                "closesAt": "2026-08-10T12:00:00.000Z",
                "allowMulti": false
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");

    let (status, tied_poll) = harness
        .json(
            Method::POST,
            "/trips/trip-a/polls",
            "leader@example.test",
            Some(json!({
                "kind": "decision",
                "title": "Tie",
                "description": "No storage-order winner.",
                "options": [{"label": "A"}, {"label": "B"}],
                "closesAt": "2026-08-10T12:00:00.000Z",
                "allowMulti": false
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let tied_id = tied_poll["id"].as_str().expect("poll id");
    let tied_options = tied_poll["options"].as_array().expect("options");
    for (email, option) in [
        ("member-a@example.test", &tied_options[0]["id"]),
        ("member-b@example.test", &tied_options[1]["id"]),
    ] {
        let (status, _) = harness
            .json(
                Method::POST,
                &format!("/trips/trip-a/polls/{tied_id}/votes"),
                email,
                Some(json!({"optionIds": [option]})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, tied) = harness
        .json(
            Method::POST,
            &format!("/trips/trip-a/polls/{tied_id}/close"),
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(tied["status"], "failed");
    assert!(
        tied["resolutionNote"]
            .as_str()
            .is_some_and(|note| note.contains("tied"))
    );

    let (status, no_vote_poll) = harness
        .json(
            Method::POST,
            "/trips/trip-a/polls",
            "member-a@example.test",
            Some(json!({
                "kind": "decision",
                "title": "No participation",
                "description": "",
                "options": [{"label": "A"}, {"label": "B"}],
                "closesAt": "2026-08-10T12:00:00.000Z",
                "allowMulti": true
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let no_vote_id = no_vote_poll["id"].as_str().expect("poll id");
    let (status, expired) = harness
        .json(
            Method::POST,
            &format!("/trips/trip-a/polls/{no_vote_id}/close"),
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(expired["status"], "expired");

    let (status, other_trip) = harness
        .json(
            Method::POST,
            "/trips",
            "leader@example.test",
            Some(json!({
                "name": "Other trip",
                "startDate": "2027-01-01",
                "endDate": "2027-01-02",
                "baseCurrency": "GBP"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let other_trip_id = other_trip["id"].as_str().expect("other trip id");
    let (status, body) = harness
        .json(
            Method::POST,
            &format!("/trips/{other_trip_id}/polls/{poll_id}/close"),
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");
}

#[tokio::test]
async fn proposal_requests_reject_unknown_fields_and_foreign_plan_children_before_writing() {
    let harness = Harness::new();
    let leader = harness.seed_user("leader", "leader@example.test").await;
    harness.seed_trip(vec![(&leader, TripRole::Leader)]).await;
    let (_, candidate) = harness
        .json(
            Method::POST,
            "/trips/trip-a/candidates",
            "leader@example.test",
            Some(json!({
                "sourcePlaceId": null,
                "place": {
                    "name": "Anchor",
                    "kind": "sight",
                    "city": "Kyoto",
                    "address": "",
                    "website": null,
                    "phone": null,
                    "openingHours": [],
                    "photoUrls": [],
                    "guide": null
                },
                "pitch": "Start here",
                "tags": []
            })),
        )
        .await;
    let _ = harness
        .json(
            Method::POST,
            "/trips/trip-a/plan",
            "leader@example.test",
            Some(json!({"anchorPlaceId": candidate["placeId"]})),
        )
        .await;
    let malformed = json!({
        "title": "Forged change",
        "rationale": "Must fail.",
        "changeSet": {
            "basePlanVersion": 1,
            "ops": [{
                "op": "remove_stop",
                "stopId": "missing-stop",
                "replacementPlanId": "caller-owned"
            }]
        },
        "route": "leader_approval"
    });
    let (status, body) = harness
        .json(
            Method::POST,
            "/trips/trip-a/proposals",
            "leader@example.test",
            Some(malformed),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");

    let (status, body) = harness
        .json(
            Method::POST,
            "/trips/trip-a/proposals",
            "leader@example.test",
            Some(json!({
                "title": "Foreign child",
                "rationale": "Must fail before proposal or plan writes.",
                "changeSet": {
                    "basePlanVersion": 1,
                    "ops": [{"op": "remove_stop", "stopId": "foreign-stop"}]
                },
                "route": "leader_approval"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");
    let (status, body) = harness
        .json(
            Method::POST,
            "/trips/trip-a/proposals",
            "leader@example.test",
            Some(json!({
                "title": "Foreign child via poll",
                "rationale": "Must fail before both proposal and poll writes.",
                "changeSet": {
                    "basePlanVersion": 1,
                    "ops": [{"op": "remove_stop", "stopId": "foreign-stop"}]
                },
                "route": "poll"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");
    let (_, proposals) = harness
        .json(
            Method::GET,
            "/trips/trip-a/proposals",
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(proposals, json!([]));
    let (_, polls) = harness
        .json(
            Method::GET,
            "/trips/trip-a/polls",
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(polls, json!([]));
    let (_, current) = harness
        .json(
            Method::GET,
            "/trips/trip-a/plan",
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(current["plan"]["version"], 1);
}

#[tokio::test]
async fn malformed_and_unknown_request_fields_use_the_json_error_contract() {
    let harness = Harness::new();
    let (status, body) = harness
        .json(
            Method::POST,
            "/trips",
            "leader@example.test",
            Some(json!({
                "name": "Japan",
                "startDate": "2026-11-03",
                "endDate": "2026-11-01",
                "baseCurrency": "GBP",
                "ownerId": "forged"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");
    assert!(body["message"].is_string());

    let candidate_place = json!({
        "name": "Temple",
        "kind": "sight",
        "city": "Kyoto",
        "address": "",
        "website": null,
        "phone": null,
        "openingHours": [],
        "photoUrls": [],
        "guide": {
            "summary": "Summary",
            "intro": "Intro",
            "activityIdeas": [{"title": "Walk"}],
            "practicalTips": []
        }
    });
    let candidate_update = |place| json!({"place": place, "pitch": "Visit", "tags": []});
    let mut missing_website = candidate_place.clone();
    missing_website
        .as_object_mut()
        .expect("place object")
        .remove("website");
    let mut missing_phone = candidate_place.clone();
    missing_phone
        .as_object_mut()
        .expect("place object")
        .remove("phone");
    let mut missing_guide = candidate_place.clone();
    missing_guide
        .as_object_mut()
        .expect("place object")
        .remove("guide");
    let mut null_activity_details = candidate_place.clone();
    null_activity_details["guide"]["activityIdeas"][0]["details"] = Value::Null;

    for (method, path, patch) in [
        (
            Method::PATCH,
            "/trips/missing/days/day-a",
            json!({"cityHint": null, "windowStart": "10:00"}),
        ),
        (
            Method::PATCH,
            "/trips/missing/stops/stop-null",
            json!({"plannedArrival": null, "notes": "keep"}),
        ),
        (
            Method::PATCH,
            "/trips/missing/stops/booking-url",
            json!({"booking": {"ref": "ABC", "cost": null}}),
        ),
        (
            Method::PATCH,
            "/trips/missing/stops/booking-cost",
            json!({"booking": {"ref": "ABC", "url": null}}),
        ),
        (
            Method::PATCH,
            "/trips/missing/stops/booking-ledger",
            json!({"booking": {"ref": "ABC", "url": null, "cost": null, "ledgerEntryId": "forged"}}),
        ),
        (
            Method::PATCH,
            "/trips/missing/candidates/missing-website",
            candidate_update(missing_website),
        ),
        (
            Method::PATCH,
            "/trips/missing/candidates/missing-phone",
            candidate_update(missing_phone),
        ),
        (
            Method::PATCH,
            "/trips/missing/candidates/missing-guide",
            candidate_update(missing_guide),
        ),
        (
            Method::PATCH,
            "/trips/missing/candidates/null-details",
            candidate_update(null_activity_details),
        ),
        (
            Method::POST,
            "/trips/missing/candidates",
            json!({"place": candidate_place, "pitch": "Visit", "tags": []}),
        ),
    ] {
        let (status, body) = harness
            .json(method, path, "leader@example.test", Some(patch))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{path}: {body}");
        assert_eq!(body["error"], "invalid_request");
    }
}

#[tokio::test]
async fn history_and_revert_http_contract_enforces_roles_isolation_and_bodyless_idempotency() {
    let harness = Harness::new();
    let leader = harness.seed_user("leader", "leader@example.test").await;
    let member = harness.seed_user("member", "member@example.test").await;
    let viewer = harness.seed_user("viewer", "viewer@example.test").await;
    harness
        .seed_trip(vec![
            (&leader, TripRole::Leader),
            (&member, TripRole::Member),
            (&viewer, TripRole::Viewer),
        ])
        .await;

    let (status, trip) = harness
        .json(
            Method::PATCH,
            "/trips/trip-a/status",
            "leader@example.test",
            Some(json!({"status": "booked"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(trip["status"], "booked");

    let (status, history) = harness
        .json(
            Method::GET,
            "/trips/trip-a/history",
            "viewer@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "viewers may read history");
    let history = history.as_array().expect("history array");
    assert_eq!(history.len(), 1);
    let edit_id = history[0]["id"]
        .as_str()
        .expect("server edit id")
        .to_string();
    assert_eq!(history[0]["tripId"], "trip-a");
    assert_eq!(history[0]["entity"], "trip");
    assert_eq!(history[0]["entityId"], "trip-a");
    assert_eq!(history[0]["field"], "status");
    assert_eq!(history[0]["oldValue"], "dreaming");
    assert_eq!(history[0]["newValue"], "booked");
    assert_eq!(history[0]["status"], "applied");
    assert_eq!(history[0]["revertedBy"], Value::Null);
    assert_eq!(history[0]["revertedAt"], Value::Null);
    assert_eq!(history[0]["revertEditId"], Value::Null);
    assert_eq!(history[0]["revertsEditId"], Value::Null);

    let revert_path = format!("/trips/trip-a/edits/{edit_id}/revert");
    let (status, body) = harness
        .json(
            Method::POST,
            &revert_path,
            "member@example.test",
            Some(json!({
                "entity": "trip",
                "field": "status",
                "oldValue": "done",
                "replacementValue": "dreaming"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_request");
    let (status, body) = harness
        .json(
            Method::POST,
            &revert_path,
            "member@example.test",
            Some(json!({"padding": "x".repeat(REVERT_BODY_LIMIT_BYTES + 1)})),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "streamed bodies are bounded before buffering the whole request"
    );
    assert_eq!(body["error"], "payload_too_large");
    let (_, still_booked) = harness
        .json(Method::GET, "/trips/trip-a", "leader@example.test", None)
        .await;
    assert_eq!(still_booked["status"], "booked");

    let (status, body) = harness
        .json(Method::POST, &revert_path, "viewer@example.test", None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    let (status, body) = harness
        .json(Method::POST, &revert_path, "member@example.test", None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(body, Value::Null);

    let (_, reverted_trip) = harness
        .json(Method::GET, "/trips/trip-a", "viewer@example.test", None)
        .await;
    assert_eq!(reverted_trip["status"], "dreaming");
    let (_, history) = harness
        .json(
            Method::GET,
            "/trips/trip-a/history",
            "leader@example.test",
            None,
        )
        .await;
    let history = history.as_array().expect("history array");
    assert_eq!(history.len(), 2, "revert appends one compensating edit");
    let original = history
        .iter()
        .find(|edit| edit["id"] == edit_id)
        .expect("original retained");
    let compensation = history
        .iter()
        .find(|edit| edit["revertsEditId"] == edit_id)
        .expect("compensation linked to original");
    assert_eq!(original["status"], "reverted");
    assert_eq!(original["revertedBy"], "member");
    assert_eq!(original["revertedAt"], NOW);
    assert_eq!(original["revertEditId"], compensation["id"]);
    assert_eq!(compensation["status"], "applied");
    assert_eq!(compensation["oldValue"], "booked");
    assert_eq!(compensation["newValue"], "dreaming");

    let (status, _) = harness
        .json(Method::POST, &revert_path, "leader@example.test", None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "repeat is idempotent");
    let (_, repeated_history) = harness
        .json(
            Method::GET,
            "/trips/trip-a/history",
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(repeated_history.as_array().expect("history").len(), 2);

    let (status, other_trip) = harness
        .json(
            Method::POST,
            "/trips",
            "leader@example.test",
            Some(json!({
                "name": "Other trip",
                "startDate": "2027-01-01",
                "endDate": "2027-01-02",
                "baseCurrency": "GBP"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let other_trip_id = other_trip["id"].as_str().expect("other trip id");
    let (status, body) = harness
        .json(
            Method::POST,
            &format!("/trips/{other_trip_id}/edits/{edit_id}/revert"),
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");
}

#[tokio::test]
async fn discussion_http_contract_enforces_roles_isolation_limits_and_idempotent_reactions() {
    let harness = Harness::new();
    let leader = harness.seed_user("leader", "leader@example.test").await;
    let member = harness.seed_user("member", "member@example.test").await;
    let viewer = harness.seed_user("viewer", "viewer@example.test").await;
    harness
        .seed_trip(vec![
            (&leader, TripRole::Leader),
            (&member, TripRole::Member),
            (&viewer, TripRole::Viewer),
        ])
        .await;

    let threads_path = "/trips/trip-a/threads";
    let (status, threads) = harness
        .json(Method::GET, threads_path, "viewer@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(threads, json!([]));

    let create_body = json!({
        "anchor": {"kind": "trip"},
        "title": "  General planning  ",
        "body": "  First thought  "
    });
    let (status, body) = harness
        .json(
            Method::POST,
            threads_path,
            "viewer@example.test",
            Some(create_body.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error"], "forbidden");

    let (status, thread) = harness
        .json(
            Method::POST,
            threads_path,
            "leader@example.test",
            Some(create_body.clone()),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{thread}");
    assert_eq!(thread["tripId"], "trip-a");
    assert_eq!(thread["anchor"], json!({"kind": "trip"}));
    assert_eq!(thread["title"], "General planning");
    assert_eq!(thread["commentCount"], 1);
    assert_eq!(thread["lastActivityAt"], NOW);
    let thread_id = thread["id"].as_str().expect("thread id").to_string();

    let (status, body) = harness
        .json(
            Method::POST,
            threads_path,
            "member@example.test",
            Some(create_body),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "one thread owns each anchor");
    assert_eq!(body["error"], "conflict");

    let comments_path = format!("/trips/trip-a/threads/{thread_id}/comments");
    let (status, comments) = harness
        .json(Method::GET, &comments_path, "viewer@example.test", None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let comments = comments.as_array().expect("comments");
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0]["body"], "First thought");
    assert_eq!(comments[0]["author"], "leader");
    let first_comment_id = comments[0]["id"]
        .as_str()
        .expect("first comment id")
        .to_string();

    let (status, _) = harness
        .json(
            Method::POST,
            &comments_path,
            "viewer@example.test",
            Some(json!({"body": "No write"})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, comment) = harness
        .json(
            Method::POST,
            &comments_path,
            "member@example.test",
            Some(json!({"body": "  Second thought  "})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(comment["body"], "Second thought");
    assert_eq!(comment["author"], "member");

    let reactions_path =
        format!("/trips/trip-a/threads/{thread_id}/comments/{first_comment_id}/reactions");
    for _ in 0..2 {
        let (status, reacted) = harness
            .json(
                Method::POST,
                &reactions_path,
                "member@example.test",
                Some(json!({"emoji": "👍", "active": true})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            reacted["reactions"],
            json!([{"emoji": "👍", "userIds": ["member"]}])
        );
    }
    let (status, reacted) = harness
        .json(
            Method::POST,
            &reactions_path,
            "member@example.test",
            Some(json!({"emoji": "👍", "active": false})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reacted["reactions"], json!([]));
    let (status, _) = harness
        .json(
            Method::POST,
            &reactions_path,
            "viewer@example.test",
            Some(json!({"emoji": "👍", "active": true})),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, other_trip) = harness
        .json(
            Method::POST,
            "/trips",
            "leader@example.test",
            Some(json!({
                "name": "Other",
                "startDate": "2027-01-01",
                "endDate": "2027-01-02",
                "baseCurrency": "GBP"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let other_trip_id = other_trip["id"].as_str().expect("other trip id");
    let (status, body) = harness
        .json(
            Method::GET,
            &format!("/trips/{other_trip_id}/threads/{thread_id}/comments"),
            "leader@example.test",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], "not_found");

    for malformed in [
        json!({"anchor": {"kind": "trip", "stopId": "forged"}, "title": "x", "body": "y"}),
        json!({"anchor": {"kind": "stop", "stopId": "missing"}, "title": "x", "body": "y"}),
        json!({"anchor": {"kind": "trip"}, "title": "", "body": "y"}),
    ] {
        let (status, _) = harness
            .json(
                Method::POST,
                threads_path,
                "leader@example.test",
                Some(malformed),
            )
            .await;
        assert!(
            matches!(status, StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND),
            "unexpected malformed-anchor status {status}"
        );
    }

    let (status, _) = harness
        .json(
            Method::GET,
            threads_path,
            "leader@example.test",
            Some(json!({"unexpected": true})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = harness
        .json(
            Method::GET,
            threads_path,
            "leader@example.test",
            Some(json!({"padding": "x".repeat(DISCUSSION_BODYLESS_LIMIT_BYTES + 1)})),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _) = harness
        .json(
            Method::POST,
            threads_path,
            "leader@example.test",
            Some(json!({"padding": "x".repeat(DISCUSSION_WRITE_BODY_LIMIT_BYTES + 1)})),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, _) = harness
        .json(
            Method::POST,
            &reactions_path,
            "member@example.test",
            Some(json!({"padding": "x".repeat(DISCUSSION_REACTION_BODY_LIMIT_BYTES + 1)})),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
}
