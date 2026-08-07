use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use aws_sdk_dynamodb::{
    operation::{
        get_item::GetItemOutput,
        query::QueryOutput,
        transact_write_items::{TransactWriteItemsError, TransactWriteItemsOutput},
    },
    types::{AttributeValue, CancellationReason, error::TransactionCanceledException},
};
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use itinera_core::{
    domain::{
        content_history::{ChangeSource, Edit, EditEntity, EditStatus},
        notice::{ChecklistMode, Notice, NoticeCategory, NoticeStatus},
        trip::{
            Booking, Candidate, CandidateStatus, Day, Money, OpeningHours, Place,
            PlaceActivityIdea, PlaceGuide, PlaceKind, Stop, StopKind, TripMember, TripRole,
            TripStatus,
        },
        user::UserId,
    },
    ports::content_history::{ContentHistoryRepo, ContentHistoryRepoError},
};
use serde_json::json;

use crate::dynamodb::trip_repo::records::*;
use crate::dynamodb::{
    CONDITIONAL_FAILURE, DynamoUserRepo, ENTITY_TYPE, PK, REVISION, SK,
    notice_repo::records::{
        NOTICE_ENTITY, NOTICE_META_SK, NOTICE_PREFIX, NoticeMetaRecord, encode_notice,
        encode_notice_meta, notice_sk,
    },
    primitives::encoded_item_bytes,
};

use super::{access, audit, reservation, revert};

const TABLE: &str = "itinera-test";
const TRIP_ID: &str = "trip-a";
const ACTOR_ID: &str = "user-a";
const CREATED_AT: &str = "2026-08-06T10:00:00Z";
const REVERTED_AT: &str = "2026-08-06T11:00:00Z";

fn actor() -> UserId {
    UserId(ACTOR_ID.into())
}

fn member(role: TripRole) -> TripMember {
    trip_member(ACTOR_ID, role)
}

fn trip_member(user_id: &str, role: TripRole) -> TripMember {
    TripMember {
        user_id: user_id.into(),
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
        source: ChangeSource::Web {},
        status: EditStatus::Applied,
        created_at: CREATED_AT.into(),
        reverted_by: None,
        reverted_at: None,
        revert_edit_id: None,
        reverts_edit_id: None,
    }
}

fn filler_edit(index: usize) -> Edit {
    Edit {
        id: format!("edit-filler-{index:04}"),
        ..applied_edit()
    }
}

fn payload_edit(index: usize, payload_len: usize) -> Edit {
    Edit {
        old_value: json!("x".repeat(payload_len)),
        new_value: json!("changed"),
        ..filler_edit(index)
    }
}

fn applied_history_items(count: usize) -> Vec<HashMap<String, AttributeValue>> {
    let mut items = vec![edit_item(&applied_edit(), 1)];
    items.extend((1..count).map(|index| edit_item(&filler_edit(index), 1)));
    items
}

fn valid_place(id: &str, name: &str) -> Place {
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

fn large_place(id: &str) -> Place {
    Place {
        name: "n".repeat(200),
        address: "a".repeat(500),
        opening_hours: Some(OpeningHours {
            weekday_text: vec!["h".repeat(200); 14],
        }),
        photo_urls: vec!["p".repeat(2_048); 20],
        guide: Some(PlaceGuide {
            summary: "s".repeat(500),
            intro: "i".repeat(4_000),
            activity_ideas: vec![
                PlaceActivityIdea {
                    title: "t".repeat(160),
                    details: Some("d".repeat(1_000)),
                };
                20
            ],
            practical_tips: vec!["p".repeat(500); 30],
        }),
        ..valid_place(id, "placeholder")
    }
}

fn large_place_edit(index: usize) -> Edit {
    Edit {
        id: format!("large-edit-{index:04}"),
        entity: EditEntity::Candidate,
        entity_id: format!("candidate-{index:04}"),
        field: "place".into(),
        old_value: serde_json::to_value(large_place(&format!("old-{index:04}")))
            .expect("place serializes"),
        new_value: serde_json::to_value(large_place(&format!("new-{index:04}")))
            .expect("place serializes"),
        ..applied_edit()
    }
}

fn notice(created_by: &str, title: &str) -> Notice {
    Notice {
        id: "notice-a".into(),
        trip_id: TRIP_ID.into(),
        created_by: created_by.into(),
        category: NoticeCategory::Safety,
        title: title.into(),
        body: "Save the emergency number.".into(),
        source_url: Some("https://example.com/advice".into()),
        pinned: false,
        status: NoticeStatus::Active,
        audience: None,
        checklist_items: vec![itinera_core::domain::notice::ChecklistItem {
            id: "item-a".into(),
            text: "Register".into(),
            done_by: vec![],
            due_date: None,
            mode: ChecklistMode::Each,
        }],
    }
}

fn notice_edit(field: &str, old_value: serde_json::Value, new_value: serde_json::Value) -> Edit {
    Edit {
        entity: EditEntity::Notice,
        entity_id: "notice-a".into(),
        field: field.into(),
        old_value,
        new_value,
        ..applied_edit()
    }
}

fn notice_rule(created_by: &str, title: &str, revision: u64) -> aws_smithy_mocks::Rule {
    notice_value_rule(notice(created_by, title), revision)
}

fn notice_value_rule(value: Notice, revision: u64) -> aws_smithy_mocks::Rule {
    let item = encode_notice(&value, CREATED_AT, revision).expect("encodes");
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(notice_sk("notice-a")))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn notice_meta_rule(
    created_by: &str,
    title: &str,
    notice_revision: u64,
    meta_revision: u64,
) -> aws_smithy_mocks::Rule {
    notice_value_meta_rule(&notice(created_by, title), notice_revision, meta_revision)
}

fn notice_value_meta_rule(
    value: &Notice,
    notice_revision: u64,
    meta_revision: u64,
) -> aws_smithy_mocks::Rule {
    let notice_item = encode_notice(value, CREATED_AT, notice_revision).expect("encodes");
    let item = encode_notice_meta(
        &NoticeMetaRecord {
            trip_id: TRIP_ID.into(),
            notice_count: 1,
            notice_bytes: u32::try_from(encoded_item_bytes(&notice_item).expect("finite item"))
                .expect("fixture fits"),
        },
        meta_revision,
    )
    .expect("meta encodes");
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(NOTICE_META_SK.into()))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn notice_collection_rule(created_by: &str, title: &str, revision: u64) -> aws_smithy_mocks::Rule {
    notice_value_collection_rule(&notice(created_by, title), revision)
}

fn notice_value_collection_rule(value: &Notice, revision: u64) -> aws_smithy_mocks::Rule {
    let item = encode_notice(value, CREATED_AT, revision).expect("encodes");
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S(NOTICE_PREFIX.into()))
        })
        .then_output(move || QueryOutput::builder().items(item.clone()).build())
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

fn membership_snapshot_meta_rule(members: &[TripMember], revision: u64) -> aws_smithy_mocks::Rule {
    let item = encode_trip_meta(
        &TripMeta {
            member_count: u32::try_from(members.len()).expect("fixture count fits"),
            leader_count: u32::try_from(
                members
                    .iter()
                    .filter(|member| member.role == TripRole::Leader)
                    .count(),
            )
            .expect("fixture count fits"),
            ..meta(TripStatus::Planning)
        },
        revision,
    )
    .expect("trip metadata encodes");
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

fn membership_snapshot_query_rule(members: &[TripMember]) -> aws_smithy_mocks::Rule {
    let items = members
        .iter()
        .map(|member| encode_member(TRIP_ID, member).expect("member encodes"))
        .collect::<Vec<_>>();
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.expression_attribute_values().is_some_and(|values| {
                    values.get(":pk") == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && values.get(":prefix") == Some(&AttributeValue::S("MEMBER#".into()))
                })
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(items.clone()))
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
async fn a_new_revert_rejects_a_server_timestamp_before_the_original_without_writing() {
    let member = membership_rule(TripRole::Member);
    let audit = audit_rule(vec![edit_item(&applied_edit(), 2)]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &audit, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.revert_edit(
            TRIP_ID,
            &actor(),
            "edit-a",
            "2026-08-06T09:00:00Z",
            "revert-a",
        )
        .await,
        Err(ContentHistoryRepoError::CorruptData)
    );
    assert_eq!(audit.num_calls(), 1);
    assert_eq!(transaction.num_calls(), 0);
}

#[tokio::test]
async fn a_new_revert_rejects_an_existing_compensation_id_at_another_timestamp() {
    let mut collision = filler_edit(1);
    collision.id = "revert-a".into();
    collision.created_at = "2026-08-06T09:00:00Z".into();
    let member = membership_rule(TripRole::Member);
    let audit = audit_rule(vec![
        edit_item(&applied_edit(), 2),
        edit_item(&collision, 1),
    ]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &audit, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
            .await,
        Err(ContentHistoryRepoError::CorruptData)
    );
    assert_eq!(audit.num_calls(), 1);
    assert_eq!(transaction.num_calls(), 0);
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
                if items.len() != 5 {
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
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("7".into()))
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":data"))
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
                    put.item().get(SK) == Some(&AttributeValue::S(audit_sk(CREATED_AT, "edit-a")))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("3".into()))
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
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
                let reservation_ok = items[4].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S("HISTORY#SLOT#0000000002".into()))
                        && put.item().get(ENTITY_TYPE)
                            == Some(&AttributeValue::S("CONTENT_HISTORY_SLOT".into()))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("1".into()))
                        && put.item().get(DATA)
                            == Some(&AttributeValue::S("{\"recordCount\":2}".into()))
                        && put.condition_expression()
                            == Some("attribute_not_exists(#pk) AND attribute_not_exists(#sk)")
                });
                membership_ok && target_ok && original_ok && compensation_ok && reservation_ok
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
async fn notice_author_revert_uses_the_notice_snapshot_and_editor_role_atomically() {
    let original = notice_edit("title", json!("Old title"), json!("New title"));
    let member = membership_rule(TripRole::Member);
    let notice = notice_rule(ACTOR_ID, "New title", 5);
    let notice_meta = notice_meta_rule(ACTOR_ID, "New title", 5, 3);
    let notices = notice_collection_rule(ACTOR_ID, "New title", 5);
    let audit = audit_rule(vec![edit_item(&original, 2)]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 6
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                })
                && items[1].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(notice_sk("notice-a")))
                        && put.item().get(ENTITY_TYPE)
                            == Some(&AttributeValue::S(NOTICE_ENTITY.into()))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("5".into()))
                        && put
                            .item()
                            .get(DATA)
                            .and_then(|value| value.as_s().ok())
                            .is_some_and(|data| data.contains("\"title\":\"Old title\""))
                })
                && items[2].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(NOTICE_META_SK.into()))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                })
                && items[3].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(audit_sk(CREATED_AT, "edit-a")))
                })
                && items[4].put().is_some_and(|put| {
                    put.item().get(SK)
                        == Some(&AttributeValue::S(audit_sk(REVERTED_AT, "revert-a")))
                })
                && items[5].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S("HISTORY#SLOT#0000000002".into()))
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &member,
            &notice,
            &notice_meta,
            &notices,
            &audit,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
        .await
        .expect("the current author may revert their notice edit");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn audience_revert_filters_excluded_stamps_and_updates_notice_metadata_atomically() {
    let original = notice_edit("audience", json!(null), json!([ACTOR_ID, "user-b"]));
    let mut current = notice(ACTOR_ID, "Current title");
    current.audience = Some(vec![ACTOR_ID.into(), "user-b".into()]);
    current.checklist_items[0].done_by = vec![ACTOR_ID.into(), "user-b".into()];
    let member = membership_rule(TripRole::Member);
    let current_members = vec![
        trip_member(ACTOR_ID, TripRole::Member),
        trip_member("leader-a", TripRole::Leader),
    ];
    let membership_meta = membership_snapshot_meta_rule(&current_members, 7);
    let membership_query = membership_snapshot_query_rule(&current_members);
    let notice = notice_value_rule(current.clone(), 5);
    let notice_meta = notice_value_meta_rule(&current, 5, 3);
    let notices = notice_value_collection_rule(&current, 5);
    let audit = audit_rule(vec![edit_item(&original, 2)]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 7
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                })
                && items[1].put().is_some_and(|put| {
                    put.item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .is_some_and(|data| {
                            !data.contains("\"audience\"")
                                && data.contains("\"doneBy\":[\"user-a\"]")
                                && !data.contains("user-b\"]")
                        })
                })
                && items[2].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(NOTICE_META_SK.into()))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                })
                && items[3].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && condition.condition_expression()
                            == "#entity = :entity AND #revision = :revision AND #data = :data"
                })
                && items[6].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S("HISTORY#SLOT#0000000002".into()))
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &member,
            &membership_meta,
            &membership_query,
            &notice,
            &notice_meta,
            &notices,
            &audit,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
        .await
        .expect("audience revert commits");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn notice_revert_requires_the_current_author_or_a_current_leader() {
    let original = notice_edit("title", json!("Old title"), json!("New title"));

    let member = membership_rule(TripRole::Member);
    let notice = notice_rule("user-b", "New title", 5);
    let audit = audit_rule(vec![edit_item(&original, 2)]);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &notice, &audit]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
            .await,
        Err(ContentHistoryRepoError::Forbidden)
    );

    let member = membership_rule(TripRole::Leader);
    let notice = notice_rule("user-b", "New title", 5);
    let notice_meta = notice_meta_rule("user-b", "New title", 5, 3);
    let notices = notice_collection_rule("user-b", "New title", 5);
    let audit = audit_rule(vec![edit_item(&original, 2)]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            request.transact_items()[0]
                .condition_check()
                .is_some_and(|condition| {
                    condition.condition_expression() == "#entity = :member AND #role = :leader"
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &member,
            &notice,
            &notice_meta,
            &notices,
            &audit,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
        .await
        .expect("a current leader may revert another author's notice edit");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn malformed_and_stale_notice_revert_values_fail_closed() {
    for (edit, current_title, expected) in [
        (
            notice_edit("title", json!(""), json!("New title")),
            "New title",
            ContentHistoryRepoError::CorruptData,
        ),
        (
            notice_edit("title", json!("Old title"), json!("New title")),
            "Later title",
            ContentHistoryRepoError::Conflict,
        ),
    ] {
        let member = membership_rule(TripRole::Member);
        let notice = notice_rule(ACTOR_ID, current_title, 5);
        let audit = audit_rule(vec![edit_item(&edit, 2)]);
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&member, &notice, &audit]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
        assert_eq!(
            repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
                .await,
            Err(expected)
        );
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
async fn impossible_provenance_cycles_and_time_travel_fail_closed() {
    let mut first = applied_edit();
    first.id = "cycle-a".into();
    first.status = EditStatus::Reverted;
    first.reverted_by = Some("author-b".into());
    first.reverted_at = Some(CREATED_AT.into());
    first.revert_edit_id = Some("cycle-b".into());
    first.reverts_edit_id = Some("cycle-b".into());
    let second = Edit {
        id: "cycle-b".into(),
        old_value: first.new_value.clone(),
        new_value: first.old_value.clone(),
        author: "author-b".into(),
        status: EditStatus::Reverted,
        reverted_by: Some(first.author.clone()),
        reverted_at: Some(CREATED_AT.into()),
        revert_edit_id: Some(first.id.clone()),
        reverts_edit_id: Some(first.id.clone()),
        ..applied_edit()
    };

    let mut time_travelling_original = reverted_edit();
    time_travelling_original.created_at = "2026-08-06T12:00:00Z".into();
    let time_travelling_compensation = compensating_edit();

    for records in [
        vec![edit_item(&first, 2), edit_item(&second, 2)],
        vec![
            edit_item(&time_travelling_original, 2),
            edit_item(&time_travelling_compensation, 1),
        ],
    ] {
        let member = membership_rule(TripRole::Viewer);
        let meta = meta_rule(TripStatus::Booked, 7);
        let audit = audit_rule(records);
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&member, &meta, &audit]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

        assert_eq!(
            repo.list_history(TRIP_ID, &actor())
                .await
                .expect_err("impossible provenance must fail closed"),
            ContentHistoryRepoError::CorruptData
        );
        assert_eq!(
            audit.num_calls(),
            2,
            "graph-only corruption gets one bounded retry"
        );
    }
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
async fn history_reads_and_responses_have_a_four_mib_byte_budget() {
    let edits = (0..40).map(large_place_edit).collect::<Vec<_>>();
    assert!(
        edits.iter().all(|edit| revert::validate_place(
            &serde_json::from_value(edit.old_value.clone()).expect("valid place")
        )
        .is_ok()),
        "the byte-budget fixture must contain semantically valid candidate snapshots"
    );
    assert_eq!(
        audit::ensure_history_response_budget(&edits)
            .expect_err("a large JSON array must be rejected before the HTTP response"),
        ContentHistoryRepoError::SafetyLimitExceeded
    );

    let member = membership_rule(TripRole::Viewer);
    let meta = meta_rule(TripStatus::Booked, 7);
    let items = edits
        .iter()
        .map(|edit| edit_item(edit, 1))
        .collect::<Vec<_>>();
    assert!(
        items
            .iter()
            .try_fold(0_usize, |total, item| {
                total.checked_add(access::encoded_item_bytes(item).ok()?)
            })
            .is_some_and(|bytes| bytes > access::MAX_HISTORY_BYTES),
        "the encoded audit rows must cross the storage-read budget"
    );
    let audit = audit_rule(items);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &meta, &audit]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.list_history(TRIP_ID, &actor())
            .await
            .expect_err("large rows must be bounded independently of row count"),
        ContentHistoryRepoError::SafetyLimitExceeded
    );
}

#[tokio::test]
async fn a_revert_rejects_projected_encoded_bytes_before_transaction() {
    let original = applied_edit();
    let original_item = edit_item(&original, 1);
    let reverted = audit::reverted_original(&original, &actor(), REVERTED_AT, "revert-a");
    let compensation = compensating_edit();
    let reverted_item = encode_record(
        trip_pk(TRIP_ID),
        audit_sk(&original.created_at, &original.id),
        AUDIT_ENTITY,
        &reverted,
        2,
    )
    .expect("reverted edit encodes");
    let compensation_item = edit_item(&compensation, 1);
    let original_bytes = access::encoded_item_bytes(&original_item).expect("original size");
    let reverted_bytes = access::encoded_item_bytes(&reverted_item).expect("reverted size");
    let compensation_bytes =
        access::encoded_item_bytes(&compensation_item).expect("compensation size");
    let projected_growth = reverted_bytes
        .checked_add(compensation_bytes)
        .and_then(|bytes| bytes.checked_sub(original_bytes))
        .expect("a compensation grows history");
    let target_loaded_bytes = access::MAX_HISTORY_BYTES
        .checked_sub(projected_growth)
        .and_then(|bytes| bytes.checked_add(1))
        .expect("projected boundary");

    // Fill with realistic sub-400-KiB DynamoDB items, then tune the final
    // string one byte at a time so the read fits but the compensation does not.
    const FIXED_PAYLOAD: usize = 250_000;
    const MAX_TUNABLE_PAYLOAD: usize = 350_000;
    let mut items = vec![original_item];
    let mut loaded_bytes = original_bytes;
    let mut index = 1;
    let max_tunable_size =
        access::encoded_item_bytes(&edit_item(&payload_edit(index, MAX_TUNABLE_PAYLOAD), 1))
            .expect("maximum tuning item size");
    while target_loaded_bytes - loaded_bytes > max_tunable_size {
        let item = edit_item(&payload_edit(index, FIXED_PAYLOAD), 1);
        loaded_bytes += access::encoded_item_bytes(&item).expect("filler size");
        items.push(item);
        index += 1;
    }

    let minimum_tunable_size = access::encoded_item_bytes(&edit_item(&payload_edit(index, 0), 1))
        .expect("minimum tuning item size");
    if target_loaded_bytes - loaded_bytes < minimum_tunable_size {
        let removed = items.pop().expect("at least one fixed filler exists");
        loaded_bytes -= access::encoded_item_bytes(&removed).expect("removed filler size");
        index -= 1;
    }
    let remaining = target_loaded_bytes - loaded_bytes;
    assert!(remaining >= minimum_tunable_size && remaining <= max_tunable_size);

    let mut low = 0;
    let mut high = MAX_TUNABLE_PAYLOAD;
    while low < high {
        let midpoint = low + (high - low).div_ceil(2);
        let size = access::encoded_item_bytes(&edit_item(&payload_edit(index, midpoint), 1))
            .expect("candidate tuning size");
        if size <= remaining {
            low = midpoint;
        } else {
            high = midpoint - 1;
        }
    }
    let tuning_item = edit_item(&payload_edit(index, low), 1);
    loaded_bytes += access::encoded_item_bytes(&tuning_item).expect("tuned filler size");
    items.push(tuning_item);

    assert!(loaded_bytes <= access::MAX_HISTORY_BYTES);
    assert!(
        loaded_bytes
            .checked_sub(original_bytes)
            .and_then(|bytes| bytes.checked_add(reverted_bytes))
            .and_then(|bytes| bytes.checked_add(compensation_bytes))
            .is_some_and(|bytes| bytes > access::MAX_HISTORY_BYTES),
        "the replacement and compensation must cross the projected-byte limit"
    );

    let member = membership_rule(TripRole::Leader);
    let audit = audit_rule(items);
    let meta = meta_rule(TripStatus::Booked, 7);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &audit, &meta]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
            .await
            .expect_err("projected bytes must fail before a transaction"),
        ContentHistoryRepoError::SafetyLimitExceeded
    );
}

#[tokio::test]
async fn a_new_revert_reserves_the_final_history_slot_but_not_a_row_beyond_it() {
    let member = membership_rule(TripRole::Leader);
    let audit = audit_rule(applied_history_items(access::MAX_HISTORY_RECORDS - 1));
    let meta = meta_rule(TripStatus::Booked, 7);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &audit, &meta, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
        .await
        .expect("row 1,000 may be the compensating edit");
    assert_eq!(transaction.num_calls(), 1);

    let member = membership_rule(TripRole::Leader);
    let audit = audit_rule(applied_history_items(access::MAX_HISTORY_RECORDS));
    let client = mock_client!(aws_sdk_dynamodb, RuleMode::MatchAny, [&member, &audit]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
            .await
            .expect_err("row 1,001 must never be appended by revert"),
        ContentHistoryRepoError::SafetyLimitExceeded
    );
}

#[tokio::test]
async fn an_already_reverted_edit_remains_idempotent_at_the_history_ceiling() {
    let member = membership_rule(TripRole::Leader);
    let mut items = completed_revert_items();
    items.extend((2..access::MAX_HISTORY_RECORDS).map(|index| edit_item(&filler_edit(index), 1)));
    let audit = audit_rule(items);
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
    .expect("the completed command remains a no-op at exactly 1,000 rows");
}

#[tokio::test]
async fn pending_and_rejected_review_material_is_not_shared_content_history() {
    let mut pending = filler_edit(1);
    pending.status = EditStatus::PendingReview;
    let mut rejected = filler_edit(2);
    rejected.status = EditStatus::Rejected;
    let applied = applied_edit();
    let records = vec![
        edit_item(&pending, 1),
        edit_item(&rejected, 1),
        edit_item(&applied, 1),
    ];

    let member = membership_rule(TripRole::Viewer);
    let meta = meta_rule(TripStatus::Booked, 7);
    let audit = audit_rule(records.clone());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &meta, &audit]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.list_history(TRIP_ID, &actor())
            .await
            .expect("visible history"),
        vec![applied]
    );

    let member = membership_rule(TripRole::Leader);
    let audit = audit_rule(records);
    let client = mock_client!(aws_sdk_dynamodb, RuleMode::MatchAny, [&member, &audit]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.revert_edit(TRIP_ID, &actor(), &pending.id, REVERTED_AT, "revert-a")
            .await
            .expect_err("review material is not addressable through content revert"),
        ContentHistoryRepoError::NotFound
    );
}

#[test]
fn revert_uses_canonical_semantic_validators_for_every_supported_value_shape() {
    assert!(revert::validate_required_text("Kyoto", 120).is_ok());
    assert!(revert::validate_required_text(" Kyoto ", 120).is_err());
    assert!(revert::validate_tags(&["food".into()]).is_ok());
    assert!(revert::validate_tags(&[" food ".into()]).is_err());
    assert!(revert::validate_local_time("09:30").is_ok());
    assert!(revert::validate_local_time("9:30").is_err());
    assert!(revert::validate_duration(1_440).is_ok());
    assert!(revert::validate_duration(0).is_err());
    assert!(revert::validate_text_len(&"n".repeat(10_000), 10_000).is_ok());
    assert!(revert::validate_text_len(&"n".repeat(10_001), 10_000).is_err());

    let malformed_url = Booking {
        reference: "ABC".into(),
        url: Some("https://".into()),
        cost: Some(Money {
            amount: 10.0,
            currency: "GBP".into(),
        }),
        ledger_entry_id: None,
    };
    assert!(revert::validate_booking(Some(&malformed_url)).is_err());
    let whitespace_ledger_id = Booking {
        url: Some("https://example.test/booking".into()),
        ledger_entry_id: Some(" ledger-a ".into()),
        ..malformed_url
    };
    assert!(revert::validate_booking(Some(&whitespace_ledger_id)).is_err());

    let mut oversized_nested_place = valid_place("place-a", "Temple");
    oversized_nested_place.guide = Some(PlaceGuide {
        summary: "Summary".into(),
        intro: "i".repeat(4_001),
        activity_ideas: vec![],
        practical_tips: vec![],
    });
    assert!(revert::validate_place(&oversized_nested_place).is_err());
    assert!(revert::validate_place(&valid_place("place-a", "Temple")).is_ok());
}

#[test]
fn booking_revert_never_replays_or_drops_the_server_owned_ledger_pointer() {
    let previous = Booking {
        reference: "OLD".into(),
        url: None,
        cost: None,
        ledger_entry_id: None,
    };
    let current = Booking {
        reference: "NEW".into(),
        url: None,
        cost: None,
        ledger_entry_id: Some("expense-current".into()),
    };
    let audited_current = Booking {
        ledger_entry_id: None,
        ..current.clone()
    };
    let (reverted, (before, after)) = revert::reverted_booking(
        &Some(current.clone()),
        &serde_json::to_value(Some(previous.clone())).expect("serializes"),
        &serde_json::to_value(Some(audited_current.clone())).expect("serializes"),
    )
    .expect("user-owned booking fields revert");
    assert_eq!(
        reverted.and_then(|booking| booking.ledger_entry_id),
        Some("expense-current".into())
    );
    assert_eq!(before["ledgerEntryId"], "expense-current");
    assert_eq!(after["ledgerEntryId"], "expense-current");

    let original = Edit {
        entity: EditEntity::Stop,
        entity_id: "stop-a".into(),
        field: "booking".into(),
        old_value: serde_json::to_value(Some(previous.clone())).expect("serializes"),
        new_value: serde_json::to_value(Some(audited_current.clone())).expect("serializes"),
        ..applied_edit()
    };
    let reverted = audit::reverted_original(&original, &actor(), REVERTED_AT, "revert-a");
    let mut compensation = audit::compensating_edit(&original, &actor(), REVERTED_AT, "revert-a");
    compensation.old_value = before;
    compensation.new_value = after;
    assert!(
        audit::revert_values_match(&reverted, &compensation),
        "provenance compares only caller-owned booking fields"
    );
    compensation.new_value["ledgerEntryId"] = json!("expense-forged");
    assert!(!audit::revert_values_match(&reverted, &compensation));

    let forged = Booking {
        ledger_entry_id: Some("expense-forged".into()),
        ..audited_current
    };
    assert_eq!(
        revert::reverted_booking(
            &Some(current),
            &serde_json::to_value(Some(previous)).expect("serializes"),
            &serde_json::to_value(Some(forged)).expect("serializes"),
        ),
        Err(ContentHistoryRepoError::CorruptData)
    );
}

#[tokio::test]
async fn unsupported_entities_and_fields_never_become_arbitrary_writes() {
    for unsupported in [
        Edit {
            entity: EditEntity::Notice,
            entity_id: "notice-a".into(),
            field: "category".into(),
            old_value: json!("safety"),
            new_value: json!("health"),
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
async fn proposal_owned_in_plan_status_is_not_a_content_revert_target() {
    let edit = Edit {
        entity: EditEntity::Candidate,
        entity_id: "candidate-a".into(),
        field: "status".into(),
        old_value: json!(CandidateStatus::Shortlisted),
        new_value: json!(CandidateStatus::InPlan),
        ..applied_edit()
    };
    let candidate = Candidate {
        id: "candidate-a".into(),
        trip_id: TRIP_ID.into(),
        source_place_id: None,
        place_id: "place-a".into(),
        proposed_by: "author-a".into(),
        created_at: CREATED_AT.into(),
        pitch: "Visit".into(),
        tags: vec![],
        status: CandidateStatus::InPlan,
    };
    let candidate_item = encode_record(
        trip_pk(TRIP_ID),
        candidate_sk(&candidate.id),
        CANDIDATE_ENTITY,
        &candidate,
        2,
    )
    .expect("candidate encodes");
    let member = membership_rule(TripRole::Leader);
    let audit = audit_rule(vec![edit_item(&edit, 1)]);
    let candidate_read = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().and_then(|key| key.get(SK))
                == Some(&AttributeValue::S("CANDIDATE#candidate-a".into()))
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(candidate_item.clone()))
                .build()
        });
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &audit, &candidate_read]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "revert-a")
            .await
            .expect_err("in-plan state belongs to structural governance"),
        ContentHistoryRepoError::Unsupported
    );
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
        "2026-08-06T09:00:00Z",
        "revert-a",
    )
    .await
    .expect("already-reverted edit ignores replacement provenance and remains a no-op");

    assert_eq!(member.num_calls(), 1);
    assert_eq!(audit.num_calls(), 1);
}

#[tokio::test]
async fn repeated_reverts_still_validate_the_typed_allowlist() {
    for (mut original, expected) in [
        (
            Edit {
                field: "arbitraryWrite".into(),
                ..reverted_edit()
            },
            ContentHistoryRepoError::Unsupported,
        ),
        (
            Edit {
                old_value: json!(TripStatus::Booked),
                new_value: json!(TripStatus::Booked),
                ..reverted_edit()
            },
            ContentHistoryRepoError::CorruptData,
        ),
    ] {
        original.entity = EditEntity::Trip;
        original.entity_id = TRIP_ID.into();
        let compensation = audit::compensating_edit(&original, &actor(), REVERTED_AT, "revert-a");
        let member = membership_rule(TripRole::Member);
        let audit = audit_rule(vec![edit_item(&original, 3), edit_item(&compensation, 1)]);
        let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
            .then_output(|| TransactWriteItemsOutput::builder().build());
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&member, &audit, &transaction]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
        assert_eq!(
            repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "retry-id")
                .await,
            Err(expected)
        );
        assert_eq!(transaction.num_calls(), 0);
    }
}

#[tokio::test]
async fn ordinary_history_appends_reserve_every_slot_and_enforce_the_shared_ceiling() {
    let first = Edit {
        id: "new-a".into(),
        ..applied_edit()
    };
    let second = Edit {
        id: "new-b".into(),
        ..applied_edit()
    };
    let new_items = vec![edit_item(&first, 1), edit_item(&second, 1)];
    let audit = audit_rule(vec![]);
    let client = mock_client!(aws_sdk_dynamodb, [&audit]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let actions = reservation::reserve_history_append(&repo, TRIP_ID, &new_items)
        .await
        .expect("two appends fit");
    assert_eq!(actions.len(), 2);
    assert_eq!(
        actions[0]
            .put()
            .and_then(|put| put.item().get(SK))
            .and_then(|value| value.as_s().ok())
            .map(String::as_str),
        Some("HISTORY#SLOT#0000000001")
    );
    assert_eq!(
        actions[1]
            .put()
            .and_then(|put| put.item().get(SK))
            .and_then(|value| value.as_s().ok())
            .map(String::as_str),
        Some("HISTORY#SLOT#0000000002")
    );

    let audit = audit_rule(applied_history_items(access::MAX_HISTORY_RECORDS));
    let client = mock_client!(aws_sdk_dynamodb, [&audit]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        reservation::reserve_history_append(&repo, TRIP_ID, &new_items[..1]).await,
        Err(ContentHistoryRepoError::SafetyLimitExceeded)
    );
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
            items.len() == 6
                && items[1].put().is_some_and(|put| {
                    let day = put
                        .item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .and_then(|data| serde_json::from_str::<Day>(data).ok());
                    put.item().get(REVISION) == Some(&AttributeValue::N("6".into()))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":data"))
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
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("11".into()))
                        && meta.is_some_and(|meta| meta.cities == ["Kyoto", "Tokyo"])
                })
                && items[3].put().is_some()
                && items[4].put().is_some()
                && items[5].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S("HISTORY#SLOT#0000000002".into()))
                })
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
    let stop_link = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.key().and_then(|key| key.get(SK))
                == Some(&AttributeValue::S("LEDGER#STOP#stop-a".into()))
        })
        .then_output(|| GetItemOutput::builder().build());
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 6
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
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("5".into()))
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":data"))
                            .and_then(|value| value.as_s().ok())
                            .is_some_and(|data| data.contains("\"notes\":\"new note\""))
                        && decoded.is_some_and(|stop| stop.notes == "old note")
                })
                && items[2].condition_check().is_some_and(|guard| {
                    guard.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && guard.condition_expression()
                            == "#entity = :entity AND #revision = :revision AND #data = :data"
                        && guard
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("11".into()))
                })
                && items[3].put().is_some()
                && items[4].put().is_some()
                && items[5].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S("HISTORY#SLOT#0000000002".into()))
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &member,
            &meta,
            &audit,
            &plan_query,
            &stop_link,
            &transaction
        ]
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
            items.len() == 7
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
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("3".into()))
                })
                && items[3].condition_check().is_some_and(|guard| {
                    guard.key().get(SK) == Some(&AttributeValue::S("PLACE#place-old".into()))
                        && guard
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("2".into()))
                })
                && items[4].put().is_some()
                && items[5].put().is_some()
                && items[6].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S("HISTORY#SLOT#0000000002".into()))
                })
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
async fn distinct_concurrent_reverts_compete_for_one_create_only_history_slot() {
    let member = membership_rule(TripRole::Leader);
    let meta = meta_rule(TripStatus::Booked, 7);
    let reads = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&reads);
    let target = edit_item(&applied_edit(), 2);
    let other = filler_edit(1);
    let other_reverted = audit::reverted_original(&other, &actor(), REVERTED_AT, "other-revert");
    let other_compensation =
        audit::compensating_edit(&other, &actor(), REVERTED_AT, "other-revert");
    let initial = vec![target.clone(), edit_item(&other, 1)];
    let after_winner = vec![
        target,
        edit_item(&other_reverted, 2),
        edit_item(&other_compensation, 1),
    ];
    let audit = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request
                .expression_attribute_values()
                .and_then(|values| values.get(":prefix"))
                == Some(&AttributeValue::S("AUDIT#".into()))
        })
        .then_output(move || {
            let items = if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                initial.clone()
            } else {
                after_winner.clone()
            };
            QueryOutput::builder().set_items(Some(items)).build()
        });
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            request.transact_items().last().is_some_and(|item| {
                item.put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S("HISTORY#SLOT#0000000003".into()))
                })
            })
        })
        .then_error(|| {
            cancelled_transaction(&["None", "None", "None", "None", CONDITIONAL_FAILURE])
        });
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&member, &meta, &audit, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.revert_edit(TRIP_ID, &actor(), "edit-a", REVERTED_AT, "loser-revert")
            .await
            .expect_err("the distinct loser must reload rather than over-append"),
        ContentHistoryRepoError::Conflict
    );
    assert_eq!(transaction.num_calls(), 1);
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
