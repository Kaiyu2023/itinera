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
    primitives::Blob,
    types::{AttributeValue, CancellationReason, error::TransactionCanceledException},
};
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use itinera_core::{
    domain::{
        notice::{ChecklistItem, ChecklistMode, Notice, NoticeCategory, NoticeStatus},
        trip::{TripMember, TripRole, TripStatus},
        user::UserId,
    },
    ports::notice::{
        ChecklistToggle, NewNotice, NoticePatch, NoticeRepo, NoticeRepoError, NoticeUpdate,
    },
    services::notices::{
        CreateNoticeInput, checklist_toggle_request_hash, notice_creation_request_hash,
    },
};

use crate::dynamodb::{
    CONDITIONAL_FAILURE, CREATE_ONLY_CONDITION, DynamoUserRepo, ENTITY_TYPE, PK, REVISION, SK,
    primitives::encoded_item_bytes,
    trip_repo::{
        audit::{AuditChange, audit},
        records::{
            AUDIT_ENTITY, DATA, META_SK, TRIP_ENTITY, TripMeta, audit_sk, encode_member,
            encode_record, encode_trip_meta, member_sk, trip_pk,
        },
    },
};

use super::records::{
    MAX_NOTICE_ITEM_BYTES, NOTICE_ENTITY, NOTICE_META_ENTITY, NOTICE_META_SK,
    NOTICE_OPERATION_META_ENTITY, NOTICE_OPERATION_META_SK, NOTICE_OPERATION_PREFIX, NOTICE_PREFIX,
    NoticeMetaRecord, NoticeOperationMetaRecord, NoticeOperationRecord, NoticeOperationResult,
    decode_notice, decode_notice_operation, encode_notice, encode_notice_meta,
    encode_notice_operation, encode_notice_operation_meta, notice_operation_key_hash,
    notice_operation_partition_key, notice_operation_sk, notice_sk,
};

const TABLE: &str = "itinera-test";
const TRIP_ID: &str = "trip-a";
const ACTOR_ID: &str = "user-a";
const OTHER_ID: &str = "user-b";
const CREATED_AT: &str = "2026-08-06T10:00:00Z";

fn actor() -> UserId {
    UserId(ACTOR_ID.into())
}

fn member(user_id: &str, role: TripRole) -> TripMember {
    TripMember {
        user_id: user_id.into(),
        role,
        joined_at: "2026-08-01T00:00:00Z".into(),
    }
}

fn notice(created_by: &str, audience: Option<Vec<String>>) -> Notice {
    Notice {
        id: "notice-a".into(),
        trip_id: TRIP_ID.into(),
        created_by: created_by.into(),
        category: NoticeCategory::Safety,
        title: "Register the trip".into(),
        body: "Save the emergency number.".into(),
        source_url: Some("https://example.com/advice".into()),
        pinned: false,
        status: NoticeStatus::Active,
        audience,
        checklist_items: vec![ChecklistItem {
            id: "item-a".into(),
            text: "Register".into(),
            done_by: vec![],
            due_date: None,
            mode: ChecklistMode::Each,
        }],
    }
}

fn new_notice(value: Notice, key: &str) -> NewNotice {
    let request_hash = notice_creation_request_hash(&CreateNoticeInput {
        category: value.category,
        title: value.title.clone(),
        body: value.body.clone(),
        source_url: value.source_url.clone(),
        checklist_items: value
            .checklist_items
            .iter()
            .map(|item| item.text.clone())
            .collect(),
        audience: value.audience.clone(),
    })
    .expect("creation request hashes");
    NewNotice {
        notice: value,
        created_at: CREATED_AT.into(),
        idempotency_key: key.into(),
        request_hash,
    }
}

fn toggle(key: &str) -> ChecklistToggle {
    ChecklistToggle {
        idempotency_key: key.into(),
        request_hash: checklist_toggle_request_hash("notice-a", "item-a")
            .expect("toggle request hashes"),
        recorded_at: CREATED_AT.into(),
    }
}

fn operation_record(
    key: &str,
    actor_id: &str,
    request_hash: String,
    result: NoticeOperationResult,
    recorded_at: &str,
) -> NoticeOperationRecord {
    let recorded = chrono::DateTime::parse_from_rfc3339(recorded_at).expect("valid fixture time");
    let expires = recorded
        .checked_add_signed(chrono::Duration::hours(24))
        .expect("fixture expiry");
    NoticeOperationRecord {
        trip_id: TRIP_ID.into(),
        key_hash: notice_operation_key_hash(key),
        actor_id: actor_id.into(),
        request_hash,
        result,
        recorded_at: recorded_at.into(),
        expires_at: expires.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
    }
}

fn exact_operation_rule(operation: &NoticeOperationRecord) -> aws_smithy_mocks::Rule {
    let item = encode_notice_operation(operation).expect("operation encodes");
    let partition_key = notice_operation_partition_key(TRIP_ID, &operation.actor_id);
    let sort_key = notice_operation_sk(&operation.key_hash);
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(partition_key.clone()))
                        && key.get(SK) == Some(&AttributeValue::S(sort_key.clone()))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn missing_exact_operation_rule(key: &str) -> aws_smithy_mocks::Rule {
    let partition_key = notice_operation_partition_key(TRIP_ID, ACTOR_ID);
    let sort_key = notice_operation_sk(&notice_operation_key_hash(key));
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|item| {
                    item.get(PK) == Some(&AttributeValue::S(partition_key.clone()))
                        && item.get(SK) == Some(&AttributeValue::S(sort_key.clone()))
                })
        })
        .then_output(|| GetItemOutput::builder().build())
}

fn missing_operation_meta_rule() -> aws_smithy_mocks::Rule {
    let partition_key = notice_operation_partition_key(TRIP_ID, ACTOR_ID);
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|item| {
                    item.get(PK) == Some(&AttributeValue::S(partition_key.clone()))
                        && item.get(SK) == Some(&AttributeValue::S(NOTICE_OPERATION_META_SK.into()))
                })
        })
        .then_output(|| GetItemOutput::builder().build())
}

fn operation_meta_rule(count: u32, revision: u64) -> aws_smithy_mocks::Rule {
    let item = encode_notice_operation_meta(
        &NoticeOperationMetaRecord {
            trip_id: TRIP_ID.into(),
            actor_id: ACTOR_ID.into(),
            operation_count: count,
        },
        revision,
    )
    .expect("operation metadata encodes");
    let partition_key = notice_operation_partition_key(TRIP_ID, ACTOR_ID);
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(partition_key.clone()))
                        && key.get(SK) == Some(&AttributeValue::S(NOTICE_OPERATION_META_SK.into()))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn operation_query_rule(operations: &[NoticeOperationRecord]) -> aws_smithy_mocks::Rule {
    let items = operations
        .iter()
        .map(|operation| encode_notice_operation(operation).expect("operation encodes"))
        .collect::<Vec<_>>();
    let partition_key = notice_operation_partition_key(TRIP_ID, ACTOR_ID);
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request.expression_attribute_values().is_some_and(|values| {
                    values.get(":pk") == Some(&AttributeValue::S(partition_key.clone()))
                        && values.get(":prefix")
                            == Some(&AttributeValue::S(NOTICE_OPERATION_PREFIX.into()))
                })
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(items.clone()))
                .build()
        })
}

fn membership_rule(user_id: &str, role: TripRole) -> aws_smithy_mocks::Rule {
    let user = UserId(user_id.into());
    let item = encode_member(TRIP_ID, &member(user_id, role)).expect("member encodes");
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(member_sk(&user)))
                })
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(item.clone()))
                .build()
        })
}

fn trip_membership_meta_rule(
    members: &[(TripMember, u64)],
    revision: u64,
) -> aws_smithy_mocks::Rule {
    let item = encode_trip_meta(
        &TripMeta {
            id: TRIP_ID.into(),
            name: "Trip".into(),
            cover_photo_url: None,
            accent_color: None,
            stop_kind_labels: None,
            status: TripStatus::Planning,
            start_date: "2026-08-06".into(),
            end_date: "2026-08-07".into(),
            base_currency: "GBP".into(),
            soft_budget: None,
            current_plan_id: None,
            current_plan_version: None,
            created_at: CREATED_AT.into(),
            member_count: u32::try_from(members.len()).expect("fixture count fits"),
            leader_count: u32::try_from(
                members
                    .iter()
                    .filter(|(member, _)| member.role == TripRole::Leader)
                    .count(),
            )
            .expect("fixture count fits"),
            cities: vec![],
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

fn trip_membership_query_rule(members: &[(TripMember, u64)]) -> aws_smithy_mocks::Rule {
    let items = members
        .iter()
        .map(|(member, _)| encode_member(TRIP_ID, member).expect("member encodes"))
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

fn meta_rule(notices: &[(Notice, u64)], revision: u64) -> aws_smithy_mocks::Rule {
    let item = meta_item(notices, revision);
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

fn meta_item(
    notices: &[(Notice, u64)],
    revision: u64,
) -> std::collections::HashMap<String, AttributeValue> {
    let notice_bytes = notices
        .iter()
        .map(|(notice, revision)| {
            encoded_item_bytes(
                &encode_notice(notice, CREATED_AT, *revision).expect("notice encodes"),
            )
            .expect("notice has a finite size")
        })
        .sum::<usize>();
    encode_notice_meta(
        &NoticeMetaRecord {
            trip_id: TRIP_ID.into(),
            notice_count: u32::try_from(notices.len()).expect("fixture count fits"),
            notice_bytes: u32::try_from(notice_bytes).expect("fixture bytes fit"),
        },
        revision,
    )
    .expect("meta encodes")
}

fn meta_transition_rule(
    before: &[(Notice, u64)],
    before_revision: u64,
    after: &[(Notice, u64)],
    after_revision: u64,
) -> aws_smithy_mocks::Rule {
    let before = meta_item(before, before_revision);
    let after = meta_item(after, after_revision);
    let calls = Arc::new(AtomicUsize::new(0));
    let reads = Arc::clone(&calls);
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(NOTICE_META_SK.into()))
                })
        })
        .then_output(move || {
            let item = if reads.fetch_add(1, Ordering::SeqCst) < 2 {
                before.clone()
            } else {
                after.clone()
            };
            GetItemOutput::builder().set_item(Some(item)).build()
        })
}

fn missing_operation_rule(key: &str) -> aws_smithy_mocks::Rule {
    let sort_key = notice_operation_sk(&notice_operation_key_hash(key));
    let partition_key = notice_operation_partition_key(TRIP_ID, ACTOR_ID);
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|item| {
                    item.get(PK) == Some(&AttributeValue::S(partition_key.clone()))
                        && item.get(SK).is_some_and(|value| {
                            value == &AttributeValue::S(sort_key.clone())
                                || value == &AttributeValue::S(NOTICE_OPERATION_META_SK.into())
                        })
                })
        })
        .then_output(|| GetItemOutput::builder().build())
}

fn empty_operation_query_rule() -> aws_smithy_mocks::Rule {
    let partition_key = notice_operation_partition_key(TRIP_ID, ACTOR_ID);
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request.expression_attribute_values().is_some_and(|values| {
                    values.get(":pk") == Some(&AttributeValue::S(partition_key.clone()))
                        && values.get(":prefix")
                            == Some(&AttributeValue::S(NOTICE_OPERATION_PREFIX.into()))
                })
        })
        .then_output(|| QueryOutput::builder().build())
}

fn empty_history_query_rule() -> aws_smithy_mocks::Rule {
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S("AUDIT#".into()))
        })
        .then_output(|| QueryOutput::builder().build())
}

fn no_meta_rule() -> aws_smithy_mocks::Rule {
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && key.get(SK) == Some(&AttributeValue::S(NOTICE_META_SK.into()))
                })
        })
        .then_output(|| GetItemOutput::builder().build())
}

fn notice_query_rule(notices: Vec<(Notice, u64)>) -> aws_smithy_mocks::Rule {
    let items = notices
        .into_iter()
        .map(|(notice, revision)| encode_notice(&notice, CREATED_AT, revision).expect("encodes"))
        .collect::<Vec<_>>();
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S(NOTICE_PREFIX.into()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(items.clone()))
                .build()
        })
}

fn cancelled_transaction() -> TransactWriteItemsError {
    TransactWriteItemsError::TransactionCanceledException(
        TransactionCanceledException::builder()
            .cancellation_reasons(
                CancellationReason::builder()
                    .code(CONDITIONAL_FAILURE)
                    .build(),
            )
            .build(),
    )
}

fn pinned_audit_item() -> std::collections::HashMap<String, AttributeValue> {
    let edit = audit(
        TRIP_ID,
        &actor(),
        CREATED_AT,
        "change-a-00",
        AuditChange {
            entity: "notice",
            entity_id: "notice-a",
            field: "pinned",
            old_value: serde_json::json!(false),
            new_value: serde_json::json!(true),
        },
    );
    encode_record(
        trip_pk(TRIP_ID),
        audit_sk(CREATED_AT, "change-a-00"),
        AUDIT_ENTITY,
        &edit,
        1,
    )
    .expect("audit encodes")
}

fn exact_get_rule(
    expected: std::collections::HashMap<String, AttributeValue>,
) -> aws_smithy_mocks::Rule {
    let key = std::collections::HashMap::from([
        (PK.to_string(), expected.get(PK).expect("pk").clone()),
        (SK.to_string(), expected.get(SK).expect("sk").clone()),
    ]);
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.consistent_read() == Some(true) && request.key() == Some(&key)
        })
        .then_output(move || {
            GetItemOutput::builder()
                .set_item(Some(expected.clone()))
                .build()
        })
}

#[test]
fn keys_and_codecs_are_strict_and_trip_partitioned() {
    assert_eq!(NOTICE_META_SK, "NOTICE#META");
    assert_eq!(notice_sk("notice-a"), "NOTICE_ITEM#notice-a");
    assert!(!notice_sk("notice-a").starts_with("AUDIT#"));
    let encoded = encode_notice(&notice(ACTOR_ID, None), CREATED_AT, 3).expect("encodes");
    let decoded = decode_notice(&encoded, TRIP_ID).expect("decodes");
    assert_eq!(decoded.notice.id, "notice-a");
    assert_eq!(decoded.revision, 3);
    assert!(matches!(
        decode_notice(&encoded, "trip-b"),
        Err(NoticeRepoError::CorruptData)
    ));

    let mut corrupt = encoded;
    corrupt.insert(
        DATA.into(),
        AttributeValue::S("{\"notice\":{},\"createdAt\":\"forged\"}".into()),
    );
    assert!(matches!(
        decode_notice(&corrupt, TRIP_ID),
        Err(NoticeRepoError::CorruptData)
    ));
}

#[test]
fn operation_claims_are_actor_partitioned_strict_and_overflow_safe() {
    let operation = operation_record(
        "create-a",
        ACTOR_ID,
        "a".repeat(64),
        NoticeOperationResult::Created {
            notice_id: "notice-a".into(),
        },
        CREATED_AT,
    );
    let encoded = encode_notice_operation(&operation).expect("operation encodes");
    assert_eq!(
        encoded.get(PK),
        Some(&AttributeValue::S(notice_operation_partition_key(
            TRIP_ID, ACTOR_ID
        )))
    );
    assert_ne!(
        notice_operation_partition_key(TRIP_ID, ACTOR_ID),
        notice_operation_partition_key(TRIP_ID, OTHER_ID)
    );
    assert_eq!(
        decode_notice_operation(&encoded, TRIP_ID, ACTOR_ID)
            .expect("operation decodes")
            .value,
        operation
    );
    assert!(matches!(
        decode_notice_operation(&encoded, TRIP_ID, OTHER_ID),
        Err(NoticeRepoError::CorruptData)
    ));

    let overflowing = NoticeOperationRecord {
        recorded_at: "9999-12-31T23:59:59Z".into(),
        expires_at: "9999-12-31T23:59:59Z".into(),
        ..operation
    };
    assert_eq!(
        encode_notice_operation(&overflowing),
        Err(NoticeRepoError::CorruptData)
    );
}

#[tokio::test]
async fn repository_creation_rejects_forged_server_owned_state_before_io() {
    let base = new_notice(notice(ACTOR_ID, None), "create-a");
    let mut forged = Vec::new();
    let mut value = base.clone();
    value.notice.trip_id = "trip-b".into();
    forged.push(value);
    let mut value = base.clone();
    value.notice.created_by = OTHER_ID.into();
    forged.push(value);
    let mut value = base.clone();
    value.notice.pinned = true;
    forged.push(value);
    let mut value = base.clone();
    value.notice.status = NoticeStatus::Archived;
    forged.push(value);
    let mut value = base.clone();
    value.notice.checklist_items[0].done_by = vec![ACTOR_ID.into()];
    forged.push(value);
    let mut value = base.clone();
    value.notice.checklist_items[0].mode = ChecklistMode::Group;
    forged.push(value);
    let mut value = base;
    value.notice.checklist_items[0].due_date = Some("2026-08-07".into());
    forged.push(value);

    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(aws_sdk_dynamodb, [&transaction]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    for value in forged {
        assert_eq!(
            repo.create_notice(TRIP_ID, &actor(), value).await,
            Err(NoticeRepoError::CorruptData)
        );
    }
    assert_eq!(transaction.num_calls(), 0);
}

#[tokio::test]
async fn exact_creation_replay_returns_the_current_trip_scoped_notice() {
    let request = new_notice(notice(ACTOR_ID, None), "create-a");
    let operation = operation_record(
        "create-a",
        ACTOR_ID,
        request.request_hash.clone(),
        NoticeOperationResult::Created {
            notice_id: "notice-a".into(),
        },
        "2026-08-06T09:00:00Z",
    );
    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let operation = exact_operation_rule(&operation);
    let mut current = notice(ACTOR_ID, None);
    current.title = "Later authorized title".into();
    let meta = meta_rule(&[(current.clone(), 3)], 5);
    let notices = notice_query_rule(vec![(current.clone(), 3)]);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &operation, &meta, &notices]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.replay_notice_creation(
            TRIP_ID,
            &actor(),
            "create-a",
            &request.request_hash,
            CREATED_AT,
        )
        .await
        .expect("replay succeeds"),
        Some(current)
    );
}

#[tokio::test]
async fn exact_checklist_toggle_replay_returns_the_current_notice_without_toggling_again() {
    let request = toggle("toggle-a");
    let operation = operation_record(
        "toggle-a",
        ACTOR_ID,
        request.request_hash.clone(),
        NoticeOperationResult::ChecklistToggled {
            notice_id: "notice-a".into(),
            item_id: "item-a".into(),
        },
        "2026-08-06T09:00:00Z",
    );
    let membership = membership_rule(ACTOR_ID, TripRole::Viewer);
    let operation = exact_operation_rule(&operation);
    let mut current = notice(OTHER_ID, Some(vec![ACTOR_ID.into(), OTHER_ID.into()]));
    current.checklist_items[0].done_by = vec![ACTOR_ID.into()];
    let meta = meta_rule(&[(current.clone(), 3)], 5);
    let notices = notice_query_rule(vec![(current.clone(), 3)]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &operation, &meta, &notices, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.replay_checklist_toggle(
            TRIP_ID,
            &actor(),
            "toggle-a",
            &request.request_hash,
            CREATED_AT,
        )
        .await
        .expect("exact replay succeeds"),
        Some(current)
    );
    assert_eq!(transaction.num_calls(), 0);
}

#[tokio::test]
async fn checklist_toggle_replay_rejects_same_key_with_a_different_request() {
    let request = toggle("toggle-a");
    let different_request_hash = checklist_toggle_request_hash("notice-a", "item-b")
        .expect("different checklist request hashes");
    let operation = operation_record(
        "toggle-a",
        ACTOR_ID,
        request.request_hash,
        NoticeOperationResult::ChecklistToggled {
            notice_id: "notice-a".into(),
            item_id: "item-a".into(),
        },
        "2026-08-06T09:00:00Z",
    );
    let membership = membership_rule(ACTOR_ID, TripRole::Viewer);
    let operation = exact_operation_rule(&operation);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &operation, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.replay_checklist_toggle(
            TRIP_ID,
            &actor(),
            "toggle-a",
            &different_request_hash,
            CREATED_AT,
        )
        .await,
        Err(NoticeRepoError::Conflict)
    );
    assert_eq!(transaction.num_calls(), 0);
}

#[tokio::test]
async fn creation_replay_rejects_a_claim_linked_to_another_author() {
    let request = new_notice(notice(ACTOR_ID, None), "create-a");
    let operation = operation_record(
        "create-a",
        ACTOR_ID,
        request.request_hash.clone(),
        NoticeOperationResult::Created {
            notice_id: "notice-a".into(),
        },
        "2026-08-06T09:00:00Z",
    );
    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let operation = exact_operation_rule(&operation);
    let forged = notice(OTHER_ID, None);
    let meta = meta_rule(&[(forged.clone(), 3)], 5);
    let notices = notice_query_rule(vec![(forged, 3)]);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &operation, &meta, &notices]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.replay_notice_creation(
            TRIP_ID,
            &actor(),
            "create-a",
            &request.request_hash,
            CREATED_AT,
        )
        .await,
        Err(NoticeRepoError::CorruptData)
    );
}

#[tokio::test]
async fn an_expired_same_key_claim_is_snapshot_replaced_atomically() {
    let request = new_notice(notice(ACTOR_ID, None), "create-a");
    let expired = operation_record(
        "create-a",
        ACTOR_ID,
        "b".repeat(64),
        NoticeOperationResult::Created {
            notice_id: "old-notice".into(),
        },
        "2026-08-05T10:00:00Z",
    );
    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let exact_operation = exact_operation_rule(&expired);
    let operation_meta = operation_meta_rule(1, 2);
    let operations = operation_query_rule(std::slice::from_ref(&expired));
    let meta = no_meta_rule();
    let notices = notice_query_rule(vec![]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 5
                && items[3].put().is_some_and(|put| {
                    put.item().get(ENTITY_TYPE)
                        == Some(&AttributeValue::S(NOTICE_OPERATION_META_ENTITY.into()))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("3".into()))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                })
                && items[4].put().is_some_and(|put| {
                    put.item()
                        .get(SK)
                        .and_then(|value| value.as_s().ok())
                        .is_some_and(|value| value.starts_with(NOTICE_OPERATION_PREFIX))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &exact_operation,
            &operation_meta,
            &operations,
            &meta,
            &notices,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.create_notice(TRIP_ID, &actor(), request.clone())
            .await
            .expect("expired claim can be replaced"),
        request.notice
    );
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn active_retry_claims_are_bounded_only_in_the_callers_partition() {
    let operations = (0..32)
        .map(|index| {
            operation_record(
                &format!("old-key-{index}"),
                ACTOR_ID,
                "a".repeat(64),
                NoticeOperationResult::Created {
                    notice_id: format!("old-notice-{index}"),
                },
                "2026-08-06T09:00:00Z",
            )
        })
        .collect::<Vec<_>>();
    let request = new_notice(notice(ACTOR_ID, None), "new-key");
    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let exact_operation = missing_exact_operation_rule("new-key");
    let operation_meta = operation_meta_rule(32, 7);
    let operation_items = operation_query_rule(&operations);
    let meta = no_meta_rule();
    let notices = notice_query_rule(vec![]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &exact_operation,
            &operation_meta,
            &operation_items,
            &meta,
            &notices,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.create_notice(TRIP_ID, &actor(), request).await,
        Err(NoticeRepoError::SafetyLimitExceeded)
    );
    assert_eq!(transaction.num_calls(), 0);
    assert_ne!(
        notice_operation_partition_key(TRIP_ID, ACTOR_ID),
        notice_operation_partition_key(TRIP_ID, OTHER_ID),
        "another actor has an independent claim budget"
    );
}

#[test]
fn notice_items_have_a_defensive_limit_below_dynamodb_maximum() {
    let audience = (0..90)
        .map(|index| format!("user-{index:03}-{}", "x".repeat(187)))
        .collect::<Vec<_>>();
    let mut value = notice(ACTOR_ID, Some(audience.clone()));
    value.checklist_items = (0..20)
        .map(|index| ChecklistItem {
            id: format!("item-{index}"),
            text: "x".repeat(500),
            done_by: audience.clone(),
            due_date: None,
            mode: ChecklistMode::Each,
        })
        .collect();
    assert_eq!(
        encode_notice(&value, CREATED_AT, 1),
        Err(NoticeRepoError::SafetyLimitExceeded)
    );
    value.checklist_items.truncate(1);
    let encoded = encode_notice(&value, CREATED_AT, 1).expect("smaller item fits");
    assert!(encoded_item_bytes(&encoded).expect("finite") <= MAX_NOTICE_ITEM_BYTES);
}

#[tokio::test]
async fn projected_notice_aggregate_over_four_mib_is_rejected_before_write() {
    fn creation_shaped_notice(index: usize, created_by: &str) -> Notice {
        let mut value = notice(created_by, None);
        value.id = format!("notice-{index}");
        value.checklist_items = (0..100)
            .map(|item_index| ChecklistItem {
                id: format!("item-{index}-{item_index}"),
                text: "x".repeat(500),
                done_by: vec![],
                due_date: None,
                mode: ChecklistMode::Each,
            })
            .collect();
        value
    }

    let new_value = creation_shaped_notice(999, ACTOR_ID);
    let new_bytes =
        encoded_item_bytes(&encode_notice(&new_value, CREATED_AT, 1).expect("new notice encodes"))
            .expect("finite item");
    let mut stored = Vec::new();
    let mut total = 0_usize;
    for index in 0..999 {
        let value = creation_shaped_notice(index, OTHER_ID);
        let bytes = encoded_item_bytes(
            &encode_notice(&value, CREATED_AT, 1).expect("stored notice encodes"),
        )
        .expect("finite item");
        if total + bytes > itinera_core::services::notices::MAX_NOTICE_RESPONSE_BYTES {
            break;
        }
        total += bytes;
        stored.push((value, 1));
    }
    assert!(
        total + new_bytes > itinera_core::services::notices::MAX_NOTICE_RESPONSE_BYTES,
        "fixture must cross the aggregate ceiling"
    );

    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let meta = meta_rule(&stored, 4);
    let notices = notice_query_rule(stored);
    let operation = missing_operation_rule("aggregate-overflow");
    let operation_query = empty_operation_query_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &meta,
            &notices,
            &operation,
            &operation_query,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.create_notice(
            TRIP_ID,
            &actor(),
            new_notice(new_value, "aggregate-overflow"),
        )
        .await,
        Err(NoticeRepoError::SafetyLimitExceeded)
    );
    assert_eq!(transaction.num_calls(), 0);
}

#[tokio::test]
async fn every_direct_member_role_can_list_but_viewers_cannot_create() {
    for role in [TripRole::Viewer, TripRole::Member, TripRole::Leader] {
        let membership = membership_rule(ACTOR_ID, role);
        let meta = meta_rule(&[(notice(ACTOR_ID, None), 2)], 4);
        let notices = notice_query_rule(vec![(notice(ACTOR_ID, None), 2)]);
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&membership, &meta, &notices]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
        assert_eq!(repo.list_notices(TRIP_ID, &actor()).await.unwrap().len(), 1);
    }

    let membership = membership_rule(ACTOR_ID, TripRole::Viewer);
    let client = mock_client!(aws_sdk_dynamodb, [&membership]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.create_notice(
            TRIP_ID,
            &actor(),
            new_notice(notice(ACTOR_ID, None), "viewer-create"),
        )
        .await,
        Err(NoticeRepoError::Forbidden)
    );
}

#[tokio::test]
async fn create_rechecks_editor_and_audience_memberships_in_one_transaction() {
    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let members = vec![
        (member(ACTOR_ID, TripRole::Member), 1),
        (member(OTHER_ID, TripRole::Viewer), 1),
        (member("leader", TripRole::Leader), 1),
    ];
    let trip_meta = trip_membership_meta_rule(&members, 7);
    let trip_members = trip_membership_query_rule(&members);
    let meta = no_meta_rule();
    let notices = notice_query_rule(vec![]);
    let operation = missing_operation_rule("create-a");
    let operation_query = empty_operation_query_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 6
                && items[0].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(member_sk(&actor())))
                        && condition.condition_expression()
                            == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                })
                && items[1].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && condition.condition_expression()
                            == "#entity = :entity AND #revision = :revision AND #data = :data"
                        && condition
                            .expression_attribute_values()
                            .is_some_and(|values| {
                                values.get(":entity")
                                    == Some(&AttributeValue::S(TRIP_ENTITY.into()))
                            })
                })
                && items[2].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(NOTICE_META_SK.into()))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
                && items[3].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(notice_sk("notice-a")))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("1".into()))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
                && items[4].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(NOTICE_OPERATION_META_SK.into()))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
                && items[5].put().is_some_and(|put| {
                    put.item()
                        .get(SK)
                        .and_then(|value| value.as_s().ok())
                        .is_some_and(|value| value.starts_with(NOTICE_OPERATION_PREFIX))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip_meta,
            &trip_members,
            &meta,
            &notices,
            &operation,
            &operation_query,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let created = repo
        .create_notice(
            TRIP_ID,
            &actor(),
            new_notice(
                notice(ACTOR_ID, Some(vec![ACTOR_ID.into(), OTHER_ID.into()])),
                "create-a",
            ),
        )
        .await
        .expect("notice commits");
    assert_eq!(created.id, "notice-a");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn foreign_notice_ids_are_not_resolved_outside_the_route_trip() {
    let membership = membership_rule(ACTOR_ID, TripRole::Leader);
    let meta = no_meta_rule();
    let notices = notice_query_rule(vec![]);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &notices]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.update_notice(
            TRIP_ID,
            &actor(),
            "notice-from-trip-b",
            NoticeUpdate {
                patch: NoticePatch {
                    pinned: Some(true),
                    ..NoticePatch::default()
                },
                changed_at: CREATED_AT.into(),
                change_id: "change-a".into(),
            },
        )
        .await,
        Err(NoticeRepoError::NotFound)
    );
}

#[tokio::test]
async fn author_update_has_exact_role_meta_and_notice_snapshot_guards() {
    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let meta = meta_rule(&[(notice(ACTOR_ID, None), 2)], 4);
    let notices = notice_query_rule(vec![(notice(ACTOR_ID, None), 2)]);
    let history = empty_history_query_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 5
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                })
                && items[1].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(NOTICE_META_SK.into()))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                })
                && items[2].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(notice_sk("notice-a")))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("3".into()))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":entity"))
                            == Some(&AttributeValue::S(NOTICE_ENTITY.into()))
                })
                && items[3].put().is_some_and(|put| {
                    put.item()
                        .get(SK)
                        .and_then(|value| value.as_s().ok())
                        .is_some_and(|value| value.starts_with("HISTORY#SLOT#"))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
                && items[4].put().is_some_and(|put| {
                    put.item() == &pinned_audit_item()
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &notices, &history, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let updated = repo
        .update_notice(
            TRIP_ID,
            &actor(),
            "notice-a",
            NoticeUpdate {
                patch: NoticePatch {
                    pinned: Some(true),
                    ..NoticePatch::default()
                },
                changed_at: CREATED_AT.into(),
                change_id: "change-a".into(),
            },
        )
        .await
        .expect("author update commits");
    assert!(updated.pinned);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn non_author_members_and_demoted_authors_cannot_manage_notices() {
    for (role, created_by) in [(TripRole::Member, OTHER_ID), (TripRole::Viewer, ACTOR_ID)] {
        let membership = membership_rule(ACTOR_ID, role);
        let meta = meta_rule(&[(notice(created_by, None), 2)], 4);
        let notices = notice_query_rule(vec![(notice(created_by, None), 2)]);
        let client = mock_client!(
            aws_sdk_dynamodb,
            RuleMode::MatchAny,
            [&membership, &meta, &notices]
        );
        let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
        assert_eq!(
            repo.update_notice(
                TRIP_ID,
                &actor(),
                "notice-a",
                NoticeUpdate {
                    patch: NoticePatch {
                        pinned: Some(true),
                        ..NoticePatch::default()
                    },
                    changed_at: CREATED_AT.into(),
                    change_id: "change-a".into(),
                },
            )
            .await,
            Err(NoticeRepoError::Forbidden)
        );
    }
}

#[tokio::test]
async fn leaders_manage_foreign_authored_notices_with_a_leader_only_transaction_check() {
    let membership = membership_rule(ACTOR_ID, TripRole::Leader);
    let meta = meta_rule(&[(notice(OTHER_ID, None), 2)], 4);
    let notices = notice_query_rule(vec![(notice(OTHER_ID, None), 2)]);
    let history = empty_history_query_rule();
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
        [&membership, &meta, &notices, &history, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert!(
        repo.update_notice(
            TRIP_ID,
            &actor(),
            "notice-a",
            NoticeUpdate {
                patch: NoticePatch {
                    status: Some(NoticeStatus::Archived),
                    ..NoticePatch::default()
                },
                changed_at: CREATED_AT.into(),
                change_id: "change-a".into(),
            },
        )
        .await
        .expect("leader update")
        .status
            == NoticeStatus::Archived
    );
}

#[tokio::test]
async fn audience_replacement_rejects_nonmembers_before_any_transaction() {
    let membership = membership_rule(ACTOR_ID, TripRole::Leader);
    let members = vec![(member(ACTOR_ID, TripRole::Leader), 1)];
    let trip_meta = trip_membership_meta_rule(&members, 7);
    let trip_members = trip_membership_query_rule(&members);
    let meta = meta_rule(&[(notice(ACTOR_ID, None), 2)], 4);
    let notices = notice_query_rule(vec![(notice(ACTOR_ID, None), 2)]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip_meta,
            &trip_members,
            &meta,
            &notices,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.update_notice(
            TRIP_ID,
            &actor(),
            "notice-a",
            NoticeUpdate {
                patch: NoticePatch {
                    audience: Some(Some(vec!["foreign-user".into()])),
                    ..NoticePatch::default()
                },
                changed_at: CREATED_AT.into(),
                change_id: "change-a".into(),
            },
        )
        .await,
        Err(NoticeRepoError::Conflict)
    );
    assert_eq!(transaction.num_calls(), 0);
}

#[tokio::test]
async fn audience_shrink_server_derives_completion_cleanup_atomically() {
    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let members = vec![
        (member(ACTOR_ID, TripRole::Member), 1),
        (member("leader", TripRole::Leader), 1),
    ];
    let trip_meta = trip_membership_meta_rule(&members, 7);
    let trip_members = trip_membership_query_rule(&members);
    let mut stored = notice(ACTOR_ID, Some(vec![ACTOR_ID.into(), OTHER_ID.into()]));
    stored.checklist_items[0].done_by = vec![ACTOR_ID.into(), OTHER_ID.into()];
    let meta = meta_rule(&[(stored.clone(), 2)], 4);
    let notices = notice_query_rule(vec![(stored, 2)]);
    let history = empty_history_query_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 6
                && items[2].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                        && condition.condition_expression()
                            == "#entity = :entity AND #revision = :revision AND #data = :data"
                })
                && items[3].put().is_some_and(|put| {
                    put.item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .is_some_and(|data| {
                            data.contains("\"audience\":[\"user-a\"]")
                                && data.contains("\"doneBy\":[\"user-a\"]")
                                && !data.contains("\"doneBy\":[\"user-a\",\"user-b\"]")
                        })
                })
                && items[4].put().is_some_and(|put| {
                    put.item()
                        .get(SK)
                        .and_then(|value| value.as_s().ok())
                        .is_some_and(|value| value.starts_with("HISTORY#SLOT#"))
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip_meta,
            &trip_members,
            &meta,
            &notices,
            &history,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let updated = repo
        .update_notice(
            TRIP_ID,
            &actor(),
            "notice-a",
            NoticeUpdate {
                patch: NoticePatch {
                    audience: Some(Some(vec![ACTOR_ID.into()])),
                    ..NoticePatch::default()
                },
                changed_at: CREATED_AT.into(),
                change_id: "audience-change".into(),
            },
        )
        .await
        .expect("audience update commits");
    assert_eq!(updated.audience, Some(vec![ACTOR_ID.into()]));
    assert_eq!(updated.checklist_items[0].done_by, vec![ACTOR_ID]);
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn whole_group_audience_request_persists_departed_stamp_cleanup() {
    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let members = vec![
        (member(ACTOR_ID, TripRole::Member), 1),
        (member("leader", TripRole::Leader), 1),
    ];
    let trip_meta = trip_membership_meta_rule(&members, 7);
    let trip_members = trip_membership_query_rule(&members);
    let mut stored = notice(ACTOR_ID, None);
    stored.checklist_items[0].mode = ChecklistMode::Group;
    stored.checklist_items[0].done_by = vec![OTHER_ID.into()];
    let meta = meta_rule(&[(stored.clone(), 2)], 4);
    let notices = notice_query_rule(vec![(stored, 2)]);
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 4
                && items[2].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S(META_SK.into()))
                })
                && items[3].put().is_some_and(|put| {
                    put.item()
                        .get(DATA)
                        .and_then(|value| value.as_s().ok())
                        .is_some_and(|data| data.contains("\"doneBy\":[]"))
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip_meta,
            &trip_members,
            &meta,
            &notices,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let updated = repo
        .update_notice(
            TRIP_ID,
            &actor(),
            "notice-a",
            NoticeUpdate {
                patch: NoticePatch {
                    audience: Some(None),
                    ..NoticePatch::default()
                },
                changed_at: CREATED_AT.into(),
                change_id: "cleanup-a".into(),
            },
        )
        .await
        .expect("derived cleanup commits even without an audience value change");
    assert!(updated.checklist_items[0].done_by.is_empty());
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn maximal_audience_and_six_field_patch_stays_below_the_transaction_limit() {
    let membership = membership_rule(ACTOR_ID, TripRole::Leader);
    let members = (0..90)
        .map(|index| {
            let user_id = if index == 0 {
                ACTOR_ID.to_string()
            } else {
                format!("user-{index}")
            };
            (member(&user_id, TripRole::Leader), 1)
        })
        .collect::<Vec<_>>();
    let trip_meta = trip_membership_meta_rule(&members, 7);
    let trip_members = trip_membership_query_rule(&members);
    let stored = notice(ACTOR_ID, None);
    let meta = meta_rule(&[(stored.clone(), 2)], 4);
    let notices = notice_query_rule(vec![(stored, 2)]);
    let history = empty_history_query_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| request.transact_items().len() == 16)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip_meta,
            &trip_members,
            &meta,
            &notices,
            &history,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let updated = repo
        .update_notice(
            TRIP_ID,
            &actor(),
            "notice-a",
            NoticeUpdate {
                patch: NoticePatch {
                    title: Some("Updated title".into()),
                    body: Some("Updated body".into()),
                    pinned: Some(true),
                    source_url: Some(None),
                    status: Some(NoticeStatus::Resolved),
                    audience: Some(Some(
                        members
                            .iter()
                            .map(|(member, _)| member.user_id.clone())
                            .collect(),
                    )),
                },
                changed_at: CREATED_AT.into(),
                change_id: "max-change".into(),
            },
        )
        .await
        .expect("every schema-valid patch field fits with the maximum audience");
    assert_eq!(updated.audience.as_ref().map(Vec::len), Some(90));
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn query_byte_budget_counts_malformed_top_level_attributes() {
    let mut item = encode_notice(&notice(ACTOR_ID, None), CREATED_AT, 1).expect("notice encodes");
    item.insert(
        "unexpectedBlob".into(),
        AttributeValue::B(Blob::new(vec![0; super::access::MAX_NOTICE_BYTES + 1])),
    );
    let query = mock!(aws_sdk_dynamodb::Client::query).then_output(move || {
        QueryOutput::builder()
            .set_items(Some(vec![item.clone()]))
            .build()
    });
    let client = mock_client!(aws_sdk_dynamodb, [&query]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.notice_query(&trip_pk(TRIP_ID), NOTICE_PREFIX, 1).await,
        Err(NoticeRepoError::SafetyLimitExceeded)
    );
}

#[tokio::test]
async fn viewer_checklist_toggle_is_self_owned_audience_scoped_and_atomic() {
    let stored = notice(OTHER_ID, Some(vec![ACTOR_ID.into(), OTHER_ID.into()]));
    let expected_meta_data = meta_item(&[(stored.clone(), 2)], 4)
        .remove(DATA)
        .expect("metadata contains its exact encoded snapshot");
    let expected_notice_data = encode_notice(&stored, CREATED_AT, 2)
        .expect("notice encodes")
        .remove(DATA)
        .expect("notice contains its exact encoded snapshot");
    let membership = membership_rule(ACTOR_ID, TripRole::Viewer);
    let meta = meta_rule(&[(stored.clone(), 2)], 4);
    let notices = notice_query_rule(vec![(stored, 2)]);
    let operation = missing_operation_rule("toggle-a");
    let operation_query = empty_operation_query_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(move |request| {
            let items = request.transact_items();
            items.len() == 5
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression() == "#entity = :member"
                })
                && items[1].put().is_some_and(|put| {
                    put.item().get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && put.item().get(SK) == Some(&AttributeValue::S(NOTICE_META_SK.into()))
                        && put.item().get(ENTITY_TYPE)
                            == Some(&AttributeValue::S(NOTICE_META_ENTITY.into()))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("5".into()))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                        && put.expression_attribute_names().is_some_and(|names| {
                            names.len() == 3
                                && names.get("#entity") == Some(&ENTITY_TYPE.to_string())
                                && names.get("#revision") == Some(&REVISION.to_string())
                                && names.get("#data") == Some(&DATA.to_string())
                        })
                        && put.expression_attribute_values().is_some_and(|values| {
                            values.len() == 3
                                && values.get(":entity")
                                    == Some(&AttributeValue::S(NOTICE_META_ENTITY.into()))
                                && values.get(":revision") == Some(&AttributeValue::N("4".into()))
                                && values.get(":data") == Some(&expected_meta_data)
                        })
                })
                && items[2].put().is_some_and(|put| {
                    put.item().get(PK) == Some(&AttributeValue::S(trip_pk(TRIP_ID)))
                        && put.item().get(SK) == Some(&AttributeValue::S(notice_sk("notice-a")))
                        && put.item().get(ENTITY_TYPE)
                            == Some(&AttributeValue::S(NOTICE_ENTITY.into()))
                        && put.item().get(REVISION) == Some(&AttributeValue::N("3".into()))
                        && put
                            .item()
                            .get(DATA)
                            .and_then(|value| value.as_s().ok())
                            .is_some_and(|data| data.contains("\"doneBy\":[\"user-a\"]"))
                        && put.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision AND #data = :data")
                        && put.expression_attribute_names().is_some_and(|names| {
                            names.len() == 3
                                && names.get("#entity") == Some(&ENTITY_TYPE.to_string())
                                && names.get("#revision") == Some(&REVISION.to_string())
                                && names.get("#data") == Some(&DATA.to_string())
                        })
                        && put.expression_attribute_values().is_some_and(|values| {
                            values.len() == 3
                                && values.get(":entity")
                                    == Some(&AttributeValue::S(NOTICE_ENTITY.into()))
                                && values.get(":revision") == Some(&AttributeValue::N("2".into()))
                                && values.get(":data") == Some(&expected_notice_data)
                        })
                })
                && items[3].put().is_some_and(|put| {
                    put.item().get(SK) == Some(&AttributeValue::S(NOTICE_OPERATION_META_SK.into()))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
                && items[4].put().is_some_and(|put| {
                    put.item()
                        .get(SK)
                        .and_then(|value| value.as_s().ok())
                        .is_some_and(|value| value.starts_with(NOTICE_OPERATION_PREFIX))
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
        })
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &meta,
            &notices,
            &operation,
            &operation_query,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let toggled = repo
        .toggle_checklist_item(TRIP_ID, &actor(), "notice-a", "item-a", toggle("toggle-a"))
        .await
        .expect("viewer may acknowledge their own task");
    assert_eq!(toggled.checklist_items[0].done_by, vec![ACTOR_ID]);
    assert_eq!(transaction.num_calls(), 1);

    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let meta = meta_rule(&[(notice(OTHER_ID, Some(vec![OTHER_ID.into()])), 2)], 4);
    let notices = notice_query_rule(vec![(notice(OTHER_ID, Some(vec![OTHER_ID.into()])), 2)]);
    let operation = missing_operation_rule("toggle-forbidden");
    let operation_query = empty_operation_query_rule();
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &notices, &operation, &operation_query]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.toggle_checklist_item(
            TRIP_ID,
            &actor(),
            "notice-a",
            "item-a",
            toggle("toggle-forbidden"),
        )
        .await,
        Err(NoticeRepoError::Forbidden)
    );
}

#[tokio::test]
async fn checklist_toggle_recovers_an_ambiguous_commit_from_the_exact_claim_and_snapshot() {
    let request = toggle("toggle-ambiguous");
    let committed_operation = operation_record(
        "toggle-ambiguous",
        ACTOR_ID,
        request.request_hash.clone(),
        NoticeOperationResult::ChecklistToggled {
            notice_id: "notice-a".into(),
            item_id: "item-a".into(),
        },
        CREATED_AT,
    );
    let operation_item =
        encode_notice_operation(&committed_operation).expect("committed operation encodes");
    let operation_partition = notice_operation_partition_key(TRIP_ID, ACTOR_ID);
    let operation_sort = notice_operation_sk(&committed_operation.key_hash);
    let operation_reads = Arc::new(AtomicUsize::new(0));
    let output_reads = Arc::clone(&operation_reads);
    let operation = mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(operation_partition.clone()))
                        && key.get(SK) == Some(&AttributeValue::S(operation_sort.clone()))
                })
        })
        .then_output(move || {
            let item =
                (output_reads.fetch_add(1, Ordering::SeqCst) > 0).then(|| operation_item.clone());
            GetItemOutput::builder().set_item(item).build()
        });

    let initial = notice(OTHER_ID, Some(vec![ACTOR_ID.into(), OTHER_ID.into()]));
    let mut committed = initial.clone();
    committed.checklist_items[0].done_by = vec![ACTOR_ID.into()];
    let meta = meta_transition_rule(&[(initial.clone(), 2)], 4, &[(committed.clone(), 3)], 5);
    let initial_item = encode_notice(&initial, CREATED_AT, 2).expect("initial notice encodes");
    let committed_item =
        encode_notice(&committed, CREATED_AT, 3).expect("committed notice encodes");
    let notice_reads = Arc::new(AtomicUsize::new(0));
    let output_reads = Arc::clone(&notice_reads);
    let notices = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request.consistent_read() == Some(true)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S(NOTICE_PREFIX.into()))
        })
        .then_output(move || {
            let item = if output_reads.fetch_add(1, Ordering::SeqCst) == 0 {
                initial_item.clone()
            } else {
                committed_item.clone()
            };
            QueryOutput::builder().items(item).build()
        });
    let membership = membership_rule(ACTOR_ID, TripRole::Viewer);
    let operation_meta = missing_operation_meta_rule();
    let operation_query = empty_operation_query_rule();
    let transaction =
        mock!(aws_sdk_dynamodb::Client::transact_write_items).then_error(cancelled_transaction);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &operation,
            &meta,
            &notices,
            &operation_meta,
            &operation_query,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.toggle_checklist_item(TRIP_ID, &actor(), "notice-a", "item-a", request,)
            .await
            .expect("the committed toggle is recovered after the ambiguous response"),
        committed
    );
    assert_eq!(transaction.num_calls(), 1);
    assert_eq!(membership.num_calls(), 2);
    assert_eq!(operation_reads.load(Ordering::SeqCst), 2);
    assert_eq!(notice_reads.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_group_completion_can_only_be_cleared_by_the_member_who_stamped_it() {
    let membership = membership_rule(ACTOR_ID, TripRole::Viewer);
    let mut stored = notice(OTHER_ID, None);
    stored.checklist_items[0].mode = ChecklistMode::Group;
    stored.checklist_items[0].done_by = vec![OTHER_ID.into()];
    let meta = meta_rule(&[(stored.clone(), 2)], 4);
    let notices = notice_query_rule(vec![(stored, 2)]);
    let operation = missing_operation_rule("toggle-group");
    let operation_query = empty_operation_query_rule();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .then_output(|| TransactWriteItemsOutput::builder().build());
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &meta,
            &notices,
            &operation,
            &operation_query,
            &transaction,
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.toggle_checklist_item(
            TRIP_ID,
            &actor(),
            "notice-a",
            "item-a",
            toggle("toggle-group"),
        )
        .await,
        Err(NoticeRepoError::Conflict)
    );
    assert_eq!(transaction.num_calls(), 0);
}

#[tokio::test]
async fn corrupt_or_count_mismatched_notice_aggregates_fail_closed_after_one_retry() {
    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let meta = meta_rule(&[(notice(ACTOR_ID, None), 2)], 4);
    let notices = notice_query_rule(vec![]);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &notices]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.list_notices(TRIP_ID, &actor()).await,
        Err(NoticeRepoError::CorruptData)
    );
    assert_eq!(notices.num_calls(), 2);
    assert_eq!(meta.num_calls(), 4);
}

#[tokio::test]
async fn stale_updates_conflict_but_an_ambiguous_committed_snapshot_is_recovered() {
    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let meta = meta_rule(&[(notice(ACTOR_ID, None), 2)], 4);
    let notices = notice_query_rule(vec![(notice(ACTOR_ID, None), 2)]);
    let history = empty_history_query_rule();
    let transaction =
        mock!(aws_sdk_dynamodb::Client::transact_write_items).then_error(cancelled_transaction);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &notices, &history, &transaction]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    assert_eq!(
        repo.update_notice(
            TRIP_ID,
            &actor(),
            "notice-a",
            NoticeUpdate {
                patch: NoticePatch {
                    pinned: Some(true),
                    ..NoticePatch::default()
                },
                changed_at: CREATED_AT.into(),
                change_id: "change-a".into(),
            },
        )
        .await,
        Err(NoticeRepoError::Conflict)
    );

    let membership = membership_rule(ACTOR_ID, TripRole::Member);
    let initial_notice = notice(ACTOR_ID, None);
    let old = encode_notice(&initial_notice, CREATED_AT, 2).expect("old encodes");
    let mut committed_notice = notice(ACTOR_ID, None);
    committed_notice.pinned = true;
    let committed = encode_notice(&committed_notice, CREATED_AT, 3).expect("new encodes");
    let meta = meta_transition_rule(
        &[(initial_notice, 2)],
        4,
        &[(committed_notice.clone(), 3)],
        5,
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let reads = Arc::clone(&calls);
    let notices = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| {
            request
                .expression_attribute_values()
                .and_then(|values| values.get(":prefix"))
                == Some(&AttributeValue::S(NOTICE_PREFIX.into()))
        })
        .then_output(move || {
            let item = if reads.fetch_add(1, Ordering::SeqCst) == 0 {
                old.clone()
            } else {
                committed.clone()
            };
            QueryOutput::builder().items(item).build()
        });
    let transaction =
        mock!(aws_sdk_dynamodb::Client::transact_write_items).then_error(cancelled_transaction);
    let audit = exact_get_rule(pinned_audit_item());
    let history = empty_history_query_rule();
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [&membership, &meta, &notices, &history, &transaction, &audit]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let recovered = repo
        .update_notice(
            TRIP_ID,
            &actor(),
            "notice-a",
            NoticeUpdate {
                patch: NoticePatch {
                    pinned: Some(true),
                    ..NoticePatch::default()
                },
                changed_at: CREATED_AT.into(),
                change_id: "change-a".into(),
            },
        )
        .await
        .expect("exact committed result is recovered");
    assert!(recovered.pinned);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
