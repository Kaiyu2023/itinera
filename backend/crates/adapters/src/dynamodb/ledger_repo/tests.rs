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
    types::{
        AttributeValue, CancellationReason, TransactWriteItem,
        error::{InternalServerError, TransactionCanceledException},
    },
};
use aws_smithy_mocks::{RuleMode, mock, mock_client};
use itinera_core::{
    domain::{
        ledger::{Expense, ExpenseCategory, ExpenseSplit, Settlement},
        trip::{Booking, Day, Plan, Stop, StopKind, TripMember, TripRole, TripStatus},
        user::UserId,
    },
    ports::ledger::{
        ExpenseReplacement, LedgerRepo, LedgerRepoError, LedgerTripContext, NewExpense,
        NewSettlement,
    },
    services::ledger::{
        AddExpenseInput, AddSettlementInput, expense_creation_request_hash,
        settlement_creation_request_hash,
    },
};

use crate::dynamodb::{
    CONDITIONAL_FAILURE, CREATE_ONLY_CONDITION, DynamoUserRepo, ENTITY_TYPE, PK, REVISION, SK,
    trip_repo::records::{
        DATA, DAY_ENTITY, META_SK, PLAN_ENTITY, STOP_ENTITY, TripMeta, day_sk, encode_member,
        encode_record, encode_trip_meta, plan_prefix, plan_sk, stop_sk, trip_pk,
    },
};

use super::records::{
    EXPENSE_ENTITY, EXPENSE_PREFIX, LEDGER_AUDIT_ENTITY, LEDGER_AUDIT_PREFIX, LEDGER_META_ENTITY,
    LEDGER_META_SK, LEDGER_OPERATION_ENTITY, LEDGER_OPERATION_PREFIX, LedgerAuditAction,
    LedgerAuditRecord, LedgerAuditValue, LedgerMetaRecord, LedgerOperationRecord,
    SETTLEMENT_ENTITY, SETTLEMENT_PREFIX, STOP_LINK_ENTITY, STOP_LINK_PREFIX, StopLinkRecord,
    decode_expense, encode_expense, encode_ledger_audit, encode_ledger_meta,
    encode_ledger_operation, encode_stop_link, operation_key_hash,
};
use super::{
    access::{LedgerReadBudget, MAX_LEDGER_BYTES},
    operations::validate_audit_graph,
};

const TABLE: &str = "itinera-test";
const TRIP_ID: &str = "trip-a";
const ACTOR_ID: &str = "member-a";
const CREATED_AT: &str = "2026-08-06T10:00:00Z";
const LATER_AT: &str = "2026-08-06T10:01:00Z";

fn actor() -> UserId {
    UserId(ACTOR_ID.into())
}

fn member(role: TripRole) -> TripMember {
    member_with_id(ACTOR_ID, role)
}

fn member_with_id(user_id: &str, role: TripRole) -> TripMember {
    TripMember {
        user_id: user_id.into(),
        role,
        joined_at: "2026-08-01T00:00:00Z".into(),
    }
}

fn trip_meta(trip_id: &str, with_plan: bool) -> TripMeta {
    TripMeta {
        id: trip_id.into(),
        name: "Japan".into(),
        cover_photo_url: None,
        accent_color: None,
        stop_kind_labels: None,
        status: TripStatus::Planning,
        start_date: "2026-11-01".into(),
        end_date: "2026-11-03".into(),
        base_currency: "GBP".into(),
        soft_budget: None,
        current_plan_id: with_plan.then(|| "plan-a".into()),
        current_plan_version: with_plan.then_some(1),
        created_at: "2026-08-01T00:00:00Z".into(),
        member_count: 1,
        leader_count: 1,
        cities: vec!["Kyoto".into()],
    }
}

fn expense(id: &str, linked_stop_id: Option<&str>) -> Expense {
    Expense {
        id: id.into(),
        trip_id: TRIP_ID.into(),
        paid_by: ACTOR_ID.into(),
        amount: 100.0,
        currency: "GBP".into(),
        fx_rate_to_base: 1.0,
        category: ExpenseCategory::Food,
        split: ExpenseSplit::Even {
            participant_ids: vec![ACTOR_ID.into()],
        },
        note: "Dinner".into(),
        receipt_photo_url: None,
        linked_stop_id: linked_stop_id.map(str::to_string),
        created_at: CREATED_AT.into(),
    }
}

fn new_expense(value: Expense, trip_revision: u64) -> NewExpense {
    let request_hash = expense_request_hash(&value);
    NewExpense {
        expense: value,
        context: LedgerTripContext {
            base_currency: "GBP".into(),
            trip_revision,
        },
        idempotency_key: "operation-a".into(),
        request_hash,
        audit_id: "audit-a".into(),
        audit_at: CREATED_AT.into(),
    }
}

fn expense_request_hash(value: &Expense) -> String {
    expense_creation_request_hash(&AddExpenseInput {
        paid_by: value.paid_by.clone(),
        amount: value.amount,
        currency: value.currency.clone(),
        category: value.category,
        split: value.split.clone(),
        note: value.note.clone(),
        linked_stop_id: value.linked_stop_id.clone(),
    })
    .expect("fixture request hashes")
}

fn expense_created_audit(value: &Expense) -> LedgerAuditRecord {
    LedgerAuditRecord {
        id: "audit-a".into(),
        trip_id: TRIP_ID.into(),
        actor_id: ACTOR_ID.into(),
        action: LedgerAuditAction::ExpenseCreated,
        entity_id: value.id.clone(),
        before: None,
        after: Some(LedgerAuditValue::Expense(value.clone())),
        previous_audit_id: None,
        created_at: CREATED_AT.into(),
    }
}

fn expense_operation(value: &Expense) -> LedgerOperationRecord {
    LedgerOperationRecord {
        trip_id: TRIP_ID.into(),
        key_hash: operation_key_hash("operation-a"),
        actor_id: ACTOR_ID.into(),
        request_hash: expense_request_hash(value),
        result: LedgerAuditValue::Expense(value.clone()),
        created_at: CREATED_AT.into(),
    }
}

fn settlement() -> Settlement {
    Settlement {
        id: "settlement-a".into(),
        trip_id: TRIP_ID.into(),
        from_user: ACTOR_ID.into(),
        to_user: "member-b".into(),
        amount: 20.0,
        settled_at: CREATED_AT.into(),
    }
}

fn new_settlement(value: Settlement) -> NewSettlement {
    let request_hash = settlement_request_hash(&value);
    NewSettlement {
        settlement: value,
        idempotency_key: "settlement-operation-a".into(),
        request_hash,
        audit_id: "audit-s".into(),
        audit_at: CREATED_AT.into(),
    }
}

fn settlement_request_hash(value: &Settlement) -> String {
    settlement_creation_request_hash(&AddSettlementInput {
        from_user: value.from_user.clone(),
        to_user: value.to_user.clone(),
        amount: value.amount,
    })
    .expect("fixture request hashes")
}

fn settlement_created_audit(value: &Settlement) -> LedgerAuditRecord {
    LedgerAuditRecord {
        id: "audit-s".into(),
        trip_id: TRIP_ID.into(),
        actor_id: ACTOR_ID.into(),
        action: LedgerAuditAction::SettlementCreated,
        entity_id: value.id.clone(),
        before: None,
        after: Some(LedgerAuditValue::Settlement(value.clone())),
        previous_audit_id: None,
        created_at: CREATED_AT.into(),
    }
}

fn settlement_operation(value: &Settlement) -> LedgerOperationRecord {
    LedgerOperationRecord {
        trip_id: TRIP_ID.into(),
        key_hash: operation_key_hash("settlement-operation-a"),
        actor_id: ACTOR_ID.into(),
        request_hash: settlement_request_hash(value),
        result: LedgerAuditValue::Settlement(value.clone()),
        created_at: CREATED_AT.into(),
    }
}

fn loaded<T>(value: T, revision: u64) -> super::records::Loaded<T> {
    super::records::Loaded {
        value,
        revision,
        raw_data: String::new(),
    }
}

fn ledger_meta(expense_count: u32, link_count: u32, audit_count: u64) -> LedgerMetaRecord {
    LedgerMetaRecord {
        trip_id: TRIP_ID.into(),
        expense_count,
        settlement_count: 0,
        stop_link_count: link_count,
        audit_count,
        operation_count: u32::try_from(audit_count).expect("fixture count fits"),
        audit_head_id: (audit_count > 0).then(|| "audit-a".into()),
        audit_head_at: (audit_count > 0).then(|| CREATED_AT.into()),
    }
}

fn member_rule(trip_id: &str, role: TripRole) -> aws_smithy_mocks::Rule {
    let trip_id = trip_id.to_string();
    let item = encode_member(&trip_id, &member(role)).expect("member encodes");
    get_rule(&trip_id, format!("MEMBER#{ACTOR_ID}"), Some(item))
}

fn trip_meta_rule(trip_id: &str, revision: u64, with_plan: bool) -> aws_smithy_mocks::Rule {
    let item = encode_trip_meta(&trip_meta(trip_id, with_plan), revision).expect("trip encodes");
    get_rule(trip_id, META_SK.into(), Some(item))
}

fn ledger_meta_rule(value: Option<LedgerMetaRecord>, revision: u64) -> aws_smithy_mocks::Rule {
    let item = value.map(|meta| encode_ledger_meta(&meta, revision).expect("meta encodes"));
    get_rule(TRIP_ID, LEDGER_META_SK.into(), item)
}

fn get_rule(
    trip_id: &str,
    sort_key: String,
    item: Option<HashMap<String, AttributeValue>>,
) -> aws_smithy_mocks::Rule {
    let trip_id = trip_id.to_string();
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.table_name() == Some(TABLE)
                && request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(&trip_id)))
                        && key.get(SK) == Some(&AttributeValue::S(sort_key.clone()))
                })
        })
        .then_output(move || GetItemOutput::builder().set_item(item.clone()).build())
}

fn query_rule(
    trip_id: &str,
    prefix: impl Into<String>,
    items: Vec<HashMap<String, AttributeValue>>,
) -> aws_smithy_mocks::Rule {
    let trip_id = trip_id.to_string();
    let prefix = prefix.into();
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request.table_name() == Some(TABLE)
                && request.consistent_read() == Some(true)
                && request.limit() == Some(100)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":pk"))
                    == Some(&AttributeValue::S(trip_pk(&trip_id)))
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S(prefix.clone()))
        })
        .then_output(move || {
            QueryOutput::builder()
                .set_items(Some(items.clone()))
                .build()
        })
}

fn get_sequence_rule(
    trip_id: &str,
    sort_key: String,
    items: Vec<Option<HashMap<String, AttributeValue>>>,
) -> aws_smithy_mocks::Rule {
    let trip_id = trip_id.to_string();
    let calls = Arc::new(AtomicUsize::new(0));
    mock!(aws_sdk_dynamodb::Client::get_item)
        .match_requests(move |request| {
            request.table_name() == Some(TABLE)
                && request.consistent_read() == Some(true)
                && request.key().is_some_and(|key| {
                    key.get(PK) == Some(&AttributeValue::S(trip_pk(&trip_id)))
                        && key.get(SK) == Some(&AttributeValue::S(sort_key.clone()))
                })
        })
        .then_output(move || {
            let index = calls.fetch_add(1, Ordering::SeqCst).min(items.len() - 1);
            GetItemOutput::builder()
                .set_item(items[index].clone())
                .build()
        })
}

fn query_sequence_rule(
    trip_id: &str,
    prefix: impl Into<String>,
    pages: Vec<Vec<HashMap<String, AttributeValue>>>,
) -> aws_smithy_mocks::Rule {
    let trip_id = trip_id.to_string();
    let prefix = prefix.into();
    let calls = Arc::new(AtomicUsize::new(0));
    mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| {
            request.table_name() == Some(TABLE)
                && request.consistent_read() == Some(true)
                && request.limit() == Some(100)
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":pk"))
                    == Some(&AttributeValue::S(trip_pk(&trip_id)))
                && request
                    .expression_attribute_values()
                    .and_then(|values| values.get(":prefix"))
                    == Some(&AttributeValue::S(prefix.clone()))
        })
        .then_output(move || {
            let index = calls.fetch_add(1, Ordering::SeqCst).min(pages.len() - 1);
            QueryOutput::builder()
                .set_items(Some(pages[index].clone()))
                .build()
        })
}

fn empty_state_rules() -> [aws_smithy_mocks::Rule; 6] {
    [
        ledger_meta_rule(None, 1),
        query_rule(TRIP_ID, EXPENSE_PREFIX, vec![]),
        query_rule(TRIP_ID, SETTLEMENT_PREFIX, vec![]),
        query_rule(TRIP_ID, STOP_LINK_PREFIX, vec![]),
        query_rule(TRIP_ID, LEDGER_AUDIT_PREFIX, vec![]),
        query_rule(TRIP_ID, LEDGER_OPERATION_PREFIX, vec![]),
    ]
}

fn transaction_succeeded() -> TransactWriteItemsOutput {
    TransactWriteItemsOutput::builder().build()
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

fn unavailable_transaction() -> TransactWriteItemsError {
    TransactWriteItemsError::InternalServerError(InternalServerError::builder().build())
}

fn plan_rows(booking_link: Option<Option<&str>>) -> Vec<HashMap<String, AttributeValue>> {
    let plan = Plan {
        id: "plan-a".into(),
        trip_id: TRIP_ID.into(),
        version: 1,
        created_from_proposal_id: None,
        created_at: "2026-08-01T00:00:00Z".into(),
    };
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
        place_id: "place-a".into(),
        seq: 1.0,
        stop_kind: StopKind::Meal,
        planned_arrival: "19:00".into(),
        duration_min: 60,
        booking: booking_link.map(|ledger_entry_id| Booking {
            reference: "BOOKING-A".into(),
            url: None,
            cost: None,
            ledger_entry_id: ledger_entry_id.map(str::to_string),
        }),
        notes: String::new(),
    };
    vec![
        encode_record(trip_pk(TRIP_ID), plan_sk(1), PLAN_ENTITY, &plan, 4).expect("plan encodes"),
        encode_record(trip_pk(TRIP_ID), day_sk(1, &day), DAY_ENTITY, &day, 7).expect("day encodes"),
        encode_record(
            trip_pk(TRIP_ID),
            stop_sk(1, &day, &stop),
            STOP_ENTITY,
            &stop,
            9,
        )
        .expect("stop encodes"),
    ]
}

fn item_entity(action: &TransactWriteItem) -> Option<&str> {
    action
        .put()
        .and_then(|put| put.item().get(ENTITY_TYPE))
        .and_then(|value| value.as_s().ok())
        .map(String::as_str)
}

#[test]
fn strict_record_codecs_reject_cross_trip_unknown_fields_and_invalid_audits() {
    let value = expense("expense-a", None);
    let encoded = encode_expense(&value, 2).expect("expense encodes");
    assert_eq!(
        decode_expense(&encoded, TRIP_ID)
            .expect("expense decodes")
            .value,
        value
    );
    assert_eq!(
        decode_expense(&encoded, "trip-b").unwrap_err(),
        LedgerRepoError::CorruptData
    );

    let mut forged = encoded;
    let mut data: serde_json::Value = serde_json::from_str(
        forged
            .get(DATA)
            .and_then(|value| value.as_s().ok())
            .expect("data"),
    )
    .expect("valid json");
    data["forged"] = serde_json::json!(true);
    forged.insert(DATA.into(), AttributeValue::S(data.to_string()));
    assert_eq!(
        decode_expense(&forged, TRIP_ID).unwrap_err(),
        LedgerRepoError::CorruptData
    );

    let invalid_audit = LedgerAuditRecord {
        id: "audit-a".into(),
        trip_id: TRIP_ID.into(),
        actor_id: ACTOR_ID.into(),
        action: LedgerAuditAction::ExpenseUpdated,
        entity_id: value.id.clone(),
        before: Some(LedgerAuditValue::Expense(value.clone())),
        after: Some(LedgerAuditValue::Expense(value)),
        previous_audit_id: None,
        created_at: LATER_AT.into(),
    };
    assert_eq!(
        encode_ledger_audit(&invalid_audit).unwrap_err(),
        LedgerRepoError::CorruptData
    );
}

#[tokio::test]
async fn ledger_queries_follow_pages_and_enforce_the_shared_byte_budget() {
    let cursor = HashMap::from([
        (PK.to_string(), AttributeValue::S(trip_pk(TRIP_ID))),
        (
            SK.to_string(),
            AttributeValue::S("EXPENSE#expense-a".into()),
        ),
    ]);
    let first_cursor = cursor.clone();
    let first = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(|request| request.exclusive_start_key().is_none())
        .then_output(move || {
            QueryOutput::builder()
                .items(HashMap::from([(
                    DATA.to_string(),
                    AttributeValue::S("{}".into()),
                )]))
                .set_last_evaluated_key(Some(first_cursor.clone()))
                .build()
        });
    let second_cursor = cursor;
    let second = mock!(aws_sdk_dynamodb::Client::query)
        .match_requests(move |request| request.exclusive_start_key() == Some(&second_cursor))
        .then_output(|| {
            QueryOutput::builder()
                .items(HashMap::from([(
                    DATA.to_string(),
                    AttributeValue::S("{}".into()),
                )]))
                .build()
        });
    let client = mock_client!(aws_sdk_dynamodb, [&first, &second]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let mut budget = LedgerReadBudget::default();
    assert_eq!(
        repo.ledger_query(&trip_pk(TRIP_ID), EXPENSE_PREFIX, 2, &mut budget)
            .await
            .expect("all pages")
            .len(),
        2
    );

    let oversized = mock!(aws_sdk_dynamodb::Client::query).then_output(|| {
        QueryOutput::builder()
            .items(HashMap::from([(
                DATA.to_string(),
                AttributeValue::S("x".repeat(MAX_LEDGER_BYTES + 1)),
            )]))
            .build()
    });
    let client = mock_client!(aws_sdk_dynamodb, [&oversized]);
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");
    let mut budget = LedgerReadBudget::default();
    assert_eq!(
        repo.ledger_query(&trip_pk(TRIP_ID), EXPENSE_PREFIX, 1, &mut budget)
            .await,
        Err(LedgerRepoError::SafetyLimitExceeded)
    );
}

#[tokio::test]
async fn viewers_read_the_bounded_ledger_but_cannot_write() {
    let membership = member_rule(TRIP_ID, TripRole::Viewer);
    let trip = trip_meta_rule(TRIP_ID, 3, false);
    let [
        ledger_meta,
        expenses,
        settlements,
        links,
        audits,
        operations,
    ] = empty_state_rules();
    let members = query_rule(
        TRIP_ID,
        "MEMBER#",
        vec![encode_member(TRIP_ID, &member(TripRole::Viewer)).expect("member encodes")],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &ledger_meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations,
            &members
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("valid table");

    let data = repo
        .get_ledger_data(TRIP_ID, &actor())
        .await
        .expect("viewers may read");
    assert!(data.expenses.is_empty());
    assert_eq!(data.current_member_ids, vec![ACTOR_ID.to_string()]);
    assert_eq!(
        repo.add_expense(
            TRIP_ID,
            &actor(),
            new_expense(expense("expense-a", None), 3)
        )
        .await
        .unwrap_err(),
        LedgerRepoError::Forbidden
    );
}

#[tokio::test]
async fn foreign_expense_ids_are_resolved_only_inside_the_route_trip() {
    const OTHER_TRIP: &str = "trip-b";
    let membership = member_rule(OTHER_TRIP, TripRole::Member);
    let trip = trip_meta_rule(OTHER_TRIP, 2, false);
    let meta = get_rule(OTHER_TRIP, LEDGER_META_SK.into(), None);
    let expenses = query_rule(OTHER_TRIP, EXPENSE_PREFIX, vec![]);
    let settlements = query_rule(OTHER_TRIP, SETTLEMENT_PREFIX, vec![]);
    let links = query_rule(OTHER_TRIP, STOP_LINK_PREFIX, vec![]);
    let audits = query_rule(OTHER_TRIP, LEDGER_AUDIT_PREFIX, vec![]);
    let operations = query_rule(OTHER_TRIP, LEDGER_OPERATION_PREFIX, vec![]);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("valid table");

    assert_eq!(
        repo.get_expense(OTHER_TRIP, &actor(), "expense-a")
            .await
            .unwrap_err(),
        LedgerRepoError::NotFound
    );
    assert_eq!(
        expenses.num_calls(),
        1,
        "only the route partition is queried"
    );
}

#[tokio::test]
async fn expense_creation_transaction_has_exact_role_snapshot_and_audit_guards() {
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let trip = trip_meta_rule(TRIP_ID, 3, false);
    let [
        ledger_meta,
        expenses,
        settlements,
        links,
        audits,
        operations,
    ] = empty_state_rules();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 6
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                        && condition.key().get(SK)
                            == Some(&AttributeValue::S(format!("MEMBER#{ACTOR_ID}")))
                })
                && items[1].condition_check().is_some_and(|condition| {
                    condition
                        .condition_expression()
                        .contains("#revision = :revision")
                        && condition
                            .condition_expression()
                            .contains("attribute_not_exists(#plan_id)")
                        && condition
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("3".into()))
                })
                && items[2].put().is_some_and(|put| {
                    put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                        && item_entity(&items[2]) == Some(LEDGER_META_ENTITY)
                })
                && items[3].put().is_some_and(|put| {
                    put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                        && item_entity(&items[3]) == Some(EXPENSE_ENTITY)
                })
                && items[4].put().is_some_and(|put| {
                    put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                        && item_entity(&items[4]) == Some(LEDGER_AUDIT_ENTITY)
                })
                && items[5].put().is_some_and(|put| {
                    put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                        && item_entity(&items[5]) == Some(LEDGER_OPERATION_ENTITY)
                })
        })
        .then_output(transaction_succeeded);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &ledger_meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("valid table");

    let created = repo
        .add_expense(
            TRIP_ID,
            &actor(),
            new_expense(expense("expense-a", None), 3),
        )
        .await
        .expect("expense commits");
    assert_eq!(created.id, "expense-a");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn linked_expense_transaction_conditions_the_current_plan_and_updates_the_stop() {
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let trip = trip_meta_rule(TRIP_ID, 3, true);
    let [
        ledger_meta,
        expenses,
        settlements,
        links,
        audits,
        operations,
    ] = empty_state_rules();
    let plan = query_rule(
        TRIP_ID,
        format!("{}#", plan_prefix(1)),
        plan_rows(Some(None)),
    );
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 9
                && items[1].condition_check().is_some_and(|condition| {
                    condition
                        .condition_expression()
                        .contains("#plan_id = :plan_id")
                        && condition
                            .condition_expression()
                            .contains("#plan_version = :plan_version")
                })
                && items[2].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :entity AND #revision = :revision"
                        && condition.key().get(SK) == Some(&AttributeValue::S(plan_sk(1)))
                })
                && items[3].put().is_some_and(|put| {
                    put.condition_expression() == Some("#revision = :revision")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("9".into()))
                        && put
                            .item()
                            .get(DATA)
                            .and_then(|value| value.as_s().ok())
                            .is_some_and(|data| data.contains("\"ledgerEntryId\":\"expense-a\""))
                })
                && item_entity(&items[4]) == Some(STOP_LINK_ENTITY)
                && items[4].put().and_then(|put| put.item().get(SK))
                    == Some(&AttributeValue::S("LEDGER#STOP#stop-a".into()))
                && item_entity(&items[5]) == Some(LEDGER_META_ENTITY)
                && item_entity(&items[6]) == Some(EXPENSE_ENTITY)
                && item_entity(&items[7]) == Some(LEDGER_AUDIT_ENTITY)
                && item_entity(&items[8]) == Some(LEDGER_OPERATION_ENTITY)
        })
        .then_output(transaction_succeeded);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &ledger_meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations,
            &plan,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("valid table");

    repo.add_expense(
        TRIP_ID,
        &actor(),
        new_expense(expense("expense-a", Some("stop-a")), 3),
    )
    .await
    .expect("linked expense commits");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn an_expense_cannot_create_an_orphan_claim_for_a_stop_without_booking_details() {
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let trip = trip_meta_rule(TRIP_ID, 3, true);
    let [meta, expenses, settlements, links, audits, operations] = empty_state_rules();
    let plan = query_rule(TRIP_ID, format!("{}#", plan_prefix(1)), plan_rows(None));
    let transaction =
        mock!(aws_sdk_dynamodb::Client::transact_write_items).then_output(transaction_succeeded);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations,
            &plan,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.add_expense(
            TRIP_ID,
            &actor(),
            new_expense(expense("expense-a", Some("stop-a")), 3),
        )
        .await,
        Err(LedgerRepoError::Conflict)
    );
    assert_eq!(transaction.num_calls(), 0);
}

#[tokio::test]
async fn expense_update_transaction_snapshot_guards_meta_revision_and_audit_append() {
    let current = expense("expense-a", None);
    let mut replacement = current.clone();
    replacement.note = "Corrected".into();
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let trip = trip_meta_rule(TRIP_ID, 3, false);
    let meta = ledger_meta_rule(Some(ledger_meta(1, 0, 1)), 5);
    let expenses = query_rule(
        TRIP_ID,
        EXPENSE_PREFIX,
        vec![encode_expense(&current, 1).expect("expense encodes")],
    );
    let settlements = query_rule(TRIP_ID, SETTLEMENT_PREFIX, vec![]);
    let links = query_rule(TRIP_ID, STOP_LINK_PREFIX, vec![]);
    let audits = query_rule(
        TRIP_ID,
        LEDGER_AUDIT_PREFIX,
        vec![encode_ledger_audit(&expense_created_audit(&current)).expect("audit encodes")],
    );
    let operations = query_rule(
        TRIP_ID,
        LEDGER_OPERATION_PREFIX,
        vec![encode_ledger_operation(&expense_operation(&current)).expect("operation encodes")],
    );
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 5
                && items[0].condition_check().is_some_and(|condition| {
                    condition.condition_expression()
                        == "#entity = :member AND (#role = :leader OR #role = :member_role)"
                })
                && items[1].condition_check().is_some_and(|condition| {
                    condition
                        .condition_expression()
                        .contains("#revision = :revision")
                })
                && items[2].put().is_some_and(|put| {
                    item_entity(&items[2]) == Some(LEDGER_META_ENTITY)
                        && put.condition_expression()
                            == Some("#revision = :revision AND #data = :data")
                        && put
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("5".into()))
                })
                && items[3].put().is_some_and(|put| {
                    item_entity(&items[3]) == Some(EXPENSE_ENTITY)
                        && put.condition_expression() == Some("#revision = :revision")
                        && put.item().get(REVISION) == Some(&AttributeValue::N("2".into()))
                })
                && items[4].put().is_some_and(|put| {
                    item_entity(&items[4]) == Some(LEDGER_AUDIT_ENTITY)
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
        })
        .then_output(transaction_succeeded);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.replace_expense(
            TRIP_ID,
            &actor(),
            ExpenseReplacement {
                expense: replacement.clone(),
                expected_revision: 1,
                context: LedgerTripContext {
                    base_currency: "GBP".into(),
                    trip_revision: 3,
                },
                audit_id: "audit-b".into(),
                audit_at: LATER_AT.into(),
            },
        )
        .await
        .expect("update commits"),
        replacement
    );
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn expense_delete_transaction_guards_the_active_row_and_preserves_audit_history() {
    let current = expense("expense-a", None);
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let trip = trip_meta_rule(TRIP_ID, 3, false);
    let meta = ledger_meta_rule(Some(ledger_meta(1, 0, 1)), 5);
    let expenses = query_rule(
        TRIP_ID,
        EXPENSE_PREFIX,
        vec![encode_expense(&current, 1).expect("expense encodes")],
    );
    let settlements = query_rule(TRIP_ID, SETTLEMENT_PREFIX, vec![]);
    let links = query_rule(TRIP_ID, STOP_LINK_PREFIX, vec![]);
    let audits = query_rule(
        TRIP_ID,
        LEDGER_AUDIT_PREFIX,
        vec![encode_ledger_audit(&expense_created_audit(&current)).expect("audit encodes")],
    );
    let operations = query_rule(
        TRIP_ID,
        LEDGER_OPERATION_PREFIX,
        vec![encode_ledger_operation(&expense_operation(&current)).expect("operation encodes")],
    );
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 5
                && items[2].put().is_some_and(|put| {
                    item_entity(&items[2]) == Some(LEDGER_META_ENTITY)
                        && put.condition_expression()
                            == Some("#revision = :revision AND #data = :data")
                })
                && items[3].delete().is_some_and(|delete| {
                    delete.key().get(SK) == Some(&AttributeValue::S("EXPENSE#expense-a".into()))
                        && delete.condition_expression()
                            == Some("#entity = :entity AND #revision = :revision")
                        && delete
                            .expression_attribute_values()
                            .and_then(|values| values.get(":revision"))
                            == Some(&AttributeValue::N("1".into()))
                })
                && items[4].put().is_some_and(|put| {
                    item_entity(&items[4]) == Some(LEDGER_AUDIT_ENTITY)
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
        })
        .then_output(transaction_succeeded);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    repo.delete_expense(TRIP_ID, &actor(), "expense-a", "audit-b", LATER_AT)
        .await
        .expect("delete commits");
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn settlement_transaction_rechecks_both_members_and_claims_operation_and_audit() {
    let value = settlement();
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let other_member_item =
        encode_member(TRIP_ID, &member_with_id("member-b", TripRole::Member)).expect("member");
    let other_member = get_rule(TRIP_ID, "MEMBER#member-b".into(), Some(other_member_item));
    let trip = trip_meta_rule(TRIP_ID, 3, false);
    let [meta, expenses, settlements, links, audits, operations] = empty_state_rules();
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items)
        .match_requests(|request| {
            let items = request.transact_items();
            items.len() == 7
                && items[0].condition_check().is_some_and(|condition| {
                    condition.key().get(SK)
                        == Some(&AttributeValue::S(format!("MEMBER#{ACTOR_ID}")))
                        && condition.condition_expression().contains("#role = :leader")
                })
                && items[1].condition_check().is_some_and(|condition| {
                    condition.key().get(SK) == Some(&AttributeValue::S("MEMBER#member-b".into()))
                        && condition.condition_expression() == "#entity = :member"
                })
                && items[3].put().is_some_and(|put| {
                    item_entity(&items[3]) == Some(LEDGER_META_ENTITY)
                        && put.condition_expression() == Some(CREATE_ONLY_CONDITION)
                })
                && item_entity(&items[4]) == Some(SETTLEMENT_ENTITY)
                && item_entity(&items[5]) == Some(LEDGER_AUDIT_ENTITY)
                && item_entity(&items[6]) == Some(LEDGER_OPERATION_ENTITY)
                && items[4..].iter().all(|item| {
                    item.put().and_then(|put| put.condition_expression())
                        == Some(CREATE_ONLY_CONDITION)
                })
        })
        .then_output(transaction_succeeded);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &other_member,
            &trip,
            &meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.add_settlement(TRIP_ID, &actor(), new_settlement(value.clone()))
            .await
            .expect("settlement commits"),
        value
    );
    assert_eq!(transaction.num_calls(), 1);
}

#[tokio::test]
async fn stale_expense_revisions_conflict_before_a_transaction() {
    let current = expense("expense-a", None);
    let mut original = current.clone();
    original.note = "Original".into();
    let created = expense_created_audit(&original);
    let updated = LedgerAuditRecord {
        id: "audit-b".into(),
        trip_id: TRIP_ID.into(),
        actor_id: ACTOR_ID.into(),
        action: LedgerAuditAction::ExpenseUpdated,
        entity_id: current.id.clone(),
        before: Some(LedgerAuditValue::Expense(original.clone())),
        after: Some(LedgerAuditValue::Expense(current.clone())),
        previous_audit_id: Some("audit-a".into()),
        created_at: LATER_AT.into(),
    };
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let trip = trip_meta_rule(TRIP_ID, 3, false);
    let meta = ledger_meta_rule(
        Some(LedgerMetaRecord {
            trip_id: TRIP_ID.into(),
            expense_count: 1,
            settlement_count: 0,
            stop_link_count: 0,
            audit_count: 2,
            operation_count: 1,
            audit_head_id: Some("audit-b".into()),
            audit_head_at: Some(LATER_AT.into()),
        }),
        5,
    );
    let expenses = query_rule(
        TRIP_ID,
        EXPENSE_PREFIX,
        vec![encode_expense(&current, 2).expect("expense encodes")],
    );
    let settlements = query_rule(TRIP_ID, SETTLEMENT_PREFIX, vec![]);
    let links = query_rule(TRIP_ID, STOP_LINK_PREFIX, vec![]);
    let audits = query_rule(
        TRIP_ID,
        LEDGER_AUDIT_PREFIX,
        vec![
            encode_ledger_audit(&created).expect("audit encodes"),
            encode_ledger_audit(&updated).expect("audit encodes"),
        ],
    );
    let operations = query_rule(
        TRIP_ID,
        LEDGER_OPERATION_PREFIX,
        vec![encode_ledger_operation(&expense_operation(&original)).expect("operation encodes")],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("valid table");
    let mut corrected = current;
    corrected.note = "Corrected".into();

    assert_eq!(
        repo.replace_expense(
            TRIP_ID,
            &actor(),
            ExpenseReplacement {
                expense: corrected,
                expected_revision: 1,
                context: LedgerTripContext {
                    base_currency: "GBP".into(),
                    trip_revision: 3,
                },
                audit_id: "audit-b".into(),
                audit_at: LATER_AT.into(),
            },
        )
        .await
        .unwrap_err(),
        LedgerRepoError::Conflict
    );
}

#[tokio::test]
async fn concurrent_transaction_cancellation_is_not_misreported_as_a_committed_update() {
    let current = expense("expense-a", None);
    let mut replacement = current.clone();
    replacement.note = "Corrected".into();
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let trip = trip_meta_rule(TRIP_ID, 3, false);
    let meta = ledger_meta_rule(Some(ledger_meta(1, 0, 1)), 5);
    let expenses = query_rule(
        TRIP_ID,
        EXPENSE_PREFIX,
        vec![encode_expense(&current, 1).expect("expense encodes")],
    );
    let settlements = query_rule(TRIP_ID, SETTLEMENT_PREFIX, vec![]);
    let links = query_rule(TRIP_ID, STOP_LINK_PREFIX, vec![]);
    let audits = query_rule(
        TRIP_ID,
        LEDGER_AUDIT_PREFIX,
        vec![encode_ledger_audit(&expense_created_audit(&current)).expect("audit encodes")],
    );
    let operations = query_rule(
        TRIP_ID,
        LEDGER_OPERATION_PREFIX,
        vec![encode_ledger_operation(&expense_operation(&current)).expect("operation encodes")],
    );
    let transaction =
        mock!(aws_sdk_dynamodb::Client::transact_write_items).then_error(cancelled_transaction);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.replace_expense(
            TRIP_ID,
            &actor(),
            ExpenseReplacement {
                expense: replacement,
                expected_revision: 1,
                context: LedgerTripContext {
                    base_currency: "GBP".into(),
                    trip_revision: 3,
                },
                audit_id: "audit-b".into(),
                audit_at: LATER_AT.into(),
            },
        )
        .await,
        Err(LedgerRepoError::Conflict)
    );
    assert_eq!(transaction.num_calls(), 1);
}

async fn assert_ambiguous_create_recovers_only_the_exact_operation(conditional: bool) {
    let value = expense("expense-a", None);
    let new = new_expense(value.clone(), 3);
    let audit = expense_created_audit(&value);
    let operation = expense_operation(&value);
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let trip = trip_meta_rule(TRIP_ID, 3, false);
    let committed_meta = encode_ledger_meta(&ledger_meta(1, 0, 1), 1).expect("meta encodes");
    let ledger_meta = get_sequence_rule(
        TRIP_ID,
        LEDGER_META_SK.into(),
        vec![
            None,
            None,
            Some(committed_meta.clone()),
            Some(committed_meta),
        ],
    );
    let expenses = query_sequence_rule(
        TRIP_ID,
        EXPENSE_PREFIX,
        vec![
            vec![],
            vec![encode_expense(&value, 1).expect("expense encodes")],
        ],
    );
    let settlements = query_sequence_rule(TRIP_ID, SETTLEMENT_PREFIX, vec![vec![], vec![]]);
    let links = query_sequence_rule(TRIP_ID, STOP_LINK_PREFIX, vec![vec![], vec![]]);
    let audits = query_sequence_rule(
        TRIP_ID,
        LEDGER_AUDIT_PREFIX,
        vec![
            vec![],
            vec![encode_ledger_audit(&audit).expect("audit encodes")],
        ],
    );
    let operations = query_sequence_rule(
        TRIP_ID,
        LEDGER_OPERATION_PREFIX,
        vec![
            vec![],
            vec![encode_ledger_operation(&operation).expect("operation encodes")],
        ],
    );
    let transaction = mock!(aws_sdk_dynamodb::Client::transact_write_items).then_error(move || {
        if conditional {
            cancelled_transaction()
        } else {
            unavailable_transaction()
        }
    });
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &ledger_meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations,
            &transaction
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("valid table");

    assert_eq!(
        repo.add_expense(TRIP_ID, &actor(), new)
            .await
            .expect("ambiguous commit recovers"),
        value
    );
}

#[tokio::test]
async fn conditional_retry_is_success_only_when_the_exact_expense_and_audit_committed() {
    assert_ambiguous_create_recovers_only_the_exact_operation(true).await;
}

#[tokio::test]
async fn nonconditional_ambiguous_failure_recovers_the_exact_committed_operation() {
    assert_ambiguous_create_recovers_only_the_exact_operation(false).await;
}

#[tokio::test]
async fn persistent_count_mismatch_is_corrupt_after_one_complete_retry() {
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let trip = trip_meta_rule(TRIP_ID, 3, false);
    let meta = ledger_meta_rule(Some(ledger_meta(1, 0, 1)), 4);
    let expenses = query_rule(TRIP_ID, EXPENSE_PREFIX, vec![]);
    let settlements = query_rule(TRIP_ID, SETTLEMENT_PREFIX, vec![]);
    let links = query_rule(TRIP_ID, STOP_LINK_PREFIX, vec![]);
    let audits = query_rule(TRIP_ID, LEDGER_AUDIT_PREFIX, vec![]);
    let operations = query_rule(TRIP_ID, LEDGER_OPERATION_PREFIX, vec![]);
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("valid table");

    assert_eq!(
        repo.get_ledger_data(TRIP_ID, &actor()).await.unwrap_err(),
        LedgerRepoError::CorruptData
    );
    assert_eq!(expenses.num_calls(), 2, "one full graph reread is allowed");
}

#[tokio::test]
async fn reverse_stop_claim_corruption_fails_closed_before_returning_an_expense() {
    let value = expense("expense-a", Some("stop-a"));
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let trip = trip_meta_rule(TRIP_ID, 3, false);
    let meta = ledger_meta_rule(Some(ledger_meta(1, 1, 1)), 5);
    let expenses = query_rule(
        TRIP_ID,
        EXPENSE_PREFIX,
        vec![encode_expense(&value, 1).expect("expense encodes")],
    );
    let settlements = query_rule(TRIP_ID, SETTLEMENT_PREFIX, vec![]);
    let links = query_rule(
        TRIP_ID,
        STOP_LINK_PREFIX,
        vec![
            encode_stop_link(&StopLinkRecord {
                trip_id: TRIP_ID.into(),
                stop_id: "stop-a".into(),
                expense_id: "expense-b".into(),
            })
            .expect("claim encodes"),
        ],
    );
    let audits = query_rule(
        TRIP_ID,
        LEDGER_AUDIT_PREFIX,
        vec![encode_ledger_audit(&expense_created_audit(&value)).expect("audit encodes")],
    );
    let operations = query_rule(
        TRIP_ID,
        LEDGER_OPERATION_PREFIX,
        vec![encode_ledger_operation(&expense_operation(&value)).expect("operation encodes")],
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.get_expense(TRIP_ID, &actor(), "expense-a").await,
        Err(LedgerRepoError::CorruptData)
    );
}

#[tokio::test]
async fn booking_pointer_must_match_the_exact_ledger_stop_claim() {
    let value = expense("expense-a", Some("stop-a"));
    let membership = member_rule(TRIP_ID, TripRole::Member);
    let trip = trip_meta_rule(TRIP_ID, 3, true);
    let meta = ledger_meta_rule(Some(ledger_meta(1, 1, 1)), 5);
    let expenses = query_rule(
        TRIP_ID,
        EXPENSE_PREFIX,
        vec![encode_expense(&value, 1).expect("expense encodes")],
    );
    let settlements = query_rule(TRIP_ID, SETTLEMENT_PREFIX, vec![]);
    let links = query_rule(
        TRIP_ID,
        STOP_LINK_PREFIX,
        vec![
            encode_stop_link(&StopLinkRecord {
                trip_id: TRIP_ID.into(),
                stop_id: "stop-a".into(),
                expense_id: "expense-a".into(),
            })
            .expect("claim encodes"),
        ],
    );
    let audits = query_rule(
        TRIP_ID,
        LEDGER_AUDIT_PREFIX,
        vec![encode_ledger_audit(&expense_created_audit(&value)).expect("audit encodes")],
    );
    let operations = query_rule(
        TRIP_ID,
        LEDGER_OPERATION_PREFIX,
        vec![encode_ledger_operation(&expense_operation(&value)).expect("operation encodes")],
    );
    let plan = query_rule(
        TRIP_ID,
        format!("{}#", plan_prefix(1)),
        plan_rows(Some(Some("expense-b"))),
    );
    let client = mock_client!(
        aws_sdk_dynamodb,
        RuleMode::MatchAny,
        [
            &membership,
            &trip,
            &meta,
            &expenses,
            &settlements,
            &links,
            &audits,
            &operations,
            &plan
        ]
    );
    let repo = DynamoUserRepo::new(client, TABLE).expect("repo");

    assert_eq!(
        repo.get_expense(TRIP_ID, &actor(), "expense-a").await,
        Err(LedgerRepoError::CorruptData)
    );
}

#[test]
fn audit_graph_reconstructs_out_of_query_order_and_rejects_missing_or_broken_provenance() {
    let mut original = expense("expense-a", None);
    original.note = "Original".into();
    let mut current = original.clone();
    current.note = "Corrected".into();
    let created = expense_created_audit(&original);
    let updated = LedgerAuditRecord {
        id: "audit-b".into(),
        trip_id: TRIP_ID.into(),
        actor_id: ACTOR_ID.into(),
        action: LedgerAuditAction::ExpenseUpdated,
        entity_id: current.id.clone(),
        before: Some(LedgerAuditValue::Expense(original.clone())),
        after: Some(LedgerAuditValue::Expense(current.clone())),
        previous_audit_id: Some("audit-a".into()),
        created_at: LATER_AT.into(),
    };
    let meta = LedgerMetaRecord {
        trip_id: TRIP_ID.into(),
        expense_count: 1,
        settlement_count: 0,
        stop_link_count: 0,
        audit_count: 2,
        operation_count: 1,
        audit_head_id: Some("audit-b".into()),
        audit_head_at: Some(LATER_AT.into()),
    };
    let expenses = HashMap::from([(current.id.clone(), loaded(current.clone(), 2))]);
    let operations = HashMap::from([(
        operation_key_hash("operation-a"),
        loaded(expense_operation(&original), 1),
    )]);

    validate_audit_graph(
        &meta,
        &expenses,
        &HashMap::new(),
        &[loaded(updated.clone(), 1), loaded(created.clone(), 1)],
        &operations,
    )
    .expect("audit query order is not chronology");

    assert_eq!(
        validate_audit_graph(
            &meta,
            &expenses,
            &HashMap::new(),
            &[loaded(updated.clone(), 1)],
            &operations,
        ),
        Err(LedgerRepoError::CorruptData)
    );

    let mut broken = updated;
    let mut unrelated = original;
    unrelated.note = "Unrelated".into();
    broken.before = Some(LedgerAuditValue::Expense(unrelated));
    assert_eq!(
        validate_audit_graph(
            &meta,
            &expenses,
            &HashMap::new(),
            &[loaded(created, 1), loaded(broken, 1)],
            &operations,
        ),
        Err(LedgerRepoError::CorruptData)
    );
}

#[test]
fn audit_backlinks_allow_state_revisits_and_equal_timestamps_without_ambiguity() {
    let mut state_a = expense("expense-a", None);
    state_a.note = "A".into();
    let mut state_b = state_a.clone();
    state_b.note = "B".into();
    let mut state_c = state_a.clone();
    state_c.note = "C".into();
    let created = expense_created_audit(&state_a);
    let to_b = LedgerAuditRecord {
        id: "audit-b".into(),
        trip_id: TRIP_ID.into(),
        actor_id: ACTOR_ID.into(),
        action: LedgerAuditAction::ExpenseUpdated,
        entity_id: state_a.id.clone(),
        before: Some(LedgerAuditValue::Expense(state_a.clone())),
        after: Some(LedgerAuditValue::Expense(state_b.clone())),
        previous_audit_id: Some("audit-a".into()),
        created_at: LATER_AT.into(),
    };
    let back_to_a = LedgerAuditRecord {
        id: "audit-c".into(),
        trip_id: TRIP_ID.into(),
        actor_id: ACTOR_ID.into(),
        action: LedgerAuditAction::ExpenseUpdated,
        entity_id: state_a.id.clone(),
        before: Some(LedgerAuditValue::Expense(state_b)),
        after: Some(LedgerAuditValue::Expense(state_a.clone())),
        previous_audit_id: Some("audit-b".into()),
        created_at: LATER_AT.into(),
    };
    let to_c = LedgerAuditRecord {
        id: "audit-d".into(),
        trip_id: TRIP_ID.into(),
        actor_id: ACTOR_ID.into(),
        action: LedgerAuditAction::ExpenseUpdated,
        entity_id: state_a.id.clone(),
        before: Some(LedgerAuditValue::Expense(state_a.clone())),
        after: Some(LedgerAuditValue::Expense(state_c.clone())),
        previous_audit_id: Some("audit-c".into()),
        created_at: LATER_AT.into(),
    };
    let meta = LedgerMetaRecord {
        trip_id: TRIP_ID.into(),
        expense_count: 1,
        settlement_count: 0,
        stop_link_count: 0,
        audit_count: 4,
        operation_count: 1,
        audit_head_id: Some("audit-d".into()),
        audit_head_at: Some(LATER_AT.into()),
    };
    let expenses = HashMap::from([(state_c.id.clone(), loaded(state_c, 4))]);
    let operations = HashMap::from([(
        operation_key_hash("operation-a"),
        loaded(expense_operation(&state_a), 1),
    )]);

    validate_audit_graph(
        &meta,
        &expenses,
        &HashMap::new(),
        &[
            loaded(back_to_a, 1),
            loaded(to_c, 1),
            loaded(created, 1),
            loaded(to_b, 1),
        ],
        &operations,
    )
    .expect("explicit predecessors order revisited states and equal-time writes");
}

#[test]
fn operation_provenance_must_bijectively_bind_the_create_audit() {
    let value = expense("expense-a", None);
    let audit = expense_created_audit(&value);
    let meta = ledger_meta(1, 0, 1);
    let expenses = HashMap::from([(value.id.clone(), loaded(value.clone(), 1))]);
    let mut forged = expense_operation(&value);
    forged.actor_id = "another-member".into();
    let operations = HashMap::from([(forged.key_hash.clone(), loaded(forged, 1))]);

    assert_eq!(
        validate_audit_graph(
            &meta,
            &expenses,
            &HashMap::new(),
            &[loaded(audit.clone(), 1)],
            &operations,
        ),
        Err(LedgerRepoError::CorruptData)
    );

    let mut forged = expense_operation(&value);
    forged.request_hash = "f".repeat(64);
    let operations = HashMap::from([(forged.key_hash.clone(), loaded(forged, 1))]);
    assert_eq!(
        validate_audit_graph(
            &meta,
            &expenses,
            &HashMap::new(),
            &[loaded(audit, 1)],
            &operations,
        ),
        Err(LedgerRepoError::CorruptData),
        "the stored request hash is recomputed from the immutable create result"
    );
}

#[test]
fn stop_link_claims_are_strict_and_trip_scoped() {
    let claim = StopLinkRecord {
        trip_id: TRIP_ID.into(),
        stop_id: "stop-a".into(),
        expense_id: "expense-a".into(),
    };
    let encoded = encode_stop_link(&claim).expect("claim encodes");
    assert_eq!(
        encoded.get(SK),
        Some(&AttributeValue::S("LEDGER#STOP#stop-a".into()))
    );
    assert_eq!(
        encoded.get(ENTITY_TYPE),
        Some(&AttributeValue::S(STOP_LINK_ENTITY.into()))
    );
    assert_eq!(encoded.get(REVISION), Some(&AttributeValue::N("1".into())));
}

#[test]
fn settlements_remain_a_distinct_strict_record_family() {
    let settlement = settlement();
    let audit = settlement_created_audit(&settlement);
    assert_eq!(
        encode_ledger_audit(&audit)
            .expect("settlement audit encodes")
            .get(ENTITY_TYPE),
        Some(&AttributeValue::S(LEDGER_AUDIT_ENTITY.into()))
    );
    assert_eq!(
        encode_ledger_operation(&settlement_operation(&settlement))
            .expect("settlement operation encodes")
            .get(ENTITY_TYPE),
        Some(&AttributeValue::S(LEDGER_OPERATION_ENTITY.into()))
    );
}
