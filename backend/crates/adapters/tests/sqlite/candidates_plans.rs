use std::{sync::Arc, time::Duration};

use itinera_core::{
    domain::{
        trip::{
            Candidate, CandidateStatus, CandidateWithPlace, Day, Place, PlaceActivityIdea,
            PlaceGuide, PlaceKind, Plan, PlanDetail, TripRole,
        },
        user::User,
    },
    ports::{
        authorization::TripAuthorizationContext,
        trip::{TripRepo, TripRepoError},
    },
};
use sqlx::{Connection, Sqlite, Transaction};
use tokio::sync::Barrier;

use super::support::{NOW, TestDatabase, raw_connection, seed_trip, seed_user};

const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

fn human(user: &User) -> TripAuthorizationContext {
    TripAuthorizationContext::human(user.id.clone())
}

fn place(id: &str, city: &str) -> Place {
    Place {
        id: id.into(),
        name: format!("Place {id}"),
        kind: PlaceKind::Sight,
        lat: 0.0,
        lng: 0.0,
        tz: "UTC".into(),
        country_code: String::new(),
        admin_area: String::new(),
        city: city.into(),
        address: "1 Test Street".into(),
        external_ref: None,
        website: Some("https://example.test/place".into()),
        phone: None,
        rating: None,
        price_level: None,
        opening_hours: None,
        photo_urls: Vec::new(),
        guide: None,
    }
}

fn candidate(
    id: &str,
    trip_id: &str,
    place_id: &str,
    proposed_by: &User,
    source_place_id: Option<&str>,
    created_at: &str,
) -> Candidate {
    Candidate {
        id: id.into(),
        trip_id: trip_id.into(),
        source_place_id: source_place_id.map(str::to_string),
        place_id: place_id.into(),
        proposed_by: proposed_by.id.0.clone(),
        created_at: created_at.into(),
        pitch: format!("Pitch for {id}"),
        tags: vec!["quiet".into()],
        status: CandidateStatus::Shortlisted,
    }
}

fn plan(id: &str, trip_id: &str) -> Plan {
    Plan {
        id: id.into(),
        trip_id: trip_id.into(),
        version: 1,
        created_from_proposal_id: None,
        created_at: NOW.into(),
    }
}

fn bootstrap_days(plan_id: &str, city: &str, timezone: &str) -> Vec<Day> {
    ["2026-08-07", "2026-08-08", "2026-08-09"]
        .into_iter()
        .enumerate()
        .map(|(index, date)| Day {
            id: format!("day-{}", index + 1),
            plan_id: plan_id.into(),
            date: date.into(),
            city_hint: city.into(),
            tz: timezone.into(),
            window_start: "09:00".into(),
            window_end: "21:00".into(),
        })
        .collect()
}

fn minimal_guide() -> PlaceGuide {
    PlaceGuide {
        summary: "s".into(),
        intro: "i".into(),
        activity_ideas: Vec::new(),
        practical_tips: Vec::new(),
    }
}

fn exact_utf8_payload(byte_len: usize) -> String {
    let mut value = "🦀".repeat(byte_len / 4);
    match byte_len % 4 {
        0 => {}
        1 => value.push('a'),
        2 => value.push('¢'),
        3 => value.push('€'),
        _ => unreachable!(),
    }
    assert_eq!(value.len(), byte_len);
    value
}

fn boundary_candidates(proposed_by: &User) -> Vec<CandidateWithPlace> {
    (0..1_000)
        .map(|index| {
            let mut owned_place = place(&format!("candidate-place-{index:04}"), "Kyoto");
            owned_place.website = None;
            owned_place.guide = Some(minimal_guide());
            CandidateWithPlace {
                candidate: Candidate {
                    id: format!("candidate-{index:04}"),
                    trip_id: "trip-a".into(),
                    source_place_id: None,
                    place_id: owned_place.id.clone(),
                    proposed_by: proposed_by.id.0.clone(),
                    created_at: NOW.into(),
                    pitch: "p".into(),
                    tags: Vec::new(),
                    status: CandidateStatus::Shortlisted,
                },
                place: owned_place,
            }
        })
        .collect()
}

fn pad_candidate_collection_to_limit(candidates: &mut [CandidateWithPlace]) {
    let base_size = serde_json::to_vec(candidates).unwrap().len();
    assert!(base_size < MAX_RESPONSE_BYTES);
    let mut remaining = MAX_RESPONSE_BYTES - base_size;
    for candidate in candidates {
        let intro = &mut candidate.place.guide.as_mut().unwrap().intro;
        let capacity = (4_000 - intro.chars().count()) * 4;
        let addition = remaining.min(capacity);
        intro.push_str(&exact_utf8_payload(addition));
        remaining -= addition;
        if remaining == 0 {
            break;
        }
    }
    assert_eq!(remaining, 0, "candidate value bounds must reach 4 MiB");
}

fn boundary_saved_places() -> Vec<Place> {
    (0..100)
        .map(|index| {
            let mut saved = place(&format!("saved-place-{index:03}"), "Kyoto");
            saved.name = format!("Saved {index:03}");
            saved.website = None;
            saved.guide = Some(PlaceGuide {
                summary: "s".into(),
                intro: "i".into(),
                activity_ideas: vec![PlaceActivityIdea {
                    title: "a".into(),
                    details: None,
                }],
                practical_tips: (0..30).map(|_| "t".to_string()).collect(),
            });
            saved
        })
        .collect()
}

fn pad_place_collection_to_limit(places: &mut [Place]) {
    let base_size = serde_json::to_vec(&*places).unwrap().len();
    assert!(base_size < MAX_RESPONSE_BYTES);
    let mut remaining = MAX_RESPONSE_BYTES - base_size;
    for place in places {
        for tip in &mut place.guide.as_mut().unwrap().practical_tips {
            let capacity = (500 - tip.chars().count()) * 4;
            let addition = remaining.min(capacity);
            tip.push_str(&exact_utf8_payload(addition));
            remaining -= addition;
            if remaining == 0 {
                break;
            }
        }
        if remaining == 0 {
            break;
        }
    }
    assert_eq!(remaining, 0, "saved-place value bounds must reach 4 MiB");
}

fn pad_plan_detail_to_limit(detail: &mut PlanDetail) {
    let base_size = serde_json::to_vec(&*detail).unwrap().len();
    assert!(base_size < MAX_RESPONSE_BYTES);
    let mut remaining = MAX_RESPONSE_BYTES - base_size;
    for place in &mut detail.places {
        for tip in &mut place.guide.as_mut().unwrap().practical_tips {
            let capacity = (500 - tip.chars().count()) * 4;
            let addition = remaining.min(capacity);
            tip.push_str(&exact_utf8_payload(addition));
            remaining -= addition;
            if remaining == 0 {
                break;
            }
        }
        if remaining == 0 {
            break;
        }
    }
    assert_eq!(remaining, 0, "plan-place value bounds must reach 4 MiB");
}

#[tokio::test]
async fn candidate_repository_enforces_roles_isolation_and_same_trip_source_lineage() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    let viewer = seed_user(&users, "viewer", "viewer@example.com").await;
    let outsider = seed_user(&users, "outsider", "outsider@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    seed_trip(&trips, "trip-b", &outsider).await;
    add_membership(&database, "trip-a", &member, TripRole::Member).await;
    add_membership(&database, "trip-a", &viewer, TripRole::Viewer).await;

    let first_place = place("place-a", "Kyoto");
    let mut nonneutral_manual_place = place("spoofed-manual-place", "Kyoto");
    nonneutral_manual_place.lat = 35.0;
    assert!(matches!(
        trips
            .add_candidate(
                "trip-a",
                &human(&leader),
                candidate(
                    "spoofed-manual-candidate",
                    "trip-a",
                    &nonneutral_manual_place.id,
                    &leader,
                    None,
                    NOW,
                ),
                nonneutral_manual_place,
            )
            .await,
        Err(TripRepoError::CorruptData)
    ));
    let first = candidate(
        "candidate-a",
        "trip-a",
        &first_place.id,
        &member,
        None,
        "2026-08-07T12:00:00Z",
    );
    let stored = trips
        .add_candidate(
            "trip-a",
            &human(&member),
            first.clone(),
            first_place.clone(),
        )
        .await
        .expect("member adds a candidate");
    assert_eq!(stored.candidate, first);
    assert_eq!(stored.place, first_place);

    let mut copied_place = place("place-b", "Kyoto");
    copied_place.name = "Member-authored name".into();
    copied_place.address = "Different authored address".into();
    let copied = candidate(
        "candidate-b",
        "trip-a",
        &copied_place.id,
        &leader,
        Some("place-a"),
        "2026-08-07T12:01:00Z",
    );
    trips
        .add_candidate("trip-a", &human(&leader), copied, copied_place)
        .await
        .expect("same-trip snapshot can be copied without changing provider facts");
    let mut mismatched_copy = place("place-c", "Kyoto");
    mismatched_copy.lat = 34.0;
    assert!(matches!(
        trips
            .add_candidate(
                "trip-a",
                &human(&leader),
                candidate(
                    "candidate-c",
                    "trip-a",
                    &mismatched_copy.id,
                    &leader,
                    Some("place-a"),
                    NOW,
                ),
                mismatched_copy,
            )
            .await,
        Err(TripRepoError::CorruptData)
    ));

    let foreign_place = place("foreign-place", "Osaka");
    trips
        .add_candidate(
            "trip-b",
            &human(&outsider),
            candidate(
                "foreign-candidate",
                "trip-b",
                &foreign_place.id,
                &outsider,
                None,
                NOW,
            ),
            foreign_place,
        )
        .await
        .unwrap();

    let listed = trips
        .list_candidates("trip-a", &human(&viewer))
        .await
        .expect("viewers may read candidates");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].candidate.id, "candidate-a");
    assert_eq!(
        listed[1].candidate.source_place_id.as_deref(),
        Some("place-a")
    );
    assert_eq!(
        trips
            .find_place("trip-a", &human(&viewer), "place-a")
            .await
            .unwrap(),
        Some(listed[0].place.clone())
    );
    assert_eq!(
        trips
            .find_place("trip-a", &human(&viewer), "foreign-place")
            .await
            .unwrap(),
        None
    );
    sqlx::query("UPDATE trip_places SET lat = 34.0 WHERE trip_id = 'trip-a' AND id = 'place-a'")
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        trips.list_candidates("trip-a", &human(&viewer)).await,
        Err(TripRepoError::CorruptData)
    ));
    sqlx::query("UPDATE trip_places SET lat = 0.0 WHERE trip_id = 'trip-a' AND id = 'place-a'")
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        trips.list_candidates("trip-a", &human(&outsider)).await,
        Err(TripRepoError::NotFound)
    ));
    assert!(matches!(
        trips
            .list_candidates(
                "trip-a",
                &TripAuthorizationContext::service(leader.id.clone(), "service-a".into()),
            )
            .await,
        Err(TripRepoError::Forbidden)
    ));

    let rejected_place = place("viewer-place", "Kyoto");
    assert!(matches!(
        trips
            .add_candidate(
                "trip-a",
                &human(&viewer),
                candidate(
                    "viewer-candidate",
                    "trip-a",
                    &rejected_place.id,
                    &viewer,
                    None,
                    NOW,
                ),
                rejected_place,
            )
            .await,
        Err(TripRepoError::Forbidden)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM trip_places WHERE trip_id = 'trip-a' AND id = 'viewer-place'",
        )
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        0
    );

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn candidate_multi_row_insert_rolls_back_and_corrupt_joins_fail_closed() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;

    sqlx::query(
        "CREATE TRIGGER fail_candidate_insert BEFORE INSERT ON candidates \
         BEGIN SELECT RAISE(FAIL, 'injected candidate failure'); END",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    let failed_place = place("failed-place", "Kyoto");
    assert!(matches!(
        trips
            .add_candidate(
                "trip-a",
                &human(&leader),
                candidate(
                    "failed-candidate",
                    "trip-a",
                    &failed_place.id,
                    &leader,
                    None,
                    NOW,
                ),
                failed_place,
            )
            .await,
        Err(TripRepoError::Unavailable)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM trip_places WHERE trip_id = 'trip-a'",)
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0
    );
    sqlx::query("DROP TRIGGER fail_candidate_insert")
        .execute(database.db.pool())
        .await
        .unwrap();

    let valid_place = place("place-a", "Kyoto");
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate("candidate-a", "trip-a", &valid_place.id, &leader, None, NOW),
            valid_place,
        )
        .await
        .unwrap();
    for unsupported_status in ["in_plan"] {
        sqlx::query("UPDATE candidates SET status = ? WHERE id = 'candidate-a'")
            .bind(unsupported_status)
            .execute(database.db.pool())
            .await
            .unwrap();
        assert!(matches!(
            trips.list_candidates("trip-a", &human(&leader)).await,
            Err(TripRepoError::CorruptData)
        ));
    }
    sqlx::query("UPDATE candidates SET status = 'shortlisted' WHERE id = 'candidate-a'")
        .execute(database.db.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE candidates SET tags_json = '[\" not-normalized\"]' WHERE id = 'candidate-a'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        trips.list_candidates("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn plan_v1_initialization_is_atomic_idempotent_and_updates_navigation_cities() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    let viewer = seed_user(&users, "viewer", "viewer@example.com").await;
    let outsider = seed_user(&users, "outsider", "outsider@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    add_membership(&database, "trip-a", &member, TripRole::Member).await;
    add_membership(&database, "trip-a", &viewer, TripRole::Viewer).await;

    let anchor = place("anchor-place", "Kyoto");
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate("anchor-candidate", "trip-a", &anchor.id, &leader, None, NOW),
            anchor.clone(),
        )
        .await
        .unwrap();
    let expected_plan = plan("plan-a", "trip-a");
    let expected_days = bootstrap_days(&expected_plan.id, &anchor.city, &anchor.tz);
    let detail = trips
        .initialize_plan(
            "trip-a",
            &human(&member),
            &anchor.id,
            expected_plan.clone(),
            expected_days.clone(),
        )
        .await
        .expect("a member may initialize plan v1");
    assert_eq!(detail.plan, expected_plan);
    assert_eq!(detail.days, expected_days);
    assert!(detail.stops.is_empty());
    assert_eq!(detail.day_feasibility.len(), 3);
    assert!(
        detail
            .day_feasibility
            .iter()
            .all(|value| value.used_min == 0)
    );

    let repeated = trips
        .initialize_plan(
            "trip-a",
            &human(&leader),
            &anchor.id,
            plan("ignored-plan", "trip-a"),
            bootstrap_days("ignored-plan", &anchor.city, &anchor.tz),
        )
        .await
        .expect("a repeated valid initialization returns the stored winner");
    assert_eq!(repeated, detail);
    assert_eq!(
        trips
            .get_current_plan("trip-a", &human(&viewer))
            .await
            .unwrap(),
        detail
    );
    assert_eq!(
        trips
            .list_plan_versions("trip-a", &human(&viewer))
            .await
            .unwrap(),
        vec![expected_plan]
    );
    assert!(matches!(
        trips.get_current_plan("trip-a", &human(&outsider)).await,
        Err(TripRepoError::NotFound)
    ));
    assert!(matches!(
        trips
            .get_current_plan(
                "trip-a",
                &TripAuthorizationContext::service(leader.id.clone(), "service-a".into()),
            )
            .await,
        Err(TripRepoError::Forbidden)
    ));
    let stored_trip = trips.get_trip("trip-a", &human(&leader)).await.unwrap();
    assert_eq!(stored_trip.current_plan_id(), Some("plan-a"));
    assert_eq!(
        stored_trip.status(),
        itinera_core::domain::trip::TripStatus::Planning
    );
    let summaries = trips.list_trips(&human(&leader)).await.unwrap();
    assert_eq!(summaries[0].cities, vec!["Kyoto"]);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT revision FROM trips WHERE id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        2
    );

    assert!(matches!(
        trips
            .initialize_plan(
                "trip-a",
                &human(&viewer),
                &anchor.id,
                plan("viewer-plan", "trip-a"),
                bootstrap_days("viewer-plan", &anchor.city, &anchor.tz),
            )
            .await,
        Err(TripRepoError::Forbidden)
    ));
    assert!(matches!(
        trips
            .initialize_plan(
                "trip-a",
                &TripAuthorizationContext::service(leader.id.clone(), "service-a".into()),
                &anchor.id,
                plan("service-plan", "trip-a"),
                bootstrap_days("service-plan", &anchor.city, &anchor.tz),
            )
            .await,
        Err(TripRepoError::Forbidden)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn concurrent_plan_initializers_return_one_complete_winner() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = Arc::new(database.trips());
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    let anchor = place("anchor", "Kyoto");
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate("candidate", "trip-a", "anchor", &leader, None, NOW),
            anchor.clone(),
        )
        .await
        .unwrap();

    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();
    for suffix in ["a", "b"] {
        let trips = trips.clone();
        let barrier = barrier.clone();
        let authorization = human(&leader);
        let anchor = anchor.clone();
        tasks.push(tokio::spawn(async move {
            let plan_id = format!("plan-{suffix}");
            barrier.wait().await;
            trips
                .initialize_plan(
                    "trip-a",
                    &authorization,
                    &anchor.id,
                    plan(&plan_id, "trip-a"),
                    bootstrap_days(&plan_id, &anchor.city, &anchor.tz),
                )
                .await
        }));
    }
    barrier.wait().await;
    let first = tasks.remove(0).await.unwrap().unwrap();
    let second = tasks.remove(0).await.unwrap().unwrap();
    assert_eq!(first, second);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM plans WHERE trip_id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM plan_days WHERE trip_id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        3
    );

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn plan_initialization_rolls_back_and_revision_corruption_blocks_later_mutations() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    let anchor = place("anchor", "Kyoto");
    let anchor_candidate = candidate("candidate", "trip-a", "anchor", &leader, None, NOW);
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            anchor_candidate.clone(),
            anchor.clone(),
        )
        .await
        .unwrap();

    sqlx::query(
        "CREATE TRIGGER fail_second_day BEFORE INSERT ON plan_days \
         WHEN NEW.date = '2026-08-08' \
         BEGIN SELECT RAISE(FAIL, 'injected day failure'); END",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        trips
            .initialize_plan(
                "trip-a",
                &human(&leader),
                &anchor.id,
                plan("plan-a", "trip-a"),
                bootstrap_days("plan-a", &anchor.city, &anchor.tz),
            )
            .await,
        Err(TripRepoError::Unavailable)
    ));
    for query in [
        "SELECT COUNT(*) FROM plans WHERE trip_id = 'trip-a'",
        "SELECT COUNT(*) FROM plan_days WHERE trip_id = 'trip-a'",
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .fetch_one(database.db.pool())
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    let pointer: (Option<String>, Option<i64>, i64) = sqlx::query_as(
        "SELECT current_plan_id, current_plan_version, revision FROM trips WHERE id = 'trip-a'",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    assert_eq!(pointer, (None, None, 1));
    sqlx::query("DROP TRIGGER fail_second_day")
        .execute(database.db.pool())
        .await
        .unwrap();

    sqlx::query("UPDATE trips SET revision = 9223372036854775807 WHERE id = 'trip-a'")
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        trips
            .initialize_plan(
                "trip-a",
                &human(&leader),
                &anchor.id,
                plan("overflow-plan", "trip-a"),
                bootstrap_days("overflow-plan", &anchor.city, &anchor.tz),
            )
            .await,
        Err(TripRepoError::CorruptData)
    ));
    let overflow_state: (i64, i64, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT \
             (SELECT COUNT(*) FROM plans WHERE trip_id = 'trip-a'), \
             (SELECT COUNT(*) FROM plan_days WHERE trip_id = 'trip-a'), \
             current_plan_id, current_plan_version \
         FROM trips WHERE id = 'trip-a'",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    assert_eq!(overflow_state, (0, 0, None, None));

    assert_eq!(
        trips
            .list_candidates("trip-a", &human(&leader))
            .await
            .unwrap()[0]
            .candidate,
        anchor_candidate
    );

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn candidate_collection_enforces_exact_row_and_encoded_byte_boundaries() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;

    let mut expected = boundary_candidates(&leader);
    pad_candidate_collection_to_limit(&mut expected);
    assert_eq!(
        serde_json::to_vec(&expected).unwrap().len(),
        MAX_RESPONSE_BYTES
    );

    let mut transaction = database.db.pool().begin().await.unwrap();
    for value in &expected {
        insert_place_raw(&mut transaction, "trip-a", &value.place).await;
        insert_candidate_raw(&mut transaction, &value.candidate).await;
    }
    transaction.commit().await.unwrap();

    let exact = trips
        .list_candidates("trip-a", &human(&leader))
        .await
        .expect("exactly 1,000 rows and 4 MiB are accepted");
    assert_eq!(exact, expected);
    assert_eq!(
        serde_json::to_vec(&exact).unwrap().len(),
        MAX_RESPONSE_BYTES
    );

    let overflow_place = place("overflow-place", "Kyoto");
    assert!(matches!(
        trips
            .add_candidate(
                "trip-a",
                &human(&leader),
                candidate(
                    "overflow-candidate",
                    "trip-a",
                    &overflow_place.id,
                    &leader,
                    None,
                    NOW,
                ),
                overflow_place,
            )
            .await,
        Err(TripRepoError::Conflict)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM trip_places WHERE trip_id = 'trip-a' AND id = 'overflow-place'",
        )
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        0
    );

    let extendable = expected
        .iter()
        .find(|value| value.place.guide.as_ref().unwrap().intro.chars().count() < 4_000)
        .expect("one candidate guide retains one character of headroom");
    let original_guide = serde_json::to_string(extendable.place.guide.as_ref().unwrap()).unwrap();
    let mut over_limit_guide = extendable.place.guide.clone().unwrap();
    over_limit_guide.intro.push('a');
    sqlx::query("UPDATE trip_places SET guide_json = ? WHERE trip_id = 'trip-a' AND id = ?")
        .bind(serde_json::to_string(&over_limit_guide).unwrap())
        .bind(&extendable.place.id)
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        trips.list_candidates("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    sqlx::query("UPDATE trip_places SET guide_json = ? WHERE trip_id = 'trip-a' AND id = ?")
        .bind(original_guide)
        .bind(&extendable.place.id)
        .execute(database.db.pool())
        .await
        .unwrap();

    let extra_place = place("raw-place-1001", "Kyoto");
    let extra_candidate = candidate(
        "raw-candidate-1001",
        "trip-a",
        &extra_place.id,
        &leader,
        None,
        NOW,
    );
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_place_raw(&mut transaction, "trip-a", &extra_place).await;
    insert_candidate_raw(&mut transaction, &extra_candidate).await;
    transaction.commit().await.unwrap();
    assert!(matches!(
        trips.list_candidates("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn saved_place_search_enforces_exact_row_and_encoded_byte_boundaries() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    let anchor = place("anchor", "Kyoto");
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate("anchor-candidate", "trip-a", &anchor.id, &leader, None, NOW),
            anchor.clone(),
        )
        .await
        .unwrap();
    trips
        .initialize_plan(
            "trip-a",
            &human(&leader),
            &anchor.id,
            plan("plan-a", "trip-a"),
            bootstrap_days("plan-a", &anchor.city, &anchor.tz),
        )
        .await
        .unwrap();

    let mut untrusted = place("untrusted-saved", "Kyoto");
    untrusted.name = "Untrusted saved place".into();
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_place_raw(&mut transaction, "trip-a", &untrusted).await;
    sqlx::query(
        "INSERT INTO plans ( \
             trip_id, version, id, created_from_proposal_id, created_at, revision \
         ) VALUES ('trip-a', 2, 'untrusted-plan', 'untrusted-proposal', ?, 1)",
    )
    .bind(NOW)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO plan_days ( \
             trip_id, plan_version, id, plan_id, date, city_hint, tz, \
             window_start, window_end, revision \
         ) VALUES ('trip-a', 2, 'untrusted-day', 'untrusted-plan', \
             '2026-08-07', 'Kyoto', 'UTC', '09:00', '21:00', 1)",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO stop_identities (trip_id, id) \
         VALUES ('trip-a', 'untrusted-stop')",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO plan_stops ( \
             trip_id, plan_version, id, day_id, seq, place_id, stop_kind, \
             planned_arrival, duration_min, notes, revision \
         ) VALUES ('trip-a', 2, 'untrusted-stop', 'untrusted-day', 1, \
             'untrusted-saved', 'visit', '10:00', 60, '', 1)",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    assert_eq!(
        trips
            .search_saved_places("trip-a", &human(&leader), "Untrusted")
            .await
            .unwrap(),
        Vec::<Place>::new()
    );

    let mut expected = boundary_saved_places();
    pad_place_collection_to_limit(&mut expected);
    assert_eq!(
        serde_json::to_vec(&expected).unwrap().len(),
        MAX_RESPONSE_BYTES
    );
    let mut transaction = database.db.pool().begin().await.unwrap();
    for (index, saved) in expected.iter().enumerate() {
        insert_place_raw(&mut transaction, "trip-a", saved).await;
        insert_plan_stop_raw(
            &mut transaction,
            "trip-a",
            "day-1",
            &format!("saved-stop-{index:03}"),
            &saved.id,
            index + 1,
        )
        .await;
    }
    transaction.commit().await.unwrap();

    let exact = trips
        .search_saved_places("trip-a", &human(&leader), "Saved")
        .await
        .expect("exactly 100 saved rows and 4 MiB are accepted");
    assert_eq!(exact, expected);
    assert_eq!(
        serde_json::to_vec(&exact).unwrap().len(),
        MAX_RESPONSE_BYTES
    );

    let extendable = expected
        .iter()
        .find(|saved| {
            saved
                .guide
                .as_ref()
                .unwrap()
                .practical_tips
                .iter()
                .any(|tip| tip.chars().count() < 500)
        })
        .expect("one saved-place guide retains one character of headroom");
    let original_guide = serde_json::to_string(extendable.guide.as_ref().unwrap()).unwrap();
    let mut over_limit_guide = extendable.guide.clone().unwrap();
    over_limit_guide
        .practical_tips
        .iter_mut()
        .find(|tip| tip.chars().count() < 500)
        .unwrap()
        .push('a');
    sqlx::query("UPDATE trip_places SET guide_json = ? WHERE trip_id = 'trip-a' AND id = ?")
        .bind(serde_json::to_string(&over_limit_guide).unwrap())
        .bind(&extendable.id)
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        trips
            .search_saved_places("trip-a", &human(&leader), "Saved")
            .await,
        Err(TripRepoError::CorruptData)
    ));
    sqlx::query("UPDATE trip_places SET guide_json = ? WHERE trip_id = 'trip-a' AND id = ?")
        .bind(original_guide)
        .bind(&extendable.id)
        .execute(database.db.pool())
        .await
        .unwrap();

    let mut extra = place("saved-place-100", "Kyoto");
    extra.name = "Saved 100".into();
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_place_raw(&mut transaction, "trip-a", &extra).await;
    insert_plan_stop_raw(
        &mut transaction,
        "trip-a",
        "day-1",
        "saved-stop-100",
        &extra.id,
        101,
    )
    .await;
    transaction.commit().await.unwrap();
    assert!(matches!(
        trips
            .search_saved_places("trip-a", &human(&leader), "Saved")
            .await,
        Err(TripRepoError::CorruptData)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn current_plan_accepts_exactly_four_mib_and_rejects_one_more_byte() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    let anchor = place("anchor", "Kyoto");
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate("anchor-candidate", "trip-a", &anchor.id, &leader, None, NOW),
            anchor.clone(),
        )
        .await
        .unwrap();
    trips
        .initialize_plan(
            "trip-a",
            &human(&leader),
            &anchor.id,
            plan("plan-a", "trip-a"),
            bootstrap_days("plan-a", &anchor.city, &anchor.tz),
        )
        .await
        .unwrap();

    let saved_places = boundary_saved_places();
    let mut transaction = database.db.pool().begin().await.unwrap();
    for (index, saved) in saved_places.iter().enumerate() {
        insert_place_raw(&mut transaction, "trip-a", saved).await;
        insert_plan_stop_raw(
            &mut transaction,
            "trip-a",
            "day-1",
            &format!("plan-stop-{index:03}"),
            &saved.id,
            index + 1,
        )
        .await;
    }
    transaction.commit().await.unwrap();

    let mut expected = trips
        .get_current_plan("trip-a", &human(&leader))
        .await
        .expect("the unpadded plan graph is valid");
    pad_plan_detail_to_limit(&mut expected);
    assert_eq!(
        serde_json::to_vec(&expected).unwrap().len(),
        MAX_RESPONSE_BYTES
    );
    let mut transaction = database.db.pool().begin().await.unwrap();
    for saved in &expected.places {
        sqlx::query("UPDATE trip_places SET guide_json = ? WHERE trip_id = 'trip-a' AND id = ?")
            .bind(serde_json::to_string(saved.guide.as_ref().unwrap()).unwrap())
            .bind(&saved.id)
            .execute(&mut *transaction)
            .await
            .unwrap();
    }
    transaction.commit().await.unwrap();

    let exact = trips
        .get_current_plan("trip-a", &human(&leader))
        .await
        .expect("the exact encoded plan ceiling is accepted");
    assert_eq!(exact, expected);
    assert_eq!(
        serde_json::to_vec(&exact).unwrap().len(),
        MAX_RESPONSE_BYTES
    );

    let extendable = expected
        .places
        .iter()
        .find(|saved| {
            saved
                .guide
                .as_ref()
                .unwrap()
                .practical_tips
                .iter()
                .any(|tip| tip.chars().count() < 500)
        })
        .expect("one plan place retains one character of headroom");
    let mut over_limit_guide = extendable.guide.clone().unwrap();
    over_limit_guide
        .practical_tips
        .iter_mut()
        .find(|tip| tip.chars().count() < 500)
        .unwrap()
        .push('a');
    sqlx::query("UPDATE trip_places SET guide_json = ? WHERE trip_id = 'trip-a' AND id = ?")
        .bind(serde_json::to_string(&over_limit_guide).unwrap())
        .bind(&extendable.id)
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        trips.get_current_plan("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn candidate_and_plan_writers_recheck_roles_after_reserving_the_writer() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    add_membership(&database, "trip-a", &member, TripRole::Member).await;
    let anchor = place("anchor", "Kyoto");
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate("anchor-candidate", "trip-a", &anchor.id, &leader, None, NOW),
            anchor.clone(),
        )
        .await
        .unwrap();

    let mut blocker = raw_connection(&database.path).await;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut blocker)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE trip_memberships SET role = 'viewer', revision = revision + 1 \
         WHERE trip_id = 'trip-a' AND user_id = 'member'",
    )
    .execute(&mut blocker)
    .await
    .unwrap();
    let repository = trips.clone();
    let actor = member.clone();
    let task = tokio::spawn(async move {
        let proposed_place = place("revoked-place", "Kyoto");
        repository
            .add_candidate(
                "trip-a",
                &human(&actor),
                candidate(
                    "revoked-candidate",
                    "trip-a",
                    &proposed_place.id,
                    &actor,
                    None,
                    NOW,
                ),
                proposed_place,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    sqlx::query("COMMIT").execute(&mut blocker).await.unwrap();
    blocker.close().await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(TripRepoError::Forbidden)));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM candidates WHERE trip_id = 'trip-a' AND id = 'revoked-candidate'",
        )
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        0
    );

    sqlx::query(
        "UPDATE trip_memberships SET role = 'member', revision = revision + 1 \
         WHERE trip_id = 'trip-a' AND user_id = 'member'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    let mut blocker = raw_connection(&database.path).await;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut blocker)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE trip_memberships SET role = 'viewer', revision = revision + 1 \
         WHERE trip_id = 'trip-a' AND user_id = 'member'",
    )
    .execute(&mut blocker)
    .await
    .unwrap();
    let repository = trips.clone();
    let actor = member.clone();
    let anchor_for_task = anchor.clone();
    let task = tokio::spawn(async move {
        repository
            .initialize_plan(
                "trip-a",
                &human(&actor),
                &anchor_for_task.id,
                plan("revoked-plan", "trip-a"),
                bootstrap_days("revoked-plan", &anchor_for_task.city, &anchor_for_task.tz),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    sqlx::query("COMMIT").execute(&mut blocker).await.unwrap();
    blocker.close().await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(TripRepoError::Forbidden)));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM plans WHERE trip_id = 'trip-a'")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0
    );

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn plan_reads_fail_closed_on_domain_and_revision_corruption() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    let anchor = place("anchor", "Kyoto");
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate("anchor-candidate", "trip-a", &anchor.id, &leader, None, NOW),
            anchor.clone(),
        )
        .await
        .unwrap();
    trips
        .initialize_plan(
            "trip-a",
            &human(&leader),
            &anchor.id,
            plan("plan-a", "trip-a"),
            bootstrap_days("plan-a", &anchor.city, &anchor.tz),
        )
        .await
        .unwrap();

    sqlx::query("UPDATE plan_days SET city_hint = ' not-normalized' WHERE id = 'day-1'")
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        trips.get_current_plan("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    sqlx::query("UPDATE plan_days SET city_hint = 'Kyoto' WHERE id = 'day-1'")
        .execute(database.db.pool())
        .await
        .unwrap();

    let mut connection = raw_connection(&database.path).await;
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("UPDATE plans SET revision = 0 WHERE trip_id = 'trip-a' AND version = 1")
        .execute(&mut connection)
        .await
        .unwrap();
    sqlx::query("PRAGMA ignore_check_constraints = OFF")
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
    assert!(matches!(
        trips.get_current_plan("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    assert!(matches!(
        trips.list_plan_versions("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));

    sqlx::query("UPDATE plans SET revision = 1 WHERE trip_id = 'trip-a' AND version = 1")
        .execute(database.db.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO plans ( \
             trip_id, version, id, created_from_proposal_id, created_at, revision \
         ) VALUES ('trip-a', 2, 'untrusted-plan', 'untrusted-proposal', ?, 1)",
    )
    .bind(NOW)
    .execute(database.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO plan_days ( \
             trip_id, plan_version, id, plan_id, date, city_hint, tz, \
             window_start, window_end, revision \
         ) VALUES ('trip-a', 2, 'untrusted-day', 'untrusted-plan', \
             '2026-08-07', 'Kyoto', 'Asia/Tokyo', '09:00', '21:00', 1)",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE trips SET current_plan_id = 'untrusted-plan', current_plan_version = 2 \
         WHERE id = 'trip-a'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        trips.get_current_plan("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    assert!(matches!(
        trips.get_trip("trip-a", &human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    assert!(matches!(
        trips.list_trips(&human(&leader)).await,
        Err(TripRepoError::CorruptData)
    ));
    assert!(matches!(
        trips
            .initialize_plan(
                "trip-a",
                &human(&leader),
                &anchor.id,
                plan("ignored-plan", "trip-a"),
                bootstrap_days("ignored-plan", &anchor.city, &anchor.tz),
            )
            .await,
        Err(TripRepoError::CorruptData)
    ));

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn composite_keys_reject_cross_trip_candidate_and_plan_links() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let other = seed_user(&users, "other", "other@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    seed_trip(&trips, "trip-b", &other).await;

    let source = place("foreign-source", "Osaka");
    let owned = place("owned-place", "Kyoto");
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_place_raw(&mut transaction, "trip-b", &source).await;
    insert_place_raw(&mut transaction, "trip-a", &owned).await;
    transaction.commit().await.unwrap();
    assert!(
        sqlx::query(
            "INSERT INTO candidates ( \
                 trip_id, id, place_id, source_trip_place_id, proposed_by, created_at, \
                 pitch, tags_json, status, revision \
             ) VALUES ('trip-a', 'bad-candidate', 'owned-place', 'foreign-source', \
                 'leader', ?, 'pitch', '[]', 'shortlisted', 1)",
        )
        .bind(NOW)
        .execute(database.db.pool())
        .await
        .is_err(),
        "a trip-scoped source cannot resolve through another trip"
    );

    sqlx::query(
        "INSERT INTO plans (trip_id, version, id, created_at, revision) \
         VALUES ('trip-b', 1, 'plan-b', ?, 1)",
    )
    .bind(NOW)
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(
        sqlx::query(
            "UPDATE trips SET current_plan_id = 'plan-b', current_plan_version = 1 \
             WHERE id = 'trip-a'",
        )
        .execute(database.db.pool())
        .await
        .is_err(),
        "the exact current pointer cannot resolve through another trip"
    );
    assert!(
        sqlx::query(
            "INSERT INTO plan_days ( \
                 trip_id, plan_version, id, plan_id, date, city_hint, tz, \
                 window_start, window_end, revision \
             ) VALUES ('trip-a', 1, 'bad-day', 'plan-b', '2026-08-07', \
                 'Kyoto', 'Asia/Tokyo', '09:00', '21:00', 1)",
        )
        .execute(database.db.pool())
        .await
        .is_err(),
        "a plan day cannot resolve through another trip"
    );
    for statement in [
        "INSERT INTO trip_places (trip_id, id, name, kind, lat, lng, tz, country_code, admin_area, city, address, photo_urls_json, revision) VALUES ('trip-a', 'bad-kind', 'Bad', 'unknown', 0.0, 0.0, 'UTC', '', '', 'Kyoto', '', '[]', 1)",
        "INSERT INTO trip_places (trip_id, id, name, kind, lat, lng, tz, country_code, admin_area, city, address, photo_urls_json, revision) VALUES ('trip-a', 'bad-json', 'Bad', 'sight', 0.0, 0.0, 'UTC', '', '', 'Kyoto', '', '{broken', 1)",
        "INSERT INTO plans (trip_id, version, id, created_at, revision) VALUES ('trip-a', 2, 'bad-provenance', '2026-08-07T12:00:00Z', 1)",
    ] {
        assert!(
            sqlx::query(statement)
                .execute(database.db.pool())
                .await
                .is_err(),
            "schema unexpectedly accepted {statement}"
        );
    }

    drop(trips);
    drop(users);
    database.shutdown().await;
}

async fn insert_place_raw(transaction: &mut Transaction<'_, Sqlite>, trip_id: &str, place: &Place) {
    let external_ref_json = place
        .external_ref
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .unwrap();
    let opening_hours_json = place
        .opening_hours
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .unwrap();
    let guide_json = place
        .guide
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .unwrap();
    sqlx::query(
        "INSERT INTO trip_places ( \
             trip_id, id, name, kind, lat, lng, tz, country_code, admin_area, city, \
             address, external_ref_json, website, phone, rating, price_level, \
             opening_hours_json, photo_urls_json, guide_json, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1)",
    )
    .bind(trip_id)
    .bind(&place.id)
    .bind(&place.name)
    .bind(match place.kind {
        PlaceKind::Sight => "sight",
        PlaceKind::Food => "food",
        PlaceKind::Lodging => "lodging",
        PlaceKind::Activity => "activity",
        PlaceKind::TransportHub => "transport_hub",
    })
    .bind(place.lat)
    .bind(place.lng)
    .bind(&place.tz)
    .bind(&place.country_code)
    .bind(&place.admin_area)
    .bind(&place.city)
    .bind(&place.address)
    .bind(external_ref_json)
    .bind(&place.website)
    .bind(&place.phone)
    .bind(place.rating)
    .bind(place.price_level.map(i64::from))
    .bind(opening_hours_json)
    .bind(serde_json::to_string(&place.photo_urls).unwrap())
    .bind(guide_json)
    .execute(&mut **transaction)
    .await
    .unwrap();
}

async fn insert_candidate_raw(transaction: &mut Transaction<'_, Sqlite>, candidate: &Candidate) {
    sqlx::query(
        "INSERT INTO candidates ( \
             trip_id, id, place_id, source_catalog_place_id, source_trip_place_id, \
             proposed_by, created_at, pitch, tags_json, status, revision \
         ) VALUES (?, ?, ?, NULL, NULL, ?, ?, ?, ?, ?, 1)",
    )
    .bind(&candidate.trip_id)
    .bind(&candidate.id)
    .bind(&candidate.place_id)
    .bind(&candidate.proposed_by)
    .bind(&candidate.created_at)
    .bind(&candidate.pitch)
    .bind(serde_json::to_string(&candidate.tags).unwrap())
    .bind(match candidate.status {
        CandidateStatus::Shortlisted => "shortlisted",
        CandidateStatus::InPlan => "in_plan",
        CandidateStatus::Rejected => "rejected",
    })
    .execute(&mut **transaction)
    .await
    .unwrap();
}

async fn insert_plan_stop_raw(
    transaction: &mut Transaction<'_, Sqlite>,
    trip_id: &str,
    day_id: &str,
    stop_id: &str,
    place_id: &str,
    sequence: usize,
) {
    sqlx::query("INSERT INTO stop_identities (trip_id, id) VALUES (?, ?)")
        .bind(trip_id)
        .bind(stop_id)
        .execute(&mut **transaction)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO plan_stops ( \
             trip_id, plan_version, id, day_id, seq, place_id, stop_kind, \
             planned_arrival, duration_min, notes, revision \
         ) VALUES (?, 1, ?, ?, ?, ?, 'visit', '10:00', 1, '', 1)",
    )
    .bind(trip_id)
    .bind(stop_id)
    .bind(day_id)
    .bind(sequence as f64)
    .bind(place_id)
    .execute(&mut **transaction)
    .await
    .unwrap();
}

async fn add_membership(database: &TestDatabase, trip_id: &str, user: &User, role: TripRole) {
    sqlx::query(
        "INSERT INTO trip_memberships \
         (trip_id, user_id, role, joined_at, revision) VALUES (?, ?, ?, ?, 1)",
    )
    .bind(trip_id)
    .bind(&user.id.0)
    .bind(match role {
        TripRole::Leader => "leader",
        TripRole::Member => "member",
        TripRole::Viewer => "viewer",
    })
    .bind(NOW)
    .execute(database.db.pool())
    .await
    .unwrap();
}
