use std::{sync::Arc, time::Duration};

use itinera_core::{
    domain::{
        content_history::{ChangeSource, Edit, EditEntity, EditStatus},
        trip::{
            Booking, Candidate, CandidateDisposition, CandidateStatus, Day, DayPatch,
            ExternalPlaceRef, Money, Place, PlaceKind, Plan, StopPatch, TripRole, TripStatus,
        },
        user::User,
    },
    ports::{
        authorization::TripAuthorizationContext,
        content_history::{ContentHistoryRepo, ContentHistoryRepoError},
        trip::{CandidateUpdate, TripRepo, TripRepoError},
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

fn catalog_place(id: &str, source_id: &str, city: &str) -> Place {
    Place {
        lat: 35.0,
        lng: 135.0,
        tz: "Asia/Tokyo".into(),
        country_code: "JP".into(),
        admin_area: "Kyoto".into(),
        external_ref: Some(ExternalPlaceRef {
            provider: "google".into(),
            place_id: source_id.into(),
        }),
        rating: Some(4.5),
        price_level: Some(2),
        ..place(id, city)
    }
}

fn candidate(id: &str, trip_id: &str, place_id: &str, proposer: &User) -> Candidate {
    Candidate {
        id: id.into(),
        trip_id: trip_id.into(),
        source_place_id: None,
        place_id: place_id.into(),
        proposed_by: proposer.id.0.clone(),
        created_at: NOW.into(),
        pitch: "Original pitch".into(),
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

fn days(plan_id: &str) -> Vec<Day> {
    ["2026-08-07", "2026-08-08", "2026-08-09"]
        .into_iter()
        .enumerate()
        .map(|(index, date)| Day {
            id: format!("day-{}", index + 1),
            plan_id: plan_id.into(),
            date: date.into(),
            city_hint: "Kyoto".into(),
            tz: "UTC".into(),
            window_start: "09:00".into(),
            window_end: "21:00".into(),
        })
        .collect()
}

async fn seed_content_graph(database: &TestDatabase) -> (User, User, User, User, Place) {
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    let viewer = seed_user(&users, "viewer", "viewer@example.com").await;
    let outsider = seed_user(&users, "outsider", "outsider@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    add_membership(database, "trip-a", &member, TripRole::Member).await;
    add_membership(database, "trip-a", &viewer, TripRole::Viewer).await;
    let anchor = place("anchor-place", "Kyoto");
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            candidate("candidate-a", "trip-a", &anchor.id, &leader),
            anchor.clone(),
        )
        .await
        .expect("seed candidate");
    trips
        .initialize_plan(
            "trip-a",
            &human(&leader),
            &anchor.id,
            plan("plan-a", "trip-a"),
            days("plan-a"),
        )
        .await
        .expect("seed plan");
    insert_stop(database, "trip-a", "day-1", "stop-a", &anchor.id).await;
    (leader, member, viewer, outsider, anchor)
}

#[tokio::test]
async fn audited_mutations_round_trip_with_roles_order_and_typed_values() {
    let database = TestDatabase::new().await;
    let trips = database.trips();
    let history = database.history();
    let (leader, _member, viewer, outsider, _anchor) = seed_content_graph(&database).await;

    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Booked,
            "2026-08-07T13:00:00.000Z",
            "trip-status",
        )
        .await
        .unwrap();
    let replacement = place("replacement-place", "Nara");
    trips
        .update_candidate(
            "trip-a",
            &human(&leader),
            "candidate-a",
            CandidateUpdate {
                place: replacement,
                pitch: "Replacement pitch".into(),
                tags: vec!["temple".into(), "morning".into()],
                changed_at: "2026-08-07T14:00:00.000Z".into(),
                change_id: "candidate-edit".into(),
            },
        )
        .await
        .unwrap();
    trips
        .set_candidate_status(
            "trip-a",
            &human(&leader),
            "candidate-a",
            CandidateDisposition::Rejected,
            "2026-08-07T15:00:00.000Z",
            "candidate-status",
        )
        .await
        .unwrap();
    trips
        .update_day(
            "trip-a",
            &human(&leader),
            "day-1",
            DayPatch {
                window_start: Some("10:00".into()),
                window_end: None,
                city_hint: Some("Osaka".into()),
            },
            "2026-08-07T16:00:00.000Z",
            "day-edit",
        )
        .await
        .unwrap();
    trips
        .update_stop(
            "trip-a",
            &human(&leader),
            "stop-a",
            StopPatch {
                planned_arrival: Some("11:00".into()),
                duration_min: Some(90),
                notes: Some("Bring the voucher".into()),
                booking: Some(Some(Booking {
                    reference: "ABC-123".into(),
                    url: Some("https://example.test/booking".into()),
                    cost: Some(Money {
                        amount: 42.0,
                        currency: "GBP".into(),
                    }),
                    ledger_entry_id: None,
                })),
            },
            "2026-08-07T17:00:00.000Z",
            "stop-edit",
        )
        .await
        .unwrap();

    let edits = history
        .list_history("trip-a", &human(&viewer))
        .await
        .expect("every direct member may read history");
    assert_eq!(edits.len(), 11);
    assert_eq!(edits[0].id, "stop-edit-03");
    assert_eq!(edits[1].id, "stop-edit-02");
    assert_eq!(edits.last().unwrap().id, "trip-status");
    assert!(edits.iter().all(|edit| {
        edit.status == EditStatus::Applied && matches!(edit.source, ChangeSource::Web {})
    }));
    assert!(edits.iter().any(|edit| {
        edit.id == "candidate-edit-00"
            && edit.entity == EditEntity::Candidate
            && edit.field == "place"
    }));
    assert!(edits.iter().any(|edit| {
        edit.id == "stop-edit-03"
            && edit.field == "booking"
            && edit.old_value.is_null()
            && edit.new_value["ref"] == "ABC-123"
    }));
    assert!(matches!(
        history.list_history("trip-a", &human(&outsider)).await,
        Err(ContentHistoryRepoError::NotFound)
    ));
    assert!(matches!(
        history
            .list_history(
                "trip-a",
                &TripAuthorizationContext::service(leader.id.clone(), "service-a".into()),
            )
            .await,
        Err(ContentHistoryRepoError::Forbidden)
    ));

    database.shutdown().await;
}

#[tokio::test]
async fn catalog_candidate_revisions_and_reverts_preserve_immutable_provider_facts() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let history = database.history();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;

    let original = catalog_place("catalog-snapshot-a", "catalog-source-a", "Kyoto");
    let mut sourced = candidate("candidate-a", "trip-a", &original.id, &leader);
    sourced.source_place_id = Some("catalog-source-a".into());
    trips
        .add_candidate("trip-a", &human(&leader), sourced, original.clone())
        .await
        .unwrap();

    let mut forged = original.clone();
    forged.id = "forged-snapshot".into();
    forged.lat = 34.0;
    assert!(matches!(
        trips
            .update_candidate(
                "trip-a",
                &human(&leader),
                "candidate-a",
                CandidateUpdate {
                    place: forged,
                    pitch: "Original pitch".into(),
                    tags: vec!["quiet".into()],
                    changed_at: NOW.into(),
                    change_id: "forged-change".into(),
                },
            )
            .await,
        Err(TripRepoError::CorruptData)
    ));
    assert!(
        history
            .list_history("trip-a", &human(&leader))
            .await
            .unwrap()
            .is_empty()
    );

    let mut replacement = original.clone();
    replacement.id = "catalog-snapshot-b".into();
    replacement.name = "Updated temple name".into();
    replacement.city = "Uji".into();
    trips
        .update_candidate(
            "trip-a",
            &human(&leader),
            "candidate-a",
            CandidateUpdate {
                place: replacement.clone(),
                pitch: "Original pitch".into(),
                tags: vec!["quiet".into()],
                changed_at: NOW.into(),
                change_id: "catalog-change".into(),
            },
        )
        .await
        .unwrap();

    let mut unrelated = catalog_place("unrelated-snapshot", "catalog-source-b", "Osaka");
    unrelated.lat = 34.7;
    unrelated.lng = 135.5;
    let mut unrelated_candidate = candidate("candidate-b", "trip-a", &unrelated.id, &leader);
    unrelated_candidate.source_place_id = Some("catalog-source-b".into());
    trips
        .add_candidate(
            "trip-a",
            &human(&leader),
            unrelated_candidate,
            unrelated.clone(),
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE content_edits SET old_value_json = ? \
         WHERE trip_id = 'trip-a' AND id = 'catalog-change-00'",
    )
    .bind(serde_json::to_string(&unrelated).unwrap())
    .execute(database.db.pool())
    .await
    .unwrap();

    assert!(matches!(
        history
            .revert_edit(
                "trip-a",
                &human(&leader),
                "catalog-change-00",
                "2026-08-07T13:00:00.000Z",
                "catalog-revert",
            )
            .await,
        Err(ContentHistoryRepoError::CorruptData)
    ));
    let stored = trips
        .list_candidates("trip-a", &human(&leader))
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.candidate.id == "candidate-a")
        .unwrap();
    assert_eq!(stored.place, replacement);

    database.shutdown().await;
}

#[tokio::test]
async fn entity_and_history_failures_roll_back_the_whole_mutation() {
    let database = TestDatabase::new().await;
    let trips = database.trips();
    let history = database.history();
    let (leader, _member, _viewer, _outsider, _anchor) = seed_content_graph(&database).await;

    sqlx::query(
        "CREATE TRIGGER fail_trip_status BEFORE UPDATE OF status ON trips \
         BEGIN SELECT RAISE(FAIL, 'injected entity failure'); END",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        trips
            .set_trip_status(
                "trip-a",
                &human(&leader),
                TripStatus::Booked,
                "2026-08-07T13:00:00.000Z",
                "rolled-back-edit",
            )
            .await,
        Err(TripRepoError::Unavailable)
    ));
    assert!(
        history
            .list_history("trip-a", &human(&leader))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        trips
            .get_trip("trip-a", &human(&leader))
            .await
            .unwrap()
            .status(),
        TripStatus::Planning
    );
    sqlx::query("DROP TRIGGER fail_trip_status")
        .execute(database.db.pool())
        .await
        .unwrap();

    sqlx::query(
        "CREATE TRIGGER fail_history BEFORE INSERT ON content_edits \
         BEGIN SELECT RAISE(FAIL, 'injected history failure'); END",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        trips
            .set_trip_status(
                "trip-a",
                &human(&leader),
                TripStatus::Booked,
                "2026-08-07T13:00:00.000Z",
                "failed-history",
            )
            .await,
        Err(TripRepoError::Unavailable)
    ));
    assert_eq!(
        trips
            .get_trip("trip-a", &human(&leader))
            .await
            .unwrap()
            .status(),
        TripStatus::Planning
    );
    sqlx::query("DROP TRIGGER fail_history")
        .execute(database.db.pool())
        .await
        .unwrap();

    sqlx::query(
        "CREATE TRIGGER fail_candidate_update BEFORE UPDATE ON candidates \
         BEGIN SELECT RAISE(FAIL, 'injected candidate failure'); END",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        trips
            .update_candidate(
                "trip-a",
                &human(&leader),
                "candidate-a",
                CandidateUpdate {
                    place: place("rolled-back-place", "Nara"),
                    pitch: "Changed".into(),
                    tags: Vec::new(),
                    changed_at: "2026-08-07T14:00:00.000Z".into(),
                    change_id: "rolled-back-candidate".into(),
                },
            )
            .await,
        Err(TripRepoError::Unavailable)
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM trip_places WHERE trip_id = 'trip-a' AND id = 'rolled-back-place'",
        )
        .fetch_one(database.db.pool())
        .await
        .unwrap(),
        0
    );
    assert!(
        history
            .list_history("trip-a", &human(&leader))
            .await
            .unwrap()
            .is_empty()
    );

    database.shutdown().await;
}

#[tokio::test]
async fn trip_status_revert_is_exact_chainable_idempotent_and_non_disclosing() {
    let database = TestDatabase::new().await;
    let trips = database.trips();
    let history = database.history();
    let (leader, _member, viewer, _outsider, _anchor) = seed_content_graph(&database).await;
    seed_trip(&trips, "trip-b", &leader).await;

    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Booked,
            "2026-08-07T13:00:00.000Z",
            "status-a",
        )
        .await
        .unwrap();
    assert!(matches!(
        history
            .revert_edit(
                "trip-a",
                &human(&viewer),
                "status-a",
                "2026-08-07T14:00:00.000Z",
                "status-b",
            )
            .await,
        Err(ContentHistoryRepoError::Forbidden)
    ));
    assert!(matches!(
        history
            .revert_edit(
                "trip-b",
                &human(&leader),
                "status-a",
                "2026-08-07T14:00:00.000Z",
                "foreign-compensation",
            )
            .await,
        Err(ContentHistoryRepoError::NotFound)
    ));
    history
        .revert_edit(
            "trip-a",
            &human(&leader),
            "status-a",
            "2026-08-07T14:00:00.000Z",
            "status-b",
        )
        .await
        .unwrap();
    assert_eq!(
        trips
            .get_trip("trip-a", &human(&leader))
            .await
            .unwrap()
            .status(),
        TripStatus::Planning
    );
    history
        .revert_edit(
            "trip-a",
            &human(&leader),
            "status-b",
            "2026-08-07T15:00:00.000Z",
            "status-c",
        )
        .await
        .unwrap();
    assert_eq!(
        trips
            .get_trip("trip-a", &human(&leader))
            .await
            .unwrap()
            .status(),
        TripStatus::Booked
    );
    history
        .revert_edit(
            "trip-a",
            &human(&leader),
            "status-a",
            "2026-08-07T16:00:00.000Z",
            "ignored-repeat-id",
        )
        .await
        .expect("an already reverted edit is an idempotent success");
    let edits = history
        .list_history("trip-a", &human(&leader))
        .await
        .unwrap();
    assert_eq!(edits.len(), 3);
    let original = edits.iter().find(|edit| edit.id == "status-a").unwrap();
    let first_compensation = edits.iter().find(|edit| edit.id == "status-b").unwrap();
    assert_eq!(original.status, EditStatus::Reverted);
    assert_eq!(original.revert_edit_id.as_deref(), Some("status-b"));
    assert_eq!(first_compensation.status, EditStatus::Reverted);
    assert_eq!(
        first_compensation.reverts_edit_id.as_deref(),
        Some("status-a")
    );
    assert_eq!(
        first_compensation.revert_edit_id.as_deref(),
        Some("status-c")
    );

    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Done,
            "2026-08-07T17:00:00.000Z",
            "stale-status",
        )
        .await
        .unwrap();
    sqlx::query("UPDATE trips SET status = 'ongoing', revision = revision + 1 WHERE id = 'trip-a'")
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        history
            .revert_edit(
                "trip-a",
                &human(&leader),
                "stale-status",
                "2026-08-07T18:00:00.000Z",
                "stale-compensation",
            )
            .await,
        Err(ContentHistoryRepoError::Conflict)
    ));

    database.shutdown().await;
}

#[tokio::test]
async fn candidate_day_stop_and_booking_reverts_restore_only_exact_current_fields() {
    let database = TestDatabase::new().await;
    let trips = database.trips();
    let history = database.history();
    let (leader, _member, _viewer, _outsider, anchor) = seed_content_graph(&database).await;

    trips
        .update_candidate(
            "trip-a",
            &human(&leader),
            "candidate-a",
            CandidateUpdate {
                place: place("replacement-place", "Nara"),
                pitch: "Replacement pitch".into(),
                tags: vec!["new".into()],
                changed_at: "2026-08-07T13:00:00.000Z".into(),
                change_id: "candidate-change".into(),
            },
        )
        .await
        .unwrap();
    for (edit_id, compensation_id, timestamp) in [
        (
            "candidate-change-01",
            "candidate-pitch-revert",
            "2026-08-07T14:00:00.000Z",
        ),
        (
            "candidate-change-02",
            "candidate-tags-revert",
            "2026-08-07T15:00:00.000Z",
        ),
        (
            "candidate-change-00",
            "candidate-place-revert",
            "2026-08-07T16:00:00.000Z",
        ),
    ] {
        history
            .revert_edit(
                "trip-a",
                &human(&leader),
                edit_id,
                timestamp,
                compensation_id,
            )
            .await
            .unwrap();
    }
    trips
        .set_candidate_status(
            "trip-a",
            &human(&leader),
            "candidate-a",
            CandidateDisposition::Rejected,
            "2026-08-07T17:00:00.000Z",
            "candidate-status",
        )
        .await
        .unwrap();
    history
        .revert_edit(
            "trip-a",
            &human(&leader),
            "candidate-status",
            "2026-08-07T18:00:00.000Z",
            "candidate-status-revert",
        )
        .await
        .unwrap();
    let restored_candidate = trips
        .list_candidates("trip-a", &human(&leader))
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.candidate.id == "candidate-a")
        .unwrap();
    assert_eq!(restored_candidate.place, anchor);
    assert_eq!(restored_candidate.candidate.pitch, "Original pitch");
    assert_eq!(restored_candidate.candidate.tags, vec!["quiet"]);
    assert_eq!(
        restored_candidate.candidate.status,
        CandidateStatus::Shortlisted
    );

    trips
        .update_day(
            "trip-a",
            &human(&leader),
            "day-1",
            DayPatch {
                window_start: Some("10:00".into()),
                window_end: None,
                city_hint: Some("Osaka".into()),
            },
            "2026-08-07T19:00:00.000Z",
            "day-change",
        )
        .await
        .unwrap();
    history
        .revert_edit(
            "trip-a",
            &human(&leader),
            "day-change-01",
            "2026-08-07T20:00:00.000Z",
            "day-city-revert",
        )
        .await
        .unwrap();
    history
        .revert_edit(
            "trip-a",
            &human(&leader),
            "day-change-00",
            "2026-08-07T21:00:00.000Z",
            "day-window-revert",
        )
        .await
        .unwrap();

    trips
        .update_stop(
            "trip-a",
            &human(&leader),
            "stop-a",
            StopPatch {
                planned_arrival: Some("11:00".into()),
                duration_min: Some(90),
                notes: Some("Changed notes".into()),
                booking: Some(Some(Booking {
                    reference: "ABC".into(),
                    url: None,
                    cost: None,
                    ledger_entry_id: None,
                })),
            },
            "2026-08-07T22:00:00.000Z",
            "stop-change",
        )
        .await
        .unwrap();
    for (edit_id, compensation_id, timestamp) in [
        (
            "stop-change-03",
            "stop-booking-revert",
            "2026-08-07T23:00:00.000Z",
        ),
        (
            "stop-change-02",
            "stop-notes-revert",
            "2026-08-08T00:00:00.000Z",
        ),
        (
            "stop-change-01",
            "stop-duration-revert",
            "2026-08-08T01:00:00.000Z",
        ),
        (
            "stop-change-00",
            "stop-arrival-revert",
            "2026-08-08T02:00:00.000Z",
        ),
    ] {
        history
            .revert_edit(
                "trip-a",
                &human(&leader),
                edit_id,
                timestamp,
                compensation_id,
            )
            .await
            .unwrap();
    }
    let detail = trips
        .get_current_plan("trip-a", &human(&leader))
        .await
        .unwrap();
    let day = detail.days.iter().find(|day| day.id == "day-1").unwrap();
    assert_eq!(day.window_start, "09:00");
    assert_eq!(day.city_hint, "Kyoto");
    let stop = detail
        .stops
        .iter()
        .find(|stop| stop.id == "stop-a")
        .unwrap();
    assert_eq!(stop.planned_arrival, "10:00");
    assert_eq!(stop.duration_min, 60);
    assert_eq!(stop.notes, "");
    assert_eq!(stop.booking, None);

    database.shutdown().await;
}

#[tokio::test]
async fn schema_and_reader_fail_closed_on_staged_sources_and_corrupt_graphs() {
    let database = TestDatabase::new().await;
    let history = database.history();
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    seed_trip(&trips, "trip-b", &leader).await;

    for statement in [
        "INSERT INTO content_edits (trip_id, id, entity, entity_id, field, old_value_json, new_value_json, author_id, source_kind, status, created_at, revision) VALUES ('trip-a', 'bad-field', 'trip', 'trip-a', 'name', '1', '2', 'leader', 'web', 'applied', '2026-08-07T12:00:00Z', 1)",
        "INSERT INTO content_edits (trip_id, id, entity, entity_id, field, old_value_json, new_value_json, author_id, source_kind, source_service_id, status, created_at, revision) VALUES ('trip-a', 'half-service', 'trip', 'trip-a', 'status', '\"dreaming\"', '\"planning\"', 'leader', 'service', 'service-a', 'applied', '2026-08-07T12:00:00Z', 1)",
        "INSERT INTO content_edits (trip_id, id, entity, entity_id, field, old_value_json, new_value_json, author_id, source_kind, status, created_at, reverted_by, reverted_at, revert_edit_id, revision) VALUES ('trip-a', 'bad-applied', 'trip', 'trip-a', 'status', '\"dreaming\"', '\"planning\"', 'leader', 'web', 'applied', '2026-08-07T12:00:00Z', 'leader', '2026-08-07T13:00:00Z', 'other', 1)",
        "INSERT INTO content_edits (trip_id, id, entity, entity_id, field, old_value_json, new_value_json, author_id, source_kind, status, created_at, revision) VALUES ('trip-a', 'same-value', 'trip', 'trip-a', 'status', '\"dreaming\"', '\"dreaming\"', 'leader', 'web', 'applied', '2026-08-07T12:00:00Z', 1)",
    ] {
        assert!(
            sqlx::query(statement)
                .execute(database.db.pool())
                .await
                .is_err(),
            "schema unexpectedly accepted {statement}"
        );
    }
    let strict: i64 =
        sqlx::query_scalar("SELECT strict FROM pragma_table_list WHERE name = 'content_edits'")
            .fetch_one(database.db.pool())
            .await
            .unwrap();
    assert_eq!(strict, 1);
    let self_fk_columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_foreign_key_list('content_edits') \
         WHERE \"table\" = 'content_edits'",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    let provenance_indexes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_index_list('content_edits') \
         WHERE \"unique\" = 1 AND partial = 1",
    )
    .fetch_one(database.db.pool())
    .await
    .unwrap();
    assert_eq!(self_fk_columns, 4);
    assert_eq!(provenance_indexes, 2);

    let content_bytes = MAX_RESPONSE_BYTES - 2;
    let mut multibyte_value = "界".repeat(content_bytes / "界".len());
    multibyte_value.push_str(&"x".repeat(content_bytes % "界".len()));
    let exact_utf8_json = serde_json::to_string(&multibyte_value).unwrap();
    assert_eq!(exact_utf8_json.len(), MAX_RESPONSE_BYTES);
    sqlx::query(
        "INSERT INTO content_edits ( \
             trip_id, id, entity, entity_id, field, old_value_json, new_value_json, \
             author_id, source_kind, status, created_at, revision \
         ) VALUES ('trip-a', 'exact-utf8', 'stop', 'stop-a', 'notes', ?, \
             '\"small\"', ?, 'web', 'applied', ?, 1)",
    )
    .bind(&exact_utf8_json)
    .bind(&leader.id.0)
    .bind(NOW)
    .execute(database.db.pool())
    .await
    .expect("the exact UTF-8 byte boundary is accepted by the schema");
    sqlx::query("DELETE FROM content_edits WHERE id = 'exact-utf8'")
        .execute(database.db.pool())
        .await
        .unwrap();
    multibyte_value.push('x');
    let oversized_utf8_json = serde_json::to_string(&multibyte_value).unwrap();
    assert_eq!(oversized_utf8_json.len(), MAX_RESPONSE_BYTES + 1);
    assert!(
        sqlx::query(
            "INSERT INTO content_edits ( \
                 trip_id, id, entity, entity_id, field, old_value_json, new_value_json, \
                 author_id, source_kind, status, created_at, revision \
             ) VALUES ('trip-a', 'oversized-utf8', 'stop', 'stop-a', 'notes', ?, \
                 '\"small\"', ?, 'web', 'applied', ?, 1)",
        )
        .bind(&oversized_utf8_json)
        .bind(&leader.id.0)
        .bind(NOW)
        .execute(database.db.pool())
        .await
        .is_err()
    );

    let mut orphan = trip_status_edit("orphan-provenance", &leader);
    orphan.reverts_edit_id = Some("missing-original".into());
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_edit_raw(&mut transaction, &orphan, 1).await;
    assert!(transaction.commit().await.is_err());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM content_edits")
            .fetch_one(database.db.pool())
            .await
            .unwrap(),
        0
    );

    let original = trip_status_edit("trip-a-original", &leader);
    let mut cross_trip = trip_status_edit("trip-b-compensation", &leader);
    cross_trip.trip_id = "trip-b".into();
    cross_trip.entity_id = "trip-b".into();
    cross_trip.reverts_edit_id = Some(original.id.clone());
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_edit_raw(&mut transaction, &original, 1).await;
    insert_edit_raw(&mut transaction, &cross_trip, 1).await;
    assert!(transaction.commit().await.is_err());

    let service_edit = Edit {
        id: "service-edit".into(),
        trip_id: "trip-a".into(),
        entity: EditEntity::Trip,
        entity_id: "trip-a".into(),
        field: "status".into(),
        old_value: json_value("dreaming"),
        new_value: json_value("planning"),
        author: leader.id.0.clone(),
        source: ChangeSource::Service {
            service_identity_id: "service-a".into(),
            service_identity_name: "Assistant".into(),
        },
        status: EditStatus::Applied,
        created_at: NOW.into(),
        reverted_by: None,
        reverted_at: None,
        revert_edit_id: None,
        reverts_edit_id: None,
    };
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_edit_raw(&mut transaction, &service_edit, 1).await;
    transaction.commit().await.unwrap();
    assert!(matches!(
        history.list_history("trip-a", &human(&leader)).await,
        Err(ContentHistoryRepoError::CorruptData)
    ));
    sqlx::query("DELETE FROM content_edits")
        .execute(database.db.pool())
        .await
        .unwrap();

    let mut staged = service_edit.clone();
    staged.source = ChangeSource::Web {};
    for (id, entity, entity_id, field, old_value, new_value) in [
        (
            "staged-notice",
            EditEntity::Notice,
            "notice-a",
            "title",
            json_value("Old"),
            json_value("New"),
        ),
        (
            "staged-in-plan",
            EditEntity::Candidate,
            "candidate-a",
            "status",
            json_value("shortlisted"),
            json_value("in_plan"),
        ),
        (
            "staged-ledger-link",
            EditEntity::Stop,
            "stop-a",
            "booking",
            serde_json::json!({
                "ref": "A", "url": null, "cost": null, "ledgerEntryId": "expense-a"
            }),
            serde_json::json!({
                "ref": "B", "url": null, "cost": null, "ledgerEntryId": "expense-a"
            }),
        ),
    ] {
        staged.id = id.into();
        staged.entity = entity;
        staged.entity_id = entity_id.into();
        staged.field = field.into();
        staged.old_value = old_value;
        staged.new_value = new_value;
        let mut transaction = database.db.pool().begin().await.unwrap();
        insert_edit_raw(&mut transaction, &staged, 1).await;
        transaction.commit().await.unwrap();
        assert!(matches!(
            history.list_history("trip-a", &human(&leader)).await,
            Err(ContentHistoryRepoError::CorruptData)
        ));
        sqlx::query("DELETE FROM content_edits")
            .execute(database.db.pool())
            .await
            .unwrap();
    }

    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Planning,
            "2026-08-07T13:00:00.000Z",
            "no-op",
        )
        .await
        .unwrap();
    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Booked,
            "2026-08-07T14:00:00.000Z",
            "canonical-edit",
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE content_edits SET old_value_json = old_value_json || ' ' \
         WHERE id = 'canonical-edit'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        history.list_history("trip-a", &human(&leader)).await,
        Err(ContentHistoryRepoError::CorruptData)
    ));

    database.shutdown().await;
}

#[tokio::test]
async fn reciprocal_chronological_and_acyclic_revert_provenance_is_mandatory() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let history = database.history();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Planning,
            NOW,
            "original",
        )
        .await
        .unwrap();
    history
        .revert_edit("trip-a", &human(&leader), "original", NOW, "compensation")
        .await
        .unwrap();
    sqlx::query(
        "UPDATE content_edits SET new_value_json = '\"booked\"' \
         WHERE trip_id = 'trip-a' AND id = 'compensation'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        history.list_history("trip-a", &human(&leader)).await,
        Err(ContentHistoryRepoError::CorruptData)
    ));
    database.shutdown().await;

    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let history = database.history();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Planning,
            NOW,
            "original",
        )
        .await
        .unwrap();
    history
        .revert_edit("trip-a", &human(&leader), "original", NOW, "compensation")
        .await
        .unwrap();
    sqlx::query(
        "UPDATE content_edits SET created_at = '2026-08-07T11:00:00.000Z' \
         WHERE trip_id = 'trip-a' AND id = 'compensation'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    sqlx::query(
        "UPDATE content_edits SET reverted_at = '2026-08-07T11:00:00.000Z' \
         WHERE trip_id = 'trip-a' AND id = 'original'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        history.list_history("trip-a", &human(&leader)).await,
        Err(ContentHistoryRepoError::CorruptData)
    ));
    database.shutdown().await;

    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let history = database.history();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Planning,
            NOW,
            "cycle-a",
        )
        .await
        .unwrap();
    history
        .revert_edit("trip-a", &human(&leader), "cycle-a", NOW, "cycle-b")
        .await
        .unwrap();
    history
        .revert_edit("trip-a", &human(&leader), "cycle-b", NOW, "cycle-c")
        .await
        .unwrap();
    history
        .revert_edit("trip-a", &human(&leader), "cycle-c", NOW, "cycle-d")
        .await
        .unwrap();
    let mut transaction = database.db.pool().begin().await.unwrap();
    sqlx::query(
        "UPDATE content_edits \
         SET status = 'reverted', reverted_by = 'leader', reverted_at = ?, \
             revert_edit_id = 'cycle-a', revision = revision + 1 \
         WHERE trip_id = 'trip-a' AND id = 'cycle-d'",
    )
    .bind(NOW)
    .execute(&mut *transaction)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE content_edits SET reverts_edit_id = 'cycle-d' \
         WHERE trip_id = 'trip-a' AND id = 'cycle-a'",
    )
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    assert!(matches!(
        history.list_history("trip-a", &human(&leader)).await,
        Err(ContentHistoryRepoError::CorruptData)
    ));
    database.shutdown().await;
}

#[tokio::test]
async fn exact_history_row_and_byte_limits_fail_closed_without_partial_results() {
    let database = TestDatabase::new().await;
    let history = database.history();
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;

    let mut transaction = database.db.pool().begin().await.unwrap();
    for index in 0..1_000 {
        let edit = trip_status_edit(&format!("count-{index:04}"), &leader);
        insert_edit_raw(&mut transaction, &edit, 1).await;
    }
    transaction.commit().await.unwrap();
    assert_eq!(
        history
            .list_history("trip-a", &human(&leader))
            .await
            .unwrap()
            .len(),
        1_000
    );
    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Dreaming,
            "2026-08-07T13:00:00.000Z",
            "count-no-op",
        )
        .await
        .expect("an exact no-op remains available at the ceiling");
    assert!(matches!(
        trips
            .set_trip_status(
                "trip-a",
                &human(&leader),
                TripStatus::Planning,
                "2026-08-07T13:00:00.000Z",
                "count-overflow",
            )
            .await,
        Err(TripRepoError::Conflict)
    ));
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_edit_raw(
        &mut transaction,
        &trip_status_edit("count-1000", &leader),
        1,
    )
    .await;
    transaction.commit().await.unwrap();
    assert!(matches!(
        history.list_history("trip-a", &human(&leader)).await,
        Err(ContentHistoryRepoError::SafetyLimitExceeded)
    ));

    sqlx::query("DELETE FROM content_edits")
        .execute(database.db.pool())
        .await
        .unwrap();
    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Planning,
            NOW,
            "ceiling-original",
        )
        .await
        .unwrap();
    history
        .revert_edit(
            "trip-a",
            &human(&leader),
            "ceiling-original",
            NOW,
            "ceiling-compensation",
        )
        .await
        .unwrap();
    let mut transaction = database.db.pool().begin().await.unwrap();
    for index in 0..998 {
        insert_edit_raw(
            &mut transaction,
            &trip_status_edit(&format!("ceiling-{index:03}"), &leader),
            1,
        )
        .await;
    }
    transaction.commit().await.unwrap();
    history
        .revert_edit(
            "trip-a",
            &human(&leader),
            "ceiling-original",
            "2026-08-07T13:00:00.000Z",
            "ignored-at-ceiling",
        )
        .await
        .expect("an already reverted command stays idempotent at the row ceiling");
    assert!(matches!(
        history
            .revert_edit(
                "trip-a",
                &human(&leader),
                "ceiling-compensation",
                "2026-08-07T13:00:00.000Z",
                "overflowing-compensation",
            )
            .await,
        Err(ContentHistoryRepoError::SafetyLimitExceeded)
    ));
    assert_eq!(
        trips
            .get_trip("trip-a", &human(&leader))
            .await
            .unwrap()
            .status(),
        TripStatus::Dreaming
    );

    sqlx::query("DELETE FROM content_edits")
        .execute(database.db.pool())
        .await
        .unwrap();
    let mut edits = (0..250)
        .map(|index| notes_edit(&format!("bytes-{index:03}"), &leader, "a", "b"))
        .collect::<Vec<_>>();
    pad_history_to_exact_limit(&mut edits);
    assert_eq!(
        serde_json::to_vec(&edits).unwrap().len(),
        MAX_RESPONSE_BYTES
    );
    let mut transaction = database.db.pool().begin().await.unwrap();
    for edit in &edits {
        insert_edit_raw(&mut transaction, edit, 1).await;
    }
    transaction.commit().await.unwrap();
    let exact = history
        .list_history("trip-a", &human(&leader))
        .await
        .expect("the exact byte ceiling is accepted");
    assert_eq!(
        serde_json::to_vec(&exact).unwrap().len(),
        MAX_RESPONSE_BYTES
    );

    let extendable = edits
        .iter()
        .find(|edit| edit.old_value.as_str().unwrap().len() < 10_000)
        .unwrap();
    let mut extended = extendable.old_value.as_str().unwrap().to_string();
    extended.push('x');
    sqlx::query("UPDATE content_edits SET old_value_json = ? WHERE trip_id = ? AND id = ?")
        .bind(serde_json::to_string(&extended).unwrap())
        .bind(&extendable.trip_id)
        .bind(&extendable.id)
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        history.list_history("trip-a", &human(&leader)).await,
        Err(ContentHistoryRepoError::SafetyLimitExceeded)
    ));

    sqlx::query("DELETE FROM content_edits")
        .execute(database.db.pool())
        .await
        .unwrap();
    let old = "a".repeat(5_000);
    let new = "b".repeat(5_000);
    let mut transaction = database.db.pool().begin().await.unwrap();
    for index in 0..450 {
        insert_edit_raw(
            &mut transaction,
            &notes_edit(&format!("raw-bytes-{index:03}"), &leader, &old, &new),
            1,
        )
        .await;
    }
    transaction.commit().await.unwrap();
    assert!(matches!(
        history.list_history("trip-a", &human(&leader)).await,
        Err(ContentHistoryRepoError::SafetyLimitExceeded)
    ));

    database.shutdown().await;
}

#[tokio::test]
async fn history_order_compares_timestamp_instants_before_ids() {
    let database = TestDatabase::new().await;
    let history = database.history();
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;

    let mut whole_second = trip_status_edit("whole-second", &leader);
    whole_second.created_at = "2026-08-07T12:00:00Z".into();
    let mut fractional = trip_status_edit("fractional", &leader);
    fractional.created_at = "2026-08-07T12:00:00.100Z".into();
    let mut transaction = database.db.pool().begin().await.unwrap();
    insert_edit_raw(&mut transaction, &whole_second, 1).await;
    insert_edit_raw(&mut transaction, &fractional, 1).await;
    transaction.commit().await.unwrap();

    let ids = history
        .list_history("trip-a", &human(&leader))
        .await
        .unwrap()
        .into_iter()
        .map(|edit| edit.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["fractional", "whole-second"]);

    database.shutdown().await;
}

#[tokio::test]
async fn concurrent_final_slot_and_role_revocation_are_serialized() {
    let database = Arc::new(TestDatabase::new().await);
    let trips = database.trips();
    let history = database.history();
    let users = database.users();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    add_membership(&database, "trip-a", &member, TripRole::Member).await;

    let mut transaction = database.db.pool().begin().await.unwrap();
    for index in 0..999 {
        insert_edit_raw(
            &mut transaction,
            &trip_status_edit(&format!("existing-{index:03}"), &leader),
            1,
        )
        .await;
    }
    transaction.commit().await.unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let mut writers = Vec::new();
    for (status, id) in [
        (TripStatus::Planning, "final-a"),
        (TripStatus::Booked, "final-b"),
    ] {
        let repo = trips.clone();
        let actor = leader.clone();
        let barrier = barrier.clone();
        writers.push(tokio::spawn(async move {
            barrier.wait().await;
            repo.set_trip_status(
                "trip-a",
                &human(&actor),
                status,
                "2026-08-07T13:00:00.000Z",
                id,
            )
            .await
        }));
    }
    barrier.wait().await;
    let mut results = Vec::new();
    for writer in writers {
        results.push(writer.await.unwrap());
    }
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(TripRepoError::Conflict)))
            .count(),
        1
    );
    assert_eq!(
        history
            .list_history("trip-a", &human(&leader))
            .await
            .unwrap()
            .len(),
        1_000
    );

    sqlx::query("DELETE FROM content_edits")
        .execute(database.db.pool())
        .await
        .unwrap();
    sqlx::query(
        "UPDATE trips SET status = 'dreaming', revision = revision + 1 WHERE id = 'trip-a'",
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
    let task = tokio::spawn(async move {
        repository
            .set_trip_status(
                "trip-a",
                &human(&actor),
                TripStatus::Planning,
                "2026-08-07T14:00:00.000Z",
                "revoked-edit",
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    sqlx::query("COMMIT").execute(&mut blocker).await.unwrap();
    blocker.close().await.unwrap();
    assert!(matches!(task.await.unwrap(), Err(TripRepoError::Forbidden)));
    assert!(
        history
            .list_history("trip-a", &human(&leader))
            .await
            .unwrap()
            .is_empty()
    );

    drop(history);
    drop(trips);
    drop(users);
    let database = Arc::try_unwrap(database)
        .ok()
        .expect("all test handles dropped");
    database.shutdown().await;
}

#[tokio::test]
async fn concurrent_revert_is_one_compensation_and_revocation_wins_before_authorization() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let history = database.history();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    let member = seed_user(&users, "member", "member@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;
    add_membership(&database, "trip-a", &member, TripRole::Member).await;
    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Planning,
            NOW,
            "original",
        )
        .await
        .unwrap();

    let barrier = Arc::new(Barrier::new(3));
    let mut tasks = Vec::new();
    for compensation_id in ["compensation-a", "compensation-b"] {
        let repository = history.clone();
        let actor = leader.clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            repository
                .revert_edit("trip-a", &human(&actor), "original", NOW, compensation_id)
                .await
        }));
    }
    barrier.wait().await;
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    let edits = history
        .list_history("trip-a", &human(&leader))
        .await
        .unwrap();
    assert_eq!(edits.len(), 2);
    assert_eq!(
        edits
            .iter()
            .filter(|edit| edit.reverts_edit_id.as_deref() == Some("original"))
            .count(),
        1
    );

    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Booked,
            "2026-08-07T13:00:00.000Z",
            "revocation-target",
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
    let repository = history.clone();
    let actor = member.clone();
    let task = tokio::spawn(async move {
        repository
            .revert_edit(
                "trip-a",
                &human(&actor),
                "revocation-target",
                "2026-08-07T14:00:00.000Z",
                "revoked-compensation",
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    sqlx::query("COMMIT").execute(&mut blocker).await.unwrap();
    blocker.close().await.unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(ContentHistoryRepoError::Forbidden)
    ));
    assert_eq!(
        trips
            .get_trip("trip-a", &human(&leader))
            .await
            .unwrap()
            .status(),
        TripStatus::Booked
    );

    database.shutdown().await;
}

#[tokio::test]
async fn revision_overflow_rolls_back_each_entity_and_its_history_append() {
    let database = TestDatabase::new().await;
    let trips = database.trips();
    let history = database.history();
    let (leader, _member, _viewer, _outsider, _anchor) = seed_content_graph(&database).await;

    for (table, predicate) in [
        ("candidates", "trip_id = 'trip-a' AND id = 'candidate-a'"),
        ("plan_days", "trip_id = 'trip-a' AND id = 'day-1'"),
        ("plan_stops", "trip_id = 'trip-a' AND id = 'stop-a'"),
    ] {
        let statement =
            format!("UPDATE {table} SET revision = 9223372036854775807 WHERE {predicate}");
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(database.db.pool())
            .await
            .unwrap();
        let result = match table {
            "candidates" => trips
                .set_candidate_status(
                    "trip-a",
                    &human(&leader),
                    "candidate-a",
                    CandidateDisposition::Rejected,
                    "2026-08-07T13:00:00.000Z",
                    "overflow-candidate",
                )
                .await
                .map(|_| ()),
            "plan_days" => trips
                .update_day(
                    "trip-a",
                    &human(&leader),
                    "day-1",
                    DayPatch {
                        window_start: Some("10:00".into()),
                        window_end: None,
                        city_hint: None,
                    },
                    "2026-08-07T13:00:00.000Z",
                    "overflow-day",
                )
                .await
                .map(|_| ()),
            "plan_stops" => trips
                .update_stop(
                    "trip-a",
                    &human(&leader),
                    "stop-a",
                    StopPatch {
                        planned_arrival: Some("11:00".into()),
                        duration_min: None,
                        notes: None,
                        booking: None,
                    },
                    "2026-08-07T13:00:00.000Z",
                    "overflow-stop",
                )
                .await
                .map(|_| ()),
            _ => unreachable!(),
        };
        assert!(matches!(result, Err(TripRepoError::CorruptData)));
        let reset = format!("UPDATE {table} SET revision = 1 WHERE {predicate}");
        sqlx::query(sqlx::AssertSqlSafe(reset))
            .execute(database.db.pool())
            .await
            .unwrap();
    }
    assert!(
        history
            .list_history("trip-a", &human(&leader))
            .await
            .unwrap()
            .is_empty()
    );

    sqlx::query("UPDATE trips SET revision = 9223372036854775807 WHERE id = 'trip-a'")
        .execute(database.db.pool())
        .await
        .unwrap();
    assert!(matches!(
        trips
            .set_trip_status(
                "trip-a",
                &human(&leader),
                TripStatus::Booked,
                "2026-08-07T14:00:00.000Z",
                "overflow-trip",
            )
            .await,
        Err(TripRepoError::CorruptData)
    ));
    sqlx::query("UPDATE trips SET revision = 2 WHERE id = 'trip-a'")
        .execute(database.db.pool())
        .await
        .unwrap();

    trips
        .set_trip_status(
            "trip-a",
            &human(&leader),
            TripStatus::Booked,
            "2026-08-07T15:00:00.000Z",
            "revert-overflow",
        )
        .await
        .unwrap();
    sqlx::query(
        "UPDATE content_edits SET revision = 9223372036854775807 \
         WHERE trip_id = 'trip-a' AND id = 'revert-overflow'",
    )
    .execute(database.db.pool())
    .await
    .unwrap();
    assert!(matches!(
        history
            .revert_edit(
                "trip-a",
                &human(&leader),
                "revert-overflow",
                "2026-08-07T16:00:00.000Z",
                "overflow-compensation",
            )
            .await,
        Err(ContentHistoryRepoError::CorruptData)
    ));
    assert_eq!(
        trips
            .get_trip("trip-a", &human(&leader))
            .await
            .unwrap()
            .status(),
        TripStatus::Booked
    );
    assert_eq!(
        history
            .list_history("trip-a", &human(&leader))
            .await
            .unwrap()
            .len(),
        1
    );

    database.shutdown().await;
}

fn trip_status_edit(id: &str, author: &User) -> Edit {
    Edit {
        id: id.into(),
        trip_id: "trip-a".into(),
        entity: EditEntity::Trip,
        entity_id: "trip-a".into(),
        field: "status".into(),
        old_value: json_value("dreaming"),
        new_value: json_value("planning"),
        author: author.id.0.clone(),
        source: ChangeSource::Web {},
        status: EditStatus::Applied,
        created_at: NOW.into(),
        reverted_by: None,
        reverted_at: None,
        revert_edit_id: None,
        reverts_edit_id: None,
    }
}

fn notes_edit(id: &str, author: &User, old: &str, new: &str) -> Edit {
    Edit {
        id: id.into(),
        trip_id: "trip-a".into(),
        entity: EditEntity::Stop,
        entity_id: "stop-a".into(),
        field: "notes".into(),
        old_value: json_value(old),
        new_value: json_value(new),
        author: author.id.0.clone(),
        source: ChangeSource::Web {},
        status: EditStatus::Applied,
        created_at: NOW.into(),
        reverted_by: None,
        reverted_at: None,
        revert_edit_id: None,
        reverts_edit_id: None,
    }
}

fn json_value(value: &str) -> serde_json::Value {
    serde_json::Value::String(value.into())
}

fn pad_history_to_exact_limit(edits: &mut [Edit]) {
    let mut remaining = MAX_RESPONSE_BYTES - serde_json::to_vec(edits).unwrap().len();
    for edit in edits {
        for value in [&mut edit.old_value, &mut edit.new_value] {
            let current = value.as_str().unwrap();
            let added = remaining.min(10_000 - current.len());
            if added == 0 {
                continue;
            }
            let mut padded = current.to_string();
            padded.push_str(&"x".repeat(added));
            *value = json_value(&padded);
            remaining -= added;
            if remaining == 0 {
                return;
            }
        }
    }
    panic!("history fixtures do not have enough safe padding capacity");
}

async fn insert_edit_raw(transaction: &mut Transaction<'_, Sqlite>, edit: &Edit, revision: i64) {
    let (source_kind, source_service_id, source_service_name) = match &edit.source {
        ChangeSource::Web {} => ("web", None, None),
        ChangeSource::Service {
            service_identity_id,
            service_identity_name,
        } => (
            "service",
            Some(service_identity_id.as_str()),
            Some(service_identity_name.as_str()),
        ),
    };
    sqlx::query(
        "INSERT INTO content_edits ( \
             trip_id, id, entity, entity_id, field, old_value_json, new_value_json, \
             author_id, source_kind, source_service_id, source_service_name, status, \
             created_at, reverted_by, reverted_at, revert_edit_id, reverts_edit_id, revision \
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&edit.trip_id)
    .bind(&edit.id)
    .bind(match edit.entity {
        EditEntity::Stop => "stop",
        EditEntity::Day => "day",
        EditEntity::Candidate => "candidate",
        EditEntity::Notice => "notice",
        EditEntity::Trip => "trip",
    })
    .bind(&edit.entity_id)
    .bind(&edit.field)
    .bind(serde_json::to_string(&edit.old_value).unwrap())
    .bind(serde_json::to_string(&edit.new_value).unwrap())
    .bind(&edit.author)
    .bind(source_kind)
    .bind(source_service_id)
    .bind(source_service_name)
    .bind(match edit.status {
        EditStatus::Applied => "applied",
        EditStatus::Reverted => "reverted",
        EditStatus::PendingReview => "pending_review",
        EditStatus::Rejected => "rejected",
    })
    .bind(&edit.created_at)
    .bind(&edit.reverted_by)
    .bind(&edit.reverted_at)
    .bind(&edit.revert_edit_id)
    .bind(&edit.reverts_edit_id)
    .bind(revision)
    .execute(&mut **transaction)
    .await
    .unwrap();
}

async fn insert_stop(
    database: &TestDatabase,
    trip_id: &str,
    day_id: &str,
    stop_id: &str,
    place_id: &str,
) {
    let mut transaction = database.db.pool().begin().await.unwrap();
    sqlx::query("INSERT INTO stop_identities (trip_id, id) VALUES (?, ?)")
        .bind(trip_id)
        .bind(stop_id)
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO plan_stops ( \
             trip_id, plan_version, id, day_id, seq, place_id, stop_kind, \
             planned_arrival, duration_min, notes, revision \
         ) VALUES (?, 1, ?, ?, 1, ?, 'visit', '10:00', 60, '', 1)",
    )
    .bind(trip_id)
    .bind(stop_id)
    .bind(day_id)
    .bind(place_id)
    .execute(&mut *transaction)
    .await
    .unwrap();
    transaction.commit().await.unwrap();
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
