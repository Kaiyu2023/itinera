use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use aws_sdk_dynamodb::{
    operation::{
        get_item::GetItemOutput,
        query::QueryOutput,
        transact_write_items::{TransactWriteItemsError, TransactWriteItemsOutput},
    },
    types::{CancellationReason, error::TransactionCanceledException},
};
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use itinera_core::domain::trip::{PlaceKind, StopKind};
use serde_json::json;

use super::*;
use crate::dynamodb::trip_repo::records::encode_member;

const TABLE: &str = "itinera-test";
const TRIP_ID: &str = "trip-a";
const ACTOR_ID: &str = "user-a";
const CREATED_AT: &str = "2026-08-06T10:00:00Z";
const REVERTED_AT: &str = "2026-08-06T11:00:00Z";

fn actor() -> UserId {
    UserId(ACTOR_ID.into())
}

fn member(role: TripRole) -> TripMember {
    TripMember {
        user_id: ACTOR_ID.into(),
        role,
        joined_at: "2026-08-01T00:00:00Z".into(),
    }
}

fn member_item(role: TripRole) -> HashMap<String, AttributeValue> {
    encode_member(TRIP_ID, &member(role)).expect("member encodes")
}

fn meta(status: TripStatus) -> TripMeta {
    TripMeta {
        id: TRIP_ID.into(),
        name: "Japan".into(),
        cover_photo_url: None,
        accent_color: None,
        stop_kind_labels: None,
        status,
        start_date: "2026-11-01".into(),
        end_date: "2026-11-03".into(),
        base_currency: "GBP".into(),
        soft_budget: None,
        current_plan_id: None,
        current_plan_version: None,
        created_at: "2026-08-01T00:00:00Z".into(),
        member_count: 1,
        leader_count: 1,
        cities: vec![],
    }
}

fn current_plan_meta(status: TripStatus) -> TripMeta {
    TripMeta {
        current_plan_id: Some("plan-a".into()),
        current_plan_version: Some(1),
        cities: vec!["Kyoto".into()],
        ..meta(status)
    }
}

fn applied_edit() -> Edit {
    Edit {
        id: "edit-a".into(),
        trip_id: TRIP_ID.into(),
        entity: EditEntity::Trip,
        entity_id: TRIP_ID.into(),
        field: "status".into(),
        old_value: json!(TripStatus::Dreaming),
        new_value: json!(TripStatus::Booked),
        author: "author-a".into(),
        source: ChangeSource::Web,
        status: EditStatus::Applied,
        created_at: CREATED_AT.into(),
        reverted_by: None,
        reverted_at: None,
        revert_edit_id: None,
        reverts_edit_id: None,
    }
}

fn reverted_edit() -> Edit {
    Edit {
        status: EditStatus::Reverted,
        reverted_by: Some(ACTOR_ID.into()),
        reverted_at: Some(REVERTED_AT.into()),
        revert_edit_id: Some("revert-a".into()),
        ..applied_edit()
    }
}

fn compensating_edit() -> Edit {
    audit::compensating_edit(&applied_edit(), &actor(), REVERTED_AT, "revert-a")
}

fn completed_revert_items() -> Vec<HashMap<String, AttributeValue>> {
    vec![
        edit_item(&reverted_edit(), 3),
        edit_item(&compensating_edit(), 1),
    ]
}

fn edit_item(edit: &Edit, revision: u64) -> HashMap<String, AttributeValue> {
    encode_record(
        trip_pk(&edit.trip_id),
        audit_sk(&edit.created_at, &edit.id),
        AUDIT_ENTITY,
        edit,
        revision,
    )
    .expect("edit encodes")
}

fn membership_rule(role: TripRole) -> aws_smithy_mocks::Rule {
    let item = member_item(role);
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(format!("MEMBER#{ACTOR_ID}")))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn meta_rule(status: TripStatus, revision: u64) -> aws_smithy_mocks::Rule {
    let item = encode_trip_meta(&meta(status), revision).expect("meta encodes");
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(META_SK.into()))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn current_plan_meta_rule(status: TripStatus, revision: u64) -> aws_smithy_mocks::Rule {
    let item = encode_trip_meta(&current_plan_meta(status), revision).expect("meta encodes");
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(META_SK.into()))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn audit_rule(edits: Vec<HashMap<String, AttributeValue>>) -> aws_smithy_mocks::Rule {
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.scan_index_forward() == Some(false)
                && request.limit() == Some(access::HISTORY_PAGE_SIZE)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":pk"))
                    == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("AUDIT#".into()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(edits.clone()))
                .build()
        })
}

fn cancelled_transaction(codes: &[&str]) -> TransactWriteItemsError {
    let mut builder = TransactionCanceledException::builder();
    for code in codes {
        builder = builder.cancellation_reasons(CancellationReason::builder().code(*code).build());
    }
    TransactWriteItemsError::TransactionCanceledException(builder.build())
}

#[tokio::test]
async fn every_direct_member_role_can_view_strongly_scoped_history() {
    for role in [TripRole::Viewer, TripRole::Member, TripRole::Leader] {
        let member = membership_rule(role);
        let meta = meta_rule(TripStatus::Booked, 7);
        let edit = applied_edit();
        let audit = audit_rule(vec![edit_item(&edit, 2)]);
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&member, &meta, &audit]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        let history = repo
            .list_history(TRIP_ID, &actor())
            .await
            .expect("direct members may view history");

        assert_eq!(history, vec![edit]);
        assert_eq!(member.num_calls(), 1);
        assert_eq!(meta.num_calls(), 1);
        assert_eq!(audit.num_calls(), 1);
    }
}

#[tokio::test]
async fn viewer_can_read_but_cannot_revert() {
    let member = membership_rule(TripRole::Viewer);
    let client = mock_client!(aws_sdk_dynamodb, [&member]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let error = repo
        .revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
        .await
        .expect_err("viewer is read-only");

    assert_eq!(error, ContentHistoryRepoError::Forbidden);
    assert_eq!(member.num_calls(), 1);
}

#[tokio::test]
async fn leaders_and_members_revert_with_exact_atomic_guards_and_provenance() {
    for role in [TripRole::Member, TripRole::Leader] {
        let member = membership_rule(role);
        let meta = meta_rule(TripStatus::Booked, 7);
        let original = applied_edit();
        let audit = audit_rule(vec![edit_item(&original, 2)]);
        let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
            .match_requests(|request| {
                let items = request.transact_items();
                if items.len() != 4 {
                    return false;
                }
                let membership_ok = items[0].condition_check().is_some_and(|condition| {
                    condition.key().get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && condition.key().get(SK)
                            == Some(&AttributeValue::S(format!("MEMBER#{ACTOR_ID}")))
                        && condition.condition_expression()
                            == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                });
                let target_ok = items[1].put().is_some_and(|put| {
                    let decoded = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<TripMeta>(data).ok());
                    put.item().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("8".into()))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :expected_revision AND #data = :expected_data")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":expected_revision"))
                            == Some(&AttributeValue::N("7".into()))
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":expected_data"))
                            .and_then(|value| value.as_s().ok())
                            .is_some_and(|data| data.contains("\"status\":\"booked\""))
                        && decoded.is_some_and(|meta| meta.status == TripStatus::Dreaming)
                });
                let original_ok = items[2].put().is_some_and(|put| {
                    let decoded = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Edit>(data).ok());
                    put.item().get(SK)
                        == Some(&AttributeValue::S(audit_sk(CREATED_AT, "edit-a")))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("3".into()))
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":expected_revision"))
                            == Some(&AttributeValue::N("2".into()))
                        && decoded.is_some_and(|edit| {
                            edit.status == EditStatus::Reverted
                                && edit.reverted_by.as_deref() == Some(ACTOR_ID)
                                && edit.reverted_at.as_deref() == Some(REVERTED_AT)
                                && edit.revert_edit_id.as_deref() == Some("revert-a")
                                && edit.old_value == json!(TripStatus::Dreaming)
                                && edit.new_value == json!(TripStatus::Booked)
                        })
                });
                let compensation_ok = items[3].put().is_some_and(|put| {
                    let decoded = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Edit>(data).ok());
                    put.item().get(SK)
                        == Some(&AttributeValue::S(audit_sk(REVERTED_AT, "revert-a")))
                        && put.condition_expression()
                            == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                        && decoded.is_some_and(|edit| {
                            edit.id == "revert-a"
                                && edit.status == EditStatus::Applied
                                && edit.author == ACTOR_ID
                                && edit.reverts_edit_id.as_deref() == Some("edit-a")
                                && edit.old_value == json!(TripStatus::Booked)
                                && edit.new_value == json!(TripStatus::Dreaming)
                        })
                });
                membership_ok && target_ok && original_ok && compensation_ok
            })
            .then_output(|| TransactWriteItemsOutput::builder().build());
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&member, &meta, &audit, &transaction]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
            .await
            .expect("editor revert succeeds");

        assert_eq!(transaction.num_calls(), 1);
    }
}

#[tokio::test]
async fn an_edit_id_from_another_trip_is_not_found_inside_the_route_partition() {
    let member = membership_rule(TripRole::Leader);
    let audit = audit_rule(vec![]);
    let client = mock_client!(aws_sdk_dynamodb, RuleMode::MatchAny, [&member, &audit]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let error = repo
        .revert_edit(TRIP_ID, &actor(), "foreign-edit", REVERTED_AT, "revert-a")
        .await
        .expect_err("foreign id must not be looked up globally");

    assert_eq!(error, ContentHistoryRepoError::NotFound);
    assert_eq!(audit.num_calls(), 1);
}

#[tokio::test]
async fn malformed_and_cross_trip_audit_rows_fail_closed() {
    for corrupt in [
        Edit {
            trip_id: "trip-b".into(),
            ..applied_edit()
        },
        Edit {
            status: EditStatus::Reverted,
            ..applied_edit()
        },
        Edit {
            created_at: "not-a-timestamp".into(),
            ..applied_edit()
        },
        // Complete-looking local provenance is still corrupt without the
        // reciprocal compensation in this trip's audit partition.
        reverted_edit(),
    ] {
        let member = membership_rule(TripRole::Viewer);
        let meta = meta_rule(TripStatus::Booked, 7);
        // Keep the physical key in trip-a so decoding reaches the embedded
        // ownership/provenance validation rather than merely missing the row.
        let item = encode_record(
            trip_pk(TRIP_ID),
            audit_sk(&corrupt.created_at, &corrupt.id),
            AUDIT_ENTITY,
            &corrupt,
            1,
        )
        .expect("corrupt fixture still serializes");
        let audit = audit_rule(vec![item]);
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&member, &meta, &audit]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        assert_eq!(
            repo.list_history(TRIP_ID, &actor())
                .await
                .expect_err("corrupt audit must fail closed"),
            ContentHistoryRepoError::CorruptData
        );
    }
}

#[tokio::test]
async fn mismatched_revert_provenance_fails_closed() {
    let member = membership_rule(TripRole::Viewer);
    let meta = meta_rule(TripStatus::Booked, 7);
    let mut compensation = compensating_edit();
    compensation.field = "name".into();
    let audit = audit_rule(vec![
        edit_item(&reverted_edit(), 3),
        edit_item(&compensation, 1),
    ]);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &meta, &audit]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.list_history(TRIP_ID, &actor())
            .await
            .expect_err("mismatched reciprocal provenance must fail closed"),
        ContentHistoryRepoError::CorruptData
    );
}

#[tokio::test]
async fn provenance_validation_retries_a_paginated_read_that_straddles_a_revert() {
    let member = membership_rule(TripRole::Viewer);
    let meta = meta_rule(TripStatus::Booked, 7);
    let mut filler = applied_edit();
    filler.id = "edit-filler".into();
    filler.created_at = "2026-08-06T10:30:00Z".into();

    let cursor = HashMap::from([
        (PK.to_string(), AttributeValue::S(trip_pk(TRIP_ID))),
        (
            SK.to_string(),
            AttributeValue::S(audit_sk(&filler.created_at, &filler.id)),
        ),
    ]);
    let first_cursor = cursor.clone();
    let first_page_reads = Arc::new(AtomicUsize::new(0));
    let first_page_counter = Arc::clone(&first_page_reads);
    let filler_item = edit_item(&filler, 1);
    let compensation_item = edit_item(&compensating_edit(), 1);
    let first_page = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| request.exclusive_start_key().is_none())
        .then_output(move || {
            let items = if first_page_counter.fetch_add(1, Ordering::SeqCst) == 0 {
                // The first pass started before the atomic revert committed,
                // so its first page does not contain the new compensation.
                vec![filler_item.clone()]
            } else {
                vec![compensation_item.clone(), filler_item.clone()]
            };
            QueryOutput::builder()
                .set_items(Some(items))
                .set_last_evaluated_key(Some(first_cursor.clone()))
                .build()
        });
    let second_cursor = cursor.clone();
    let original_item = edit_item(&reverted_edit(), 3);
    let second_page = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| request.exclusive_start_key() == Some(&second_cursor))
        .then_output(move || {
            // Both passes reach this page after the atomic revert, so the
            // original already points at its compensation.
            QueryOutput::builder().items(original_item.clone()).build()
        });
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &meta, &first_page, &second_page]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let history = repo
        .list_history(TRIP_ID, &actor())
        .await
        .expect("the complete retry sees valid reciprocal provenance");

    assert_eq!(history.len(), 3);
    assert!(history.iter().any(|edit| edit.id == "edit-a"));
    assert!(history.iter().any(|edit| edit.id == "revert-a"));
    assert!(history.iter().any(|edit| edit.id == "edit-filler"));
    assert_eq!(first_page.num_calls(), 2);
    assert_eq!(second_page.num_calls(), 2);
    assert_eq!(first_page_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn history_reads_stop_at_the_documented_safety_ceiling() {
    let member = membership_rule(TripRole::Viewer);
    let meta = meta_rule(TripStatus::Booked, 7);
    let oversized_page = vec![HashMap::new(); access::MAX_HISTORY_RECORDS + 1];
    let audit = mock!(aws_sdk_dynamodb::Client::query).then_output(move || {
        QueryOutput::builder()
            .set_items(Some(oversized_page.clone()))
            .build()
    });
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &meta, &audit]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.list_history(TRIP_ID, &actor())
            .await
            .expect_err("history must not grow memory without a hard ceiling"),
        ContentHistoryRepoError::SafetyLimitExceeded
    );
}

#[tokio::test]
async fn unsupported_entities_and_fields_never_become_arbitrary_writes() {
    for unsupported in [
        Edit {
            entity: EditEntity::Notice,
            entity_id: "notice-a".into(),
            field: "body".into(),
            old_value: json!("old"),
            new_value: json!("new"),
            ..applied_edit()
        },
        Edit {
            field: "name".into(),
            old_value: json!("Japan"),
            new_value: json!("Japan 2026"),
            ..applied_edit()
        },
    ] {
        let member = membership_rule(TripRole::Leader);
        let audit = audit_rule(vec![edit_item(&unsupported, 1)]);
        let client = mock_client!(aws_sdk_dynamodb, RuleMode::MatchAny, [&member, &audit]);
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        assert_eq!(
            repo.revert_edit(TRIP_ID, &actor(), &unsupported.id, REVERTED_AT, "revert-a",)
                .await
                .expect_err("unsupported target must not write"),
            ContentHistoryRepoError::Unsupported
        );
    }
}

#[tokio::test]
async fn malformed_supported_values_and_stale_current_values_are_distinct() {
    let malformed = Edit {
        new_value: json!(42),
        ..applied_edit()
    };
    let member = membership_rule(TripRole::Leader);
    let audit = audit_rule(vec![edit_item(&malformed, 1)]);
    let client = mock_client!(aws_sdk_dynamodb, RuleMode::MatchAny, [&member, &audit]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
            .await
            .expect_err("wrong value type is corrupt"),
        ContentHistoryRepoError::CorruptData
    );

    let member = membership_rule(TripRole::Leader);
    let audit = audit_rule(vec![edit_item(&applied_edit(), 1)]);
    let meta = meta_rule(TripStatus::Planning, 8);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &audit, &meta]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
            .await
            .expect_err("changed field is stale"),
        ContentHistoryRepoError::Conflict
    );
}

#[tokio::test]
async fn revision_overflow_is_corrupt_data_instead_of_wrapping() {
    for (audit_revision, meta_revision) in [(u64::MAX, 7), (1, u64::MAX)] {
        let member = membership_rule(TripRole::Leader);
        let audit = audit_rule(vec![edit_item(&applied_edit(), audit_revision)]);
        let meta = meta_rule(TripStatus::Booked, meta_revision);
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&member, &audit, &meta]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        assert_eq!(
            repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
                .await
                .expect_err("revision overflow must fail closed"),
            ContentHistoryRepoError::CorruptData
        );
    }
}

#[tokio::test]
async fn a_repeated_revert_is_idempotent_without_another_transaction() {
    let member = membership_rule(TripRole::Member);
    let audit = audit_rule(completed_revert_items());
    let client = mock_client!(aws_sdk_dynamodb, RuleMode::MatchAny, [&member, &audit]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    repo.revert_edit(
        TRIP_ID,
        &actor(),
        "edit-a",
        "2026-08-06T12:00:00Z",
        "another-id",
    )
    .await
    .expect("already-reverted edit is a successful no-op");

    assert_eq!(member.num_calls(), 1);
    assert_eq!(audit.num_calls(), 1);
}

#[tokio::test]
async fn day_city_revert_ignores_valid_nested_stops_and_recomputes_trip_cities() {
    let member = membership_rule(TripRole::Member);
    let mut stored_meta = current_plan_meta(TripStatus::Planning);
    stored_meta.cities = vec!["Osaka".into(), "Tokyo".into()];
    let meta_item = encode_trip_meta(&stored_meta, 11).expect("meta encodes");
    let meta = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(META_SK.into()))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(meta_item.clone()))
                .build()
        });
    let edit = Edit {
        entity: EditEntity::Day,
        entity_id: "day-a".into(),
        field: "cityHint".into(),
        old_value: json!("Kyoto"),
        new_value: json!("Osaka"),
        ..applied_edit()
    };
    let audit = audit_rule(vec![edit_item(&edit, 4)]);
    let day_a = Day {
        id: "day-a".into(),
        plan_id: "plan-a".into(),
        date: "2026-11-01".into(),
        city_hint: "Osaka".into(),
        tz: "Asia/Tokyo".into(),
        window_start: "09:00".into(),
        window_end: "21:00".into(),
    };
    let day_b = Day {
        id: "day-b".into(),
        plan_id: "plan-a".into(),
        date: "2026-11-02".into(),
        city_hint: "Tokyo".into(),
        tz: "Asia/Tokyo".into(),
        window_start: "09:00".into(),
        window_end: "21:00".into(),
    };
    let nested_stop = Stop {
        id: "stop-a".into(),
        day_id: "day-a".into(),
        seq: 1.0,
        place_id: "place-a".into(),
        stop_kind: StopKind::Visit,
        planned_arrival: "10:00".into(),
        duration_min: 60,
        booking: None,
        notes: String::new(),
    };
    let day_a_item = encode_record(
        trip_pk(TRIP_ID),
        "PLAN#0000000001#DAY#2026-11-01#day-a".into(),
        DAY_ENTITY,
        &day_a,
        5,
    )
    .expect("day encodes");
    let stop_item = encode_record(
        trip_pk(TRIP_ID),
        "PLAN#0000000001#DAY#2026-11-01#STOP#000001#stop-a".into(),
        STOP_ENTITY,
        &nested_stop,
        2,
    )
    .expect("stop encodes");
    let day_b_item = encode_record(
        trip_pk(TRIP_ID),
        "PLAN#0000000001#DAY#2026-11-02#day-b".into(),
        DAY_ENTITY,
        &day_b,
        3,
    )
    .expect("day encodes");
    let plan_query = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("PLAN#0000000001#DAY#".into()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(vec![
                    day_a_item.clone(),
                    stop_item.clone(),
                    day_b_item.clone(),
                ]))
                .build()
        });
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 5
                && items[1].put().is_some_and(|put| {
                    let day = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Day>(data).ok());
                    put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :expected_revision AND #data = :expected_data")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":expected_data"))
                            .and_then(|value| value.as_s().ok())
                            .is_some_and(|data| data.contains("\"cityHint\":\"Osaka\""))
                        && day.is_some_and(|day| day.city_hint == "Kyoto")
                })
                && items[2].put().is_some_and(|put| {
                    let meta = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<TripMeta>(data).ok());
                    put.item().get(REVISION) == Some(&AttributeValue::N("12".into()))
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":expected_revision"))
                            == Some(&AttributeValue::N("11".into()))
                        && meta.is_some_and(|meta| meta.cities == ["Kyoto", "Tokyo"])
                })
                && items[3].put().is_some()
                && items[4].put().is_some()
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &meta, &audit, &plan_query, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
        .await
        .expect("nested stop rows must not break a day revert");

    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn stop_revert_pins_the_exact_field_payload_revision_and_current_plan() {
    let member = membership_rule(TripRole::Member);
    let meta = current_plan_meta_rule(TripStatus::Planning, 11);
    let edit = Edit {
        entity: EditEntity::Stop,
        entity_id: "stop-a".into(),
        field: "notes".into(),
        old_value: json!("old note"),
        new_value: json!("new note"),
        ..applied_edit()
    };
    let audit = audit_rule(vec![edit_item(&edit, 4)]);
    let day = Day {
        id: "day-a".into(),
        plan_id: "plan-a".into(),
        date: "2026-11-01".into(),
        city_hint: "Kyoto".into(),
        tz: "Asia/Tokyo".into(),
        window_start: "09:00".into(),
        window_end: "21:00".into(),
    };
    let stop = Stop {
        id: "stop-a".into(),
        day_id: "day-a".into(),
        seq: 1.0,
        place_id: "place-a".into(),
        stop_kind: StopKind::Visit,
        planned_arrival: "10:00".into(),
        duration_min: 60,
        booking: None,
        notes: "new note".into(),
    };
    let day_item = encode_record(
        trip_pk(TRIP_ID),
        "PLAN#0000000001#DAY#2026-11-01#day-a".into(),
        DAY_ENTITY,
        &day,
        2,
    )
    .expect("day encodes");
    let stop_item = encode_record(
        trip_pk(TRIP_ID),
        "PLAN#0000000001#DAY#2026-11-01#STOP#000001#stop-a".into(),
        STOP_ENTITY,
        &stop,
        5,
    )
    .expect("stop encodes");
    let plan_query = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.scan_index_forward() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("PLAN#0000000001#".into()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(vec![day_item.clone(), stop_item.clone()]))
                .build()
        });
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 5
                && items[1].put().is_some_and(|put| {
                    let decoded = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Stop>(data).ok());
                    put.item().get(SK)
                        == Some(&AttributeValue::S(
                            "PLAN#0000000001#DAY#2026-11-01#STOP#000001#stop-a".into(),
                        ))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :expected_revision AND #data = :expected_data")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":expected_revision"))
                            == Some(&AttributeValue::N("5".into()))
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":expected_data"))
                            .and_then(|value| value.as_s().ok())
                            .is_some_and(|data| data.contains("\"notes\":\"new note\""))
                        && decoded.is_some_and(|stop| stop.notes == "old note")
                })
                && items[2].condition_check().is_some_and(|guard| {
                    guard.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && guard.condition_expression()
                            == "#entity = :entity AND #revision = :expected_revision AND #data = :expected_data"
                        && guard
                            .expression_attribute_values()
                            .and_then(|values| values.get(":expected_revision"))
                            == Some(&AttributeValue::N("11".into()))
                })
                && items[3].put().is_some()
                && items[4].put().is_some()
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &meta, &audit, &plan_query, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
        .await
        .expect("stop note reverts");

    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn candidate_place_revert_repoints_without_mutating_and_guards_both_snapshots() {
    fn place(id: &str, name: &str) -> Place {
        Place {
            id: id.into(),
            name: name.into(),
            kind: PlaceKind::Sight,
            lat: 35.0,
            lng: 135.0,
            tz: "Asia/Tokyo".into(),
            country_code: "JP".into(),
            admin_area: "Kyoto".into(),
            city: "Kyoto".into(),
            address: String::new(),
            external_ref: None,
            website: None,
            phone: None,
            rating: None,
            price_level: None,
            opening_hours: None,
            photo_urls: vec![],
            guide: None,
        }
    }

    let previous = place("place-old", "Old name");
    let current = place("place-new", "New name");
    let edit = Edit {
        entity: EditEntity::Candidate,
        entity_id: "candidate-a".into(),
        field: "place".into(),
        old_value: serde_json::to_value(&previous).expect("place serializes"),
        new_value: serde_json::to_value(&current).expect("place serializes"),
        ..applied_edit()
    };
    let candidate = Candidate {
        id: "candidate-a".into(),
        trip_id: TRIP_ID.into(),
        source_place_id: None,
        place_id: current.id.clone(),
        proposed_by: "author-a".into(),
        created_at: CREATED_AT.into(),
        pitch: "Visit".into(),
        tags: vec![],
        status: CandidateStatus::Shortlisted,
    };
    let candidate_item = encode_record(
        trip_pk(TRIP_ID),
        candidate_sk(&candidate.id),
        CANDIDATE_ENTITY,
        &candidate,
        9,
    )
    .expect("candidate encodes");
    let current_item = encode_record(
        trip_pk(TRIP_ID),
        place_sk(&current.id),
        PLACE_ENTITY,
        &current,
        3,
    )
    .expect("place encodes");
    let previous_item = encode_record(
        trip_pk(TRIP_ID),
        place_sk(&previous.id),
        PLACE_ENTITY,
        &previous,
        2,
    )
    .expect("place encodes");
    let member = membership_rule(TripRole::Leader);
    let audit = audit_rule(vec![edit_item(&edit, 1)]);
    let candidate_read = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK) == Some(&AttributeValue::S("CANDIDATE#candidate-a".into()))
            })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(candidate_item.clone()))
                .build()
        });
    let current_read = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK) == Some(&AttributeValue::S("PLACE#place-new".into()))
            })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(current_item.clone()))
                .build()
        });
    let previous_read = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().is_some_and(|key| {
                key.get(SK) == Some(&AttributeValue::S("PLACE#place-old".into()))
            })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(previous_item.clone()))
                .build()
        });
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 6
                && items[1].put().is_some_and(|put| {
                    let candidate = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Candidate>(data).ok());
                    put.item().get(REVISION) == Some(&AttributeValue::N("10".into()))
                        && candidate.is_some_and(|candidate| candidate.place_id == "place-old")
                })
                && items[2].condition_check().is_some_and(|guard| {
                    guard.key().get(SK) == Some(&AttributeValue::S("PLACE#place-new".into()))
                        && guard
                            .expression_attribute_values()
                            .and_then(|values| values.get(":expected_revision"))
                            == Some(&AttributeValue::N("3".into()))
                })
                && items[3].condition_check().is_some_and(|guard| {
                    guard.key().get(SK) == Some(&AttributeValue::S("PLACE#place-old".into()))
                        && guard
                            .expression_attribute_values()
                            .and_then(|values| values.get(":expected_revision"))
                            == Some(&AttributeValue::N("2".into()))
                })
                && items[4].put().is_some()
                && items[5].put().is_some()
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &member,
            &audit,
            &candidate_read,
            &current_read,
            &previous_read,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
        .await
        .expect("candidate snapshot repoints safely");

    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn a_concurrent_revert_winner_makes_the_loser_succeed_idempotently() {
    let member = membership_rule(TripRole::Leader);
    let meta = meta_rule(TripStatus::Booked, 7);
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    let applied = edit_item(&applied_edit(), 2);
    let reverted = completed_revert_items();
    let audit = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request
                .expression_attribute_values()
                .and_then(|values| values.get(":prefix"))
                == Some(&AttributeValue::S("AUDIT#".into()))
        })
        .then_output(move || {
            let items = if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![applied.clone()]
            } else {
                reverted.clone()
            };
            QueryOutput::builder().set_items(Some(items)).build()
        });
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_error(|| cancelled_transaction(&["None", CONDITIONAL_FAILURE, "None", "None"]));
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &meta, &audit, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "loser-revert")
        .await
        .expect("concurrent winner makes the command complete");

    assert_eq!(transaction.num_calls(), 1);
    assert_eq!(
        member.num_calls(),
        2,
        "role is rechecked after cancellation"
    );
    assert_eq!(reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_concurrent_entity_change_that_did_not_revert_the_edit_is_a_conflict() {
    let member = membership_rule(TripRole::Leader);
    let meta = meta_rule(TripStatus::Booked, 7);
    let audit = audit_rule(vec![edit_item(&applied_edit(), 2)]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_error(|| cancelled_transaction(&["None", CONDITIONAL_FAILURE, "None", "None"]));
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &meta, &audit, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    let error = repo
        .revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
        .await
        .expect_err("unrelated concurrent write must be reported as stale");

    assert_eq!(error, ContentHistoryRepoError::Conflict);
    assert_eq!(member.num_calls(), 2);
    assert_eq!(audit.num_calls(), 2);
}
