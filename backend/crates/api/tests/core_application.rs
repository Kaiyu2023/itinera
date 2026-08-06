mod support;
#[path = "support/user_repo.rs"]
mod user_repo;

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

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
        trip::{Trip, TripMember, TripRole, TripStatus},
        user::{Email, User, UserId},
    },
    ports::{
        auth::{AuthError, Identity, IdentityProvider},
        clock::Clock,
        id_gen::IdGen,
        trip::TripRepo,
        user::UserRepo,
    },
};
use serde_json::{Value, json};
use tower::ServiceExt;

use support::trip_repo::TestTripRepo;
use user_repo::TestUserRepo;

const ASSERTION_HEADER: &str = "cf-access-jwt-assertion";
const NOW: &str = "2026-08-05T12:00:00.000Z";

struct EmailIdentityProvider;

#[async_trait]
impl IdentityProvider for EmailIdentityProvider {
    async fn authenticate(&self, assertion: &str) -> Result<Identity, AuthError> {
        Ok(Identity {
            email: Email::parse(assertion).map_err(|_| AuthError::InvalidToken)?,
        })
    }
}

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> String {
        NOW.to_string()
    }
}

struct SequenceIds(Mutex<VecDeque<String>>);

impl SequenceIds {
    fn new() -> Self {
        Self(Mutex::new(
            (1..=200)
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
        let users = Arc::new(TestUserRepo::new());
        let trips = Arc::new(TestTripRepo::new());
        let app = create_app(AppState {
            identity: Arc::new(EmailIdentityProvider),
            users: users.clone(),
            trips: trips.clone(),
            access_policy: Arc::new(DevAccessPolicy),
            place_catalog: Arc::new(EmptyPlaceCatalog),
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
        let trip = Trip {
            id: "trip-a".into(),
            name: "Japan".into(),
            cover_photo_url: None,
            accent_color: None,
            stop_kind_labels: None,
            status: TripStatus::Dreaming,
            start_date: "2026-11-01".into(),
            end_date: "2026-11-03".into(),
            base_currency: "GBP".into(),
            soft_budget: None,
            members: members
                .into_iter()
                .map(|(user, role)| TripMember {
                    user_id: user.id.0.clone(),
                    role,
                    joined_at: NOW.into(),
                })
                .collect(),
            current_plan_id: None,
            created_at: NOW.into(),
        };
        self.trips
            .create_trip(trip.clone())
            .await
            .expect("seed trip");
        trip
    }

    async fn json(
        &self,
        method: Method,
        path: &str,
        email: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(ASSERTION_HEADER, email);
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
            json!({"booking": {"ref": "ABC", "cost": null, "ledgerEntryId": null}}),
        ),
        (
            Method::PATCH,
            "/trips/missing/stops/booking-cost",
            json!({"booking": {"ref": "ABC", "url": null, "ledgerEntryId": null}}),
        ),
        (
            Method::PATCH,
            "/trips/missing/stops/booking-ledger",
            json!({"booking": {"ref": "ABC", "url": null, "cost": null}}),
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
