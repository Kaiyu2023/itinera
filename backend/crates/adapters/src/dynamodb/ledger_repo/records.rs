use std::collections::HashMap;

use aws_sdk_dynamodb::types::AttributeValue;
use itinera_core::{
    domain::ledger::{Expense, Settlement},
    ports::ledger::LedgerRepoError,
    services::ledger::{
        AddExpenseInput, AddSettlementInput, expense_creation_request_hash,
        settlement_creation_request_hash, validate_stored_expense, validate_stored_settlement,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::dynamodb::{
    SK,
    trip_repo::records::{DATA, decode_record, encode_record, string, trip_pk},
};

use super::record_error;

pub(in crate::dynamodb) const LEDGER_META_ENTITY: &str = "LEDGER_META";
pub(in crate::dynamodb) const EXPENSE_ENTITY: &str = "LEDGER_EXPENSE";
pub(super) const SETTLEMENT_ENTITY: &str = "LEDGER_SETTLEMENT";
pub(in crate::dynamodb) const STOP_LINK_ENTITY: &str = "LEDGER_STOP_LINK";
pub(super) const LEDGER_AUDIT_ENTITY: &str = "LEDGER_AUDIT";
pub(super) const LEDGER_OPERATION_ENTITY: &str = "LEDGER_OPERATION";

pub(in crate::dynamodb) const LEDGER_META_SK: &str = "LEDGER#META";
pub(in crate::dynamodb) const EXPENSE_PREFIX: &str = "EXPENSE#";
pub(super) const SETTLEMENT_PREFIX: &str = "SETTLEMENT#";
pub(in crate::dynamodb) const STOP_LINK_PREFIX: &str = "LEDGER#STOP#";
pub(super) const LEDGER_AUDIT_PREFIX: &str = "LEDGER_AUDIT#";
pub(super) const LEDGER_OPERATION_PREFIX: &str = "LEDGER_OP#";

pub(super) const MAX_LEDGER_AUDITS: usize = 4_000;
pub(super) const MAX_LEDGER_OPERATIONS: usize = 4_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::dynamodb) struct LedgerMetaRecord {
    pub(in crate::dynamodb) trip_id: String,
    pub(in crate::dynamodb) expense_count: u32,
    pub(in crate::dynamodb) settlement_count: u32,
    pub(in crate::dynamodb) stop_link_count: u32,
    pub(in crate::dynamodb) audit_count: u64,
    pub(in crate::dynamodb) operation_count: u32,
    pub(in crate::dynamodb) audit_head_id: Option<String>,
    pub(in crate::dynamodb) audit_head_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::dynamodb) struct StopLinkRecord {
    pub(in crate::dynamodb) trip_id: String,
    pub(in crate::dynamodb) stop_id: String,
    pub(in crate::dynamodb) expense_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LedgerAuditAction {
    ExpenseCreated,
    ExpenseUpdated,
    ExpenseDeleted,
    SettlementCreated,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(super) enum LedgerAuditValue {
    Expense(Expense),
    Settlement(Settlement),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LedgerAuditRecord {
    pub(super) id: String,
    pub(super) trip_id: String,
    pub(super) actor_id: String,
    pub(super) action: LedgerAuditAction,
    pub(super) entity_id: String,
    pub(super) before: Option<LedgerAuditValue>,
    pub(super) after: Option<LedgerAuditValue>,
    pub(super) previous_audit_id: Option<String>,
    pub(super) created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LedgerOperationRecord {
    pub(super) trip_id: String,
    pub(super) key_hash: String,
    pub(super) actor_id: String,
    pub(super) request_hash: String,
    pub(super) result: LedgerAuditValue,
    pub(super) created_at: String,
}

#[derive(Debug, Clone)]
pub(in crate::dynamodb) struct Loaded<T> {
    pub(in crate::dynamodb) value: T,
    pub(in crate::dynamodb) revision: u64,
    pub(in crate::dynamodb) raw_data: String,
}

pub(super) fn expense_sk(expense_id: &str) -> String {
    format!("{EXPENSE_PREFIX}{expense_id}")
}

pub(super) fn settlement_sk(settlement_id: &str) -> String {
    format!("{SETTLEMENT_PREFIX}{settlement_id}")
}

pub(in crate::dynamodb) fn stop_link_sk(stop_id: &str) -> String {
    format!("{STOP_LINK_PREFIX}{stop_id}")
}

pub(super) fn ledger_audit_sk(created_at: &str, audit_id: &str) -> String {
    format!("{LEDGER_AUDIT_PREFIX}{created_at}#{audit_id}")
}

pub(super) fn operation_key_hash(idempotency_key: &str) -> String {
    let digest = Sha256::digest(idempotency_key.as_bytes());
    format!("{digest:x}")
}

pub(super) fn ledger_operation_sk(key_hash: &str) -> String {
    format!("{LEDGER_OPERATION_PREFIX}{key_hash}")
}

pub(in crate::dynamodb) fn encode_ledger_meta(
    meta: &LedgerMetaRecord,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, LedgerRepoError> {
    validate_meta(meta, revision)?;
    encode_record(
        trip_pk(&meta.trip_id),
        LEDGER_META_SK.to_string(),
        LEDGER_META_ENTITY,
        meta,
        revision,
    )
    .map_err(record_error)
}

pub(in crate::dynamodb) fn decode_ledger_meta(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<Loaded<LedgerMetaRecord>, LedgerRepoError> {
    let pk = trip_pk(expected_trip_id);
    let stored = decode_record::<LedgerMetaRecord>(item, &pk, LEDGER_META_SK, LEDGER_META_ENTITY)
        .map_err(record_error)?;
    validate_meta(&stored.value, stored.revision)?;
    if stored.value.trip_id != expected_trip_id {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
        raw_data: string(item, DATA).map_err(record_error)?,
    })
}

pub(in crate::dynamodb) fn encode_expense(
    expense: &Expense,
    revision: u64,
) -> Result<HashMap<String, AttributeValue>, LedgerRepoError> {
    if revision == 0 || validate_stored_expense(&expense.trip_id, expense).is_err() {
        return Err(LedgerRepoError::CorruptData);
    }
    encode_record(
        trip_pk(&expense.trip_id),
        expense_sk(&expense.id),
        EXPENSE_ENTITY,
        expense,
        revision,
    )
    .map_err(record_error)
}

pub(in crate::dynamodb) fn decode_expense(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<Loaded<Expense>, LedgerRepoError> {
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored = decode_record::<Expense>(item, &pk, &sk, EXPENSE_ENTITY).map_err(record_error)?;
    if stored.revision == 0
        || sk != expense_sk(&stored.value.id)
        || validate_stored_expense(expected_trip_id, &stored.value).is_err()
    {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
        raw_data: string(item, DATA).map_err(record_error)?,
    })
}

pub(super) fn encode_settlement(
    settlement: &Settlement,
) -> Result<HashMap<String, AttributeValue>, LedgerRepoError> {
    if validate_stored_settlement(&settlement.trip_id, settlement).is_err() {
        return Err(LedgerRepoError::CorruptData);
    }
    encode_record(
        trip_pk(&settlement.trip_id),
        settlement_sk(&settlement.id),
        SETTLEMENT_ENTITY,
        settlement,
        1,
    )
    .map_err(record_error)
}

pub(super) fn decode_settlement(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<Loaded<Settlement>, LedgerRepoError> {
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored =
        decode_record::<Settlement>(item, &pk, &sk, SETTLEMENT_ENTITY).map_err(record_error)?;
    if stored.revision != 1
        || sk != settlement_sk(&stored.value.id)
        || validate_stored_settlement(expected_trip_id, &stored.value).is_err()
    {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
        raw_data: string(item, DATA).map_err(record_error)?,
    })
}

pub(in crate::dynamodb) fn encode_stop_link(
    claim: &StopLinkRecord,
) -> Result<HashMap<String, AttributeValue>, LedgerRepoError> {
    if !valid_id(&claim.trip_id) || !valid_id(&claim.stop_id) || !valid_id(&claim.expense_id) {
        return Err(LedgerRepoError::CorruptData);
    }
    encode_record(
        trip_pk(&claim.trip_id),
        stop_link_sk(&claim.stop_id),
        STOP_LINK_ENTITY,
        claim,
        1,
    )
    .map_err(record_error)
}

pub(in crate::dynamodb) fn decode_stop_link(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<Loaded<StopLinkRecord>, LedgerRepoError> {
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored =
        decode_record::<StopLinkRecord>(item, &pk, &sk, STOP_LINK_ENTITY).map_err(record_error)?;
    if stored.revision != 1
        || stored.value.trip_id != expected_trip_id
        || !valid_id(&stored.value.expense_id)
        || !valid_id(&stored.value.stop_id)
        || sk != stop_link_sk(&stored.value.stop_id)
    {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
        raw_data: string(item, DATA).map_err(record_error)?,
    })
}

pub(super) fn encode_ledger_audit(
    audit: &LedgerAuditRecord,
) -> Result<HashMap<String, AttributeValue>, LedgerRepoError> {
    validate_audit(audit)?;
    encode_record(
        trip_pk(&audit.trip_id),
        ledger_audit_sk(&audit.created_at, &audit.id),
        LEDGER_AUDIT_ENTITY,
        audit,
        1,
    )
    .map_err(record_error)
}

pub(super) fn decode_ledger_audit(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<Loaded<LedgerAuditRecord>, LedgerRepoError> {
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored = decode_record::<LedgerAuditRecord>(item, &pk, &sk, LEDGER_AUDIT_ENTITY)
        .map_err(record_error)?;
    if stored.revision != 1
        || stored.value.trip_id != expected_trip_id
        || sk != ledger_audit_sk(&stored.value.created_at, &stored.value.id)
        || validate_audit(&stored.value).is_err()
    {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
        raw_data: string(item, DATA).map_err(record_error)?,
    })
}

pub(super) fn encode_ledger_operation(
    operation: &LedgerOperationRecord,
) -> Result<HashMap<String, AttributeValue>, LedgerRepoError> {
    validate_operation(operation)?;
    encode_record(
        trip_pk(&operation.trip_id),
        ledger_operation_sk(&operation.key_hash),
        LEDGER_OPERATION_ENTITY,
        operation,
        1,
    )
    .map_err(record_error)
}

pub(super) fn decode_ledger_operation(
    item: &HashMap<String, AttributeValue>,
    expected_trip_id: &str,
) -> Result<Loaded<LedgerOperationRecord>, LedgerRepoError> {
    let pk = trip_pk(expected_trip_id);
    let sk = string(item, SK).map_err(record_error)?;
    let stored = decode_record::<LedgerOperationRecord>(item, &pk, &sk, LEDGER_OPERATION_ENTITY)
        .map_err(record_error)?;
    if stored.revision != 1
        || stored.value.trip_id != expected_trip_id
        || sk != ledger_operation_sk(&stored.value.key_hash)
        || validate_operation(&stored.value).is_err()
    {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(Loaded {
        value: stored.value,
        revision: stored.revision,
        raw_data: string(item, DATA).map_err(record_error)?,
    })
}

fn validate_meta(meta: &LedgerMetaRecord, revision: u64) -> Result<(), LedgerRepoError> {
    if revision == 0
        || !valid_id(&meta.trip_id)
        || meta.expense_count > 1_000
        || meta.settlement_count > 1_000
        || meta.stop_link_count > meta.expense_count
        || meta.audit_count < u64::from(meta.expense_count) + u64::from(meta.settlement_count)
        || meta.audit_count > MAX_LEDGER_AUDITS as u64
        || meta.operation_count as usize > MAX_LEDGER_OPERATIONS
        || u64::from(meta.operation_count) > meta.audit_count
        || (meta.audit_count == 0) != (meta.audit_head_id.is_none() && meta.audit_head_at.is_none())
        || (meta.audit_count > 0) != (meta.audit_head_id.is_some() && meta.audit_head_at.is_some())
        || meta
            .audit_head_id
            .as_deref()
            .is_some_and(|value| !valid_id(value))
        || meta
            .audit_head_at
            .as_deref()
            .is_some_and(|value| !valid_utc(value))
    {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(())
}

pub(super) fn validate_operation(operation: &LedgerOperationRecord) -> Result<(), LedgerRepoError> {
    if !valid_id(&operation.trip_id)
        || !valid_id(&operation.actor_id)
        || !valid_sha256(&operation.key_hash)
        || !valid_sha256(&operation.request_hash)
        || !valid_utc(&operation.created_at)
    {
        return Err(LedgerRepoError::CorruptData);
    }
    let expected_request_hash = match &operation.result {
        LedgerAuditValue::Expense(expense) => {
            if validate_stored_expense(&operation.trip_id, expense).is_err()
                || expense.created_at != operation.created_at
                || expense.receipt_photo_url.is_some()
            {
                return Err(LedgerRepoError::CorruptData);
            }
            expense_creation_request_hash(&AddExpenseInput {
                paid_by: expense.paid_by.clone(),
                amount: expense.amount,
                currency: expense.currency.clone(),
                category: expense.category,
                split: expense.split.clone(),
                note: expense.note.clone(),
                linked_stop_id: expense.linked_stop_id.clone(),
            })
        }
        LedgerAuditValue::Settlement(settlement) => {
            if validate_stored_settlement(&operation.trip_id, settlement).is_err()
                || settlement.settled_at != operation.created_at
            {
                return Err(LedgerRepoError::CorruptData);
            }
            settlement_creation_request_hash(&AddSettlementInput {
                from_user: settlement.from_user.clone(),
                to_user: settlement.to_user.clone(),
                amount: settlement.amount,
            })
        }
    }
    .map_err(|_| LedgerRepoError::CorruptData)?;
    (operation.request_hash == expected_request_hash)
        .then_some(())
        .ok_or(LedgerRepoError::CorruptData)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_audit(audit: &LedgerAuditRecord) -> Result<(), LedgerRepoError> {
    if !valid_id(&audit.id)
        || !valid_id(&audit.trip_id)
        || !valid_id(&audit.actor_id)
        || !valid_id(&audit.entity_id)
        || audit
            .previous_audit_id
            .as_deref()
            .is_some_and(|id| !valid_id(id) || id == audit.id)
        || !valid_utc(&audit.created_at)
    {
        return Err(LedgerRepoError::CorruptData);
    }
    let before_expense = match &audit.before {
        Some(LedgerAuditValue::Expense(value)) => {
            validate_stored_expense(&audit.trip_id, value).is_ok() && value.id == audit.entity_id
        }
        _ => false,
    };
    let after_expense = match &audit.after {
        Some(LedgerAuditValue::Expense(value)) => {
            validate_stored_expense(&audit.trip_id, value).is_ok() && value.id == audit.entity_id
        }
        _ => false,
    };
    let after_settlement = match &audit.after {
        Some(LedgerAuditValue::Settlement(value)) => {
            validate_stored_settlement(&audit.trip_id, value).is_ok() && value.id == audit.entity_id
        }
        _ => false,
    };
    let valid_shape = match audit.action {
        LedgerAuditAction::ExpenseCreated => {
            audit.before.is_none()
                && after_expense
                && matches!(
                    &audit.after,
                    Some(LedgerAuditValue::Expense(value))
                        if value.created_at == audit.created_at
                )
        }
        LedgerAuditAction::ExpenseUpdated => {
            before_expense
                && after_expense
                && audit.before != audit.after
                && matches!(
                    (&audit.before, &audit.after),
                    (
                        Some(LedgerAuditValue::Expense(before)),
                        Some(LedgerAuditValue::Expense(after))
                    ) if before.created_at == after.created_at
                        && timestamp_at_or_after(&audit.created_at, &before.created_at)
                )
        }
        LedgerAuditAction::ExpenseDeleted => {
            before_expense
                && audit.after.is_none()
                && matches!(
                    &audit.before,
                    Some(LedgerAuditValue::Expense(value))
                        if timestamp_at_or_after(&audit.created_at, &value.created_at)
                )
        }
        LedgerAuditAction::SettlementCreated => {
            audit.before.is_none()
                && after_settlement
                && matches!(
                    &audit.after,
                    Some(LedgerAuditValue::Settlement(value))
                        if value.settled_at == audit.created_at
                )
        }
    };
    if valid_shape {
        Ok(())
    } else {
        Err(LedgerRepoError::CorruptData)
    }
}

fn valid_utc(value: &str) -> bool {
    value.len() <= 64
        && value.ends_with('Z')
        && chrono::DateTime::parse_from_rfc3339(value)
            .is_ok_and(|timestamp| timestamp.offset().local_minus_utc() == 0)
}

fn timestamp_at_or_after(left: &str, right: &str) -> bool {
    match (
        chrono::DateTime::parse_from_rfc3339(left),
        chrono::DateTime::parse_from_rfc3339(right),
    ) {
        (Ok(left), Ok(right)) => left >= right,
        _ => false,
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.trim() == value && value.chars().count() <= 200
}
