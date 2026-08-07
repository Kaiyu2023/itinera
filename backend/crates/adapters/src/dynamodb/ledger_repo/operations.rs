use std::collections::{HashMap, HashSet};

use aws_sdk_dynamodb::types::{AttributeValue, Delete, TransactWriteItem};
use chrono::DateTime;
use itinera_core::{
    domain::{
        ledger::{Expense, Settlement},
        trip::{Day, Plan, Stop},
        user::UserId,
    },
    ports::ledger::{
        ExpenseReplacement, LedgerData, LedgerRepoError, LedgerTripContext, NewExpense,
        NewSettlement, VersionedExpense,
    },
    services::{
        ledger::{expense_participant_ids, validate_stored_expense, validate_stored_settlement},
        plans::validate_stored_plan_graph,
    },
};

use crate::dynamodb::{
    DynamoUserRepo, ENTITY_TYPE, REVISION, SK,
    primitives::{
        condition_action, delete_action, item_key, put_action, transaction_condition_failed,
    },
    trip_repo::records::{
        DAY_ENTITY, PLAN_ENTITY, STOP_ENTITY, Stored, day_sk, decode_record, encode_record,
        plan_prefix, plan_sk, stop_sk, string, trip_pk,
    },
};

use super::{
    access::{
        LedgerReadBudget, LoadedTripMeta, MAX_LEDGER_BYTES, MAX_LEDGER_ROWS, RequiredLedgerRole,
    },
    record_error,
    records::{
        EXPENSE_ENTITY, EXPENSE_PREFIX, LEDGER_AUDIT_PREFIX, LEDGER_META_SK,
        LEDGER_OPERATION_PREFIX, LedgerAuditAction, LedgerAuditRecord, LedgerAuditValue,
        LedgerMetaRecord, LedgerOperationRecord, Loaded, MAX_LEDGER_AUDITS, MAX_LEDGER_OPERATIONS,
        SETTLEMENT_PREFIX, STOP_LINK_ENTITY, STOP_LINK_PREFIX, StopLinkRecord, decode_expense,
        decode_ledger_audit, decode_ledger_meta, decode_ledger_operation, decode_settlement,
        decode_stop_link, encode_expense, encode_ledger_audit, encode_ledger_meta,
        encode_ledger_operation, encode_settlement, encode_stop_link, expense_sk,
        operation_key_hash, stop_link_sk, validate_operation,
    },
};

struct LedgerState {
    meta: Option<Loaded<LedgerMetaRecord>>,
    expenses: HashMap<String, Loaded<Expense>>,
    settlements: HashMap<String, Loaded<Settlement>>,
    stop_links: HashMap<String, Loaded<StopLinkRecord>>,
    audits: Vec<Loaded<LedgerAuditRecord>>,
    operations: HashMap<String, Loaded<LedgerOperationRecord>>,
}

struct LoadedStop {
    value: Stop,
    revision: u64,
    sort_key: String,
}

struct CurrentPlanGraph {
    plan_revision: u64,
    plan_sort_key: String,
    stops: HashMap<String, LoadedStop>,
}

pub(super) async fn get_ledger_data(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<LedgerData, LedgerRepoError> {
    repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Any)
        .await?;
    let trip = repo.ledger_trip_meta(trip_id).await?;
    let (state, mut budget) = load_complete_ledger_state(repo, trip_id, &trip).await?;
    let current_member_ids = repo
        .ledger_current_members(trip_id, trip.value.member_count, &mut budget)
        .await?;
    let mut expenses = state
        .expenses
        .into_values()
        .map(|loaded| loaded.value)
        .collect::<Vec<_>>();
    expenses.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    let mut settlements = state
        .settlements
        .into_values()
        .map(|loaded| loaded.value)
        .collect::<Vec<_>>();
    settlements.sort_by(|left, right| {
        left.settled_at
            .cmp(&right.settled_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    enforce_response_limit((&current_member_ids, &expenses, &settlements))?;
    Ok(LedgerData {
        base_currency: trip.value.base_currency,
        current_member_ids,
        expenses,
        settlements,
    })
}

pub(super) async fn get_trip_context(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<LedgerTripContext, LedgerRepoError> {
    repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
        .await?;
    let trip = repo.ledger_trip_meta(trip_id).await?;
    Ok(context(&trip))
}

pub(super) async fn replay_expense_creation(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<Option<Expense>, LedgerRepoError> {
    repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
        .await?;
    let trip = repo.ledger_trip_meta(trip_id).await?;
    let (state, _) = load_complete_ledger_state(repo, trip_id, &trip).await?;
    match replay_operation(&state, actor, idempotency_key, request_hash)? {
        None => Ok(None),
        Some(LedgerAuditValue::Expense(expense)) => Ok(Some(expense)),
        Some(LedgerAuditValue::Settlement(_)) => Err(LedgerRepoError::Conflict),
    }
}

pub(super) async fn replay_settlement_creation(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<Option<Settlement>, LedgerRepoError> {
    repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
        .await?;
    let trip = repo.ledger_trip_meta(trip_id).await?;
    let (state, _) = load_complete_ledger_state(repo, trip_id, &trip).await?;
    match replay_operation(&state, actor, idempotency_key, request_hash)? {
        None => Ok(None),
        Some(LedgerAuditValue::Settlement(settlement)) => Ok(Some(settlement)),
        Some(LedgerAuditValue::Expense(_)) => Err(LedgerRepoError::Conflict),
    }
}

pub(super) async fn get_expense(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    expense_id: &str,
) -> Result<VersionedExpense, LedgerRepoError> {
    repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
        .await?;
    let trip = repo.ledger_trip_meta(trip_id).await?;
    let (state, _) = load_complete_ledger_state(repo, trip_id, &trip).await?;
    let loaded = state
        .expenses
        .get(expense_id)
        .ok_or(LedgerRepoError::NotFound)?;
    Ok(VersionedExpense {
        expense: loaded.value.clone(),
        revision: loaded.revision,
        context: context(&trip),
    })
}

pub(super) async fn add_expense(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    new: NewExpense,
) -> Result<Expense, LedgerRepoError> {
    repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
        .await?;
    if new.expense.trip_id != trip_id
        || new.expense.receipt_photo_url.is_some()
        || validate_stored_expense(trip_id, &new.expense).is_err()
        || new.audit_at != new.expense.created_at
        || !valid_operation_input(&new.idempotency_key, &new.request_hash)
    {
        return Err(LedgerRepoError::CorruptData);
    }
    let trip = repo.ledger_trip_meta(trip_id).await?;
    validate_context(&new.context, &trip)?;
    let (state, mut budget) = load_complete_ledger_state(repo, trip_id, &trip).await?;
    if let Some(result) = replay_operation(&state, actor, &new.idempotency_key, &new.request_hash)?
    {
        return match result {
            LedgerAuditValue::Expense(expense) => Ok(expense),
            LedgerAuditValue::Settlement(_) => Err(LedgerRepoError::Conflict),
        };
    }
    if state.expenses.contains_key(&new.expense.id) {
        return Err(LedgerRepoError::Conflict);
    }
    if state.expenses.len() >= MAX_LEDGER_ROWS {
        return Err(LedgerRepoError::SafetyLimitExceeded);
    }
    let members = expense_member_ids(&new.expense);
    repo.ledger_require_members(trip_id, &members).await?;
    let audit = expense_audit(
        &new.audit_id,
        actor,
        LedgerAuditAction::ExpenseCreated,
        None,
        Some(new.expense.clone()),
        audit_predecessor(&state),
        &new.audit_at,
    )?;
    validate_next_audit(&state, &audit)?;
    let operation = ledger_operation(
        trip_id,
        actor,
        &new.idempotency_key,
        &new.request_hash,
        LedgerAuditValue::Expense(new.expense.clone()),
        &new.audit_at,
    )?;
    let next_meta = next_meta(
        &state,
        trip_id,
        1,
        0,
        link_delta(None, &new.expense),
        Some(&audit),
        1,
    )?;
    let mut actions = mutation_prelude(repo, trip_id, actor, &members, &trip);
    if let Some(stop_id) = new.expense.linked_stop_id.as_deref() {
        if state.stop_links.contains_key(stop_id) {
            return Err(LedgerRepoError::Conflict);
        }
        actions.extend(
            prepare_stop_changes(
                repo,
                trip_id,
                &trip,
                None,
                Some((stop_id, new.expense.id.as_str())),
                &mut budget,
            )
            .await?,
        );
        actions.push(put_action(repo.create_only_put(encode_stop_link(
            &StopLinkRecord {
                trip_id: trip_id.to_string(),
                stop_id: stop_id.to_string(),
                expense_id: new.expense.id.clone(),
            },
        )?)));
    }
    actions.extend([
        put_action(meta_put(repo, &state, next_meta)?),
        put_action(repo.create_only_put(encode_expense(&new.expense, 1)?)),
        put_action(repo.create_only_put(encode_ledger_audit(&audit)?)),
        put_action(repo.create_only_put(encode_ledger_operation(&operation)?)),
    ]);
    enforce_transaction_limit(&actions)?;
    match repo
        .transaction()
        .set_transact_items(Some(actions))
        .send()
        .await
    {
        Ok(_) => Ok(new.expense),
        Err(error) => {
            repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
                .await?;
            if let Some(expense) = replay_expense_operation(
                repo,
                trip_id,
                actor,
                &new.idempotency_key,
                &new.request_hash,
            )
            .await?
            {
                Ok(expense)
            } else if transaction_condition_failed(error.as_service_error()) {
                Err(LedgerRepoError::Conflict)
            } else {
                Err(LedgerRepoError::Unavailable)
            }
        }
    }
}

pub(super) async fn replace_expense(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    replacement: ExpenseReplacement,
) -> Result<Expense, LedgerRepoError> {
    repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
        .await?;
    if replacement.expense.trip_id != trip_id
        || validate_stored_expense(trip_id, &replacement.expense).is_err()
    {
        return Err(LedgerRepoError::CorruptData);
    }
    let trip = repo.ledger_trip_meta(trip_id).await?;
    validate_context(&replacement.context, &trip)?;
    let (state, mut budget) = load_complete_ledger_state(repo, trip_id, &trip).await?;
    let current = state
        .expenses
        .get(&replacement.expense.id)
        .ok_or(LedgerRepoError::NotFound)?;
    if current.revision != replacement.expected_revision {
        return Err(LedgerRepoError::Conflict);
    }
    validate_replacement(&current.value, &replacement.expense)?;
    if current.value == replacement.expense {
        return Ok(current.value.clone());
    }
    if utc(&replacement.audit_at)? < utc(&current.value.created_at)? {
        return Err(LedgerRepoError::Conflict);
    }
    let members = expense_member_ids(&replacement.expense);
    repo.ledger_require_members(trip_id, &members).await?;
    let audit = expense_audit(
        &replacement.audit_id,
        actor,
        LedgerAuditAction::ExpenseUpdated,
        Some(current.value.clone()),
        Some(replacement.expense.clone()),
        audit_predecessor(&state),
        &replacement.audit_at,
    )?;
    validate_next_audit(&state, &audit)?;
    let next_meta = next_meta(
        &state,
        trip_id,
        0,
        0,
        link_delta(
            current.value.linked_stop_id.as_deref(),
            &replacement.expense,
        ),
        Some(&audit),
        0,
    )?;
    let mut actions = mutation_prelude(repo, trip_id, actor, &members, &trip);
    if current.value.linked_stop_id != replacement.expense.linked_stop_id {
        let old = current
            .value
            .linked_stop_id
            .as_deref()
            .map(|stop_id| (stop_id, current.value.id.as_str()));
        let new_link = replacement
            .expense
            .linked_stop_id
            .as_deref()
            .map(|stop_id| (stop_id, replacement.expense.id.as_str()));
        if let Some((stop_id, _)) = new_link
            && state.stop_links.contains_key(stop_id)
        {
            return Err(LedgerRepoError::Conflict);
        }
        actions
            .extend(prepare_stop_changes(repo, trip_id, &trip, old, new_link, &mut budget).await?);
        if let Some((stop_id, expense_id)) = old {
            actions.push(delete_action(guarded_delete(
                repo,
                trip_pk(trip_id),
                stop_link_sk(stop_id),
                STOP_LINK_ENTITY,
                1,
            )));
            debug_assert_eq!(expense_id, current.value.id);
        }
        if let Some((stop_id, expense_id)) = new_link {
            actions.push(put_action(repo.create_only_put(encode_stop_link(
                &StopLinkRecord {
                    trip_id: trip_id.to_string(),
                    stop_id: stop_id.to_string(),
                    expense_id: expense_id.to_string(),
                },
            )?)));
        }
    }
    let next_revision = current
        .revision
        .checked_add(1)
        .ok_or(LedgerRepoError::CorruptData)?;
    actions.extend([
        put_action(meta_put(repo, &state, next_meta)?),
        put_action(repo.revision_put(
            encode_expense(&replacement.expense, next_revision)?,
            current.revision,
        )),
        put_action(repo.create_only_put(encode_ledger_audit(&audit)?)),
    ]);
    enforce_transaction_limit(&actions)?;
    match repo
        .transaction()
        .set_transact_items(Some(actions))
        .send()
        .await
    {
        Ok(_) => Ok(replacement.expense),
        Err(error) => {
            repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
                .await?;
            if committed_expense_and_audit(repo, trip_id, &replacement.expense, &audit).await? {
                Ok(replacement.expense)
            } else if transaction_condition_failed(error.as_service_error()) {
                Err(LedgerRepoError::Conflict)
            } else {
                Err(LedgerRepoError::Unavailable)
            }
        }
    }
}

pub(super) async fn delete_expense(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    expense_id: &str,
    audit_id: &str,
    audit_at: &str,
) -> Result<(), LedgerRepoError> {
    repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
        .await?;
    let trip = repo.ledger_trip_meta(trip_id).await?;
    let (state, mut budget) = load_complete_ledger_state(repo, trip_id, &trip).await?;
    let current = state
        .expenses
        .get(expense_id)
        .ok_or(LedgerRepoError::NotFound)?;
    if utc(audit_at)? < utc(&current.value.created_at)? {
        return Err(LedgerRepoError::Conflict);
    }
    let audit = expense_audit(
        audit_id,
        actor,
        LedgerAuditAction::ExpenseDeleted,
        Some(current.value.clone()),
        None,
        audit_predecessor(&state),
        audit_at,
    )?;
    validate_next_audit(&state, &audit)?;
    let next_meta = next_meta(
        &state,
        trip_id,
        -1,
        0,
        if current.value.linked_stop_id.is_some() {
            -1
        } else {
            0
        },
        Some(&audit),
        0,
    )?;
    let mut actions = mutation_prelude(repo, trip_id, actor, &HashSet::new(), &trip);
    if let Some(stop_id) = current.value.linked_stop_id.as_deref() {
        actions.extend(
            prepare_stop_changes(
                repo,
                trip_id,
                &trip,
                Some((stop_id, current.value.id.as_str())),
                None,
                &mut budget,
            )
            .await?,
        );
        actions.push(delete_action(guarded_delete(
            repo,
            trip_pk(trip_id),
            stop_link_sk(stop_id),
            STOP_LINK_ENTITY,
            1,
        )));
    }
    actions.extend([
        put_action(meta_put(repo, &state, next_meta)?),
        delete_action(guarded_delete(
            repo,
            trip_pk(trip_id),
            expense_sk(expense_id),
            EXPENSE_ENTITY,
            current.revision,
        )),
        put_action(repo.create_only_put(encode_ledger_audit(&audit)?)),
    ]);
    enforce_transaction_limit(&actions)?;
    match repo
        .transaction()
        .set_transact_items(Some(actions))
        .send()
        .await
    {
        Ok(_) => Ok(()),
        Err(error) => {
            repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
                .await?;
            let expense = repo
                .ledger_get(&trip_pk(trip_id), &expense_sk(expense_id))
                .await?;
            if expense.is_none() && committed_audit(repo, trip_id, &audit).await? {
                Ok(())
            } else if transaction_condition_failed(error.as_service_error()) {
                Err(LedgerRepoError::Conflict)
            } else {
                Err(LedgerRepoError::Unavailable)
            }
        }
    }
}

pub(super) async fn add_settlement(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    new: NewSettlement,
) -> Result<Settlement, LedgerRepoError> {
    repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
        .await?;
    if new.settlement.trip_id != trip_id
        || validate_stored_settlement(trip_id, &new.settlement).is_err()
        || new.audit_at != new.settlement.settled_at
        || !valid_operation_input(&new.idempotency_key, &new.request_hash)
    {
        return Err(LedgerRepoError::CorruptData);
    }
    let trip = repo.ledger_trip_meta(trip_id).await?;
    let (state, _) = load_complete_ledger_state(repo, trip_id, &trip).await?;
    if let Some(result) = replay_operation(&state, actor, &new.idempotency_key, &new.request_hash)?
    {
        return match result {
            LedgerAuditValue::Settlement(settlement) => Ok(settlement),
            LedgerAuditValue::Expense(_) => Err(LedgerRepoError::Conflict),
        };
    }
    if state.settlements.contains_key(&new.settlement.id) {
        return Err(LedgerRepoError::Conflict);
    }
    if state.settlements.len() >= MAX_LEDGER_ROWS {
        return Err(LedgerRepoError::SafetyLimitExceeded);
    }
    let members = HashSet::from([
        new.settlement.from_user.clone(),
        new.settlement.to_user.clone(),
    ]);
    repo.ledger_require_members(trip_id, &members).await?;
    let audit = settlement_audit(&new, trip_id, actor, audit_predecessor(&state));
    validate_next_audit(&state, &audit)?;
    let operation = ledger_operation(
        trip_id,
        actor,
        &new.idempotency_key,
        &new.request_hash,
        LedgerAuditValue::Settlement(new.settlement.clone()),
        &new.audit_at,
    )?;
    let next_meta = next_meta(&state, trip_id, 0, 1, 0, Some(&audit), 1)?;
    let mut actions = mutation_prelude(repo, trip_id, actor, &members, &trip);
    actions.extend([
        put_action(meta_put(repo, &state, next_meta)?),
        put_action(repo.create_only_put(encode_settlement(&new.settlement)?)),
        put_action(repo.create_only_put(encode_ledger_audit(&audit)?)),
        put_action(repo.create_only_put(encode_ledger_operation(&operation)?)),
    ]);
    enforce_transaction_limit(&actions)?;
    match repo
        .transaction()
        .set_transact_items(Some(actions))
        .send()
        .await
    {
        Ok(_) => Ok(new.settlement),
        Err(error) => {
            repo.ledger_authorize(trip_id, actor, RequiredLedgerRole::Editor)
                .await?;
            if let Some(settlement) = replay_settlement_operation(
                repo,
                trip_id,
                actor,
                &new.idempotency_key,
                &new.request_hash,
            )
            .await?
            {
                Ok(settlement)
            } else if transaction_condition_failed(error.as_service_error()) {
                Err(LedgerRepoError::Conflict)
            } else {
                Err(LedgerRepoError::Unavailable)
            }
        }
    }
}

async fn load_complete_ledger_state(
    repo: &DynamoUserRepo,
    trip_id: &str,
    trip: &LoadedTripMeta,
) -> Result<(LedgerState, LedgerReadBudget), LedgerRepoError> {
    let (state, mut budget) = load_ledger_state_with_retry(repo, trip_id).await?;
    validate_current_stop_links(repo, trip_id, trip, &state, &mut budget).await?;
    Ok((state, budget))
}

async fn load_ledger_state_with_retry(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<(LedgerState, LedgerReadBudget), LedgerRepoError> {
    match load_ledger_state(repo, trip_id).await {
        Err(LedgerRepoError::CorruptData) => load_ledger_state(repo, trip_id).await,
        result => result,
    }
}

async fn validate_current_stop_links(
    repo: &DynamoUserRepo,
    trip_id: &str,
    trip: &LoadedTripMeta,
    state: &LedgerState,
    budget: &mut LedgerReadBudget,
) -> Result<(), LedgerRepoError> {
    let graph = load_current_plan_graph(repo, trip_id, trip, budget).await?;
    let Some(graph) = graph else {
        return if state.stop_links.is_empty() {
            Ok(())
        } else {
            Err(LedgerRepoError::CorruptData)
        };
    };
    for stop in graph.stops.values() {
        let pointer = stop
            .value
            .booking
            .as_ref()
            .and_then(|booking| booking.ledger_entry_id.as_deref());
        match pointer {
            Some(expense_id)
                if state
                    .stop_links
                    .get(&stop.value.id)
                    .is_some_and(|claim| claim.value.expense_id == expense_id) => {}
            Some(_) => return Err(LedgerRepoError::CorruptData),
            None if state.stop_links.contains_key(&stop.value.id) => {
                return Err(LedgerRepoError::CorruptData);
            }
            None => {}
        }
    }
    for claim in state.stop_links.values() {
        if !graph.stops.get(&claim.value.stop_id).is_some_and(|stop| {
            stop.value
                .booking
                .as_ref()
                .and_then(|booking| booking.ledger_entry_id.as_deref())
                == Some(claim.value.expense_id.as_str())
        }) {
            return Err(LedgerRepoError::CorruptData);
        }
    }
    Ok(())
}

async fn load_ledger_state(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<(LedgerState, LedgerReadBudget), LedgerRepoError> {
    let pk = trip_pk(trip_id);
    let before = load_meta(repo, trip_id).await?;
    let mut budget = LedgerReadBudget::default();
    let expense_items = repo
        .ledger_query(&pk, EXPENSE_PREFIX, MAX_LEDGER_ROWS, &mut budget)
        .await?;
    let settlement_items = repo
        .ledger_query(&pk, SETTLEMENT_PREFIX, MAX_LEDGER_ROWS, &mut budget)
        .await?;
    let link_items = repo
        .ledger_query(&pk, STOP_LINK_PREFIX, MAX_LEDGER_ROWS, &mut budget)
        .await?;
    let audit_items = repo
        .ledger_query(&pk, LEDGER_AUDIT_PREFIX, MAX_LEDGER_AUDITS, &mut budget)
        .await?;
    let operation_items = repo
        .ledger_query(
            &pk,
            LEDGER_OPERATION_PREFIX,
            MAX_LEDGER_OPERATIONS,
            &mut budget,
        )
        .await?;
    let after = load_meta(repo, trip_id).await?;
    if !same_meta(&before, &after) {
        return Err(LedgerRepoError::CorruptData);
    }
    let Some(meta) = before else {
        return if expense_items.is_empty()
            && settlement_items.is_empty()
            && link_items.is_empty()
            && audit_items.is_empty()
            && operation_items.is_empty()
        {
            Ok((
                LedgerState {
                    meta: None,
                    expenses: HashMap::new(),
                    settlements: HashMap::new(),
                    stop_links: HashMap::new(),
                    audits: Vec::new(),
                    operations: HashMap::new(),
                },
                budget,
            ))
        } else {
            Err(LedgerRepoError::CorruptData)
        };
    };
    if expense_items.len() != meta.value.expense_count as usize
        || settlement_items.len() != meta.value.settlement_count as usize
        || link_items.len() != meta.value.stop_link_count as usize
        || audit_items.len() != meta.value.audit_count as usize
        || operation_items.len() != meta.value.operation_count as usize
    {
        return Err(LedgerRepoError::CorruptData);
    }
    let mut expenses = HashMap::new();
    for item in expense_items {
        let loaded = decode_expense(&item, trip_id)?;
        if expenses.insert(loaded.value.id.clone(), loaded).is_some() {
            return Err(LedgerRepoError::CorruptData);
        }
    }
    let mut settlements = HashMap::new();
    for item in settlement_items {
        let loaded = decode_settlement(&item, trip_id)?;
        if settlements
            .insert(loaded.value.id.clone(), loaded)
            .is_some()
        {
            return Err(LedgerRepoError::CorruptData);
        }
    }
    let mut stop_links = HashMap::new();
    for item in link_items {
        let loaded = decode_stop_link(&item, trip_id)?;
        if stop_links
            .insert(loaded.value.stop_id.clone(), loaded)
            .is_some()
        {
            return Err(LedgerRepoError::CorruptData);
        }
    }
    let mut audits = Vec::with_capacity(audit_items.len());
    let mut audit_ids = HashSet::new();
    for item in audit_items {
        let loaded = decode_ledger_audit(&item, trip_id)?;
        if !audit_ids.insert(loaded.value.id.clone()) {
            return Err(LedgerRepoError::CorruptData);
        }
        audits.push(loaded);
    }
    let mut operations = HashMap::new();
    for item in operation_items {
        let loaded = decode_ledger_operation(&item, trip_id)?;
        if operations
            .insert(loaded.value.key_hash.clone(), loaded)
            .is_some()
        {
            return Err(LedgerRepoError::CorruptData);
        }
    }
    for expense in expenses.values() {
        match expense.value.linked_stop_id.as_deref() {
            Some(stop_id)
                if stop_links
                    .get(stop_id)
                    .is_some_and(|claim| claim.value.expense_id == expense.value.id) => {}
            Some(_) => return Err(LedgerRepoError::CorruptData),
            None => {}
        }
    }
    for claim in stop_links.values() {
        if !expenses
            .get(&claim.value.expense_id)
            .is_some_and(|expense| {
                expense.value.linked_stop_id.as_deref() == Some(claim.value.stop_id.as_str())
            })
        {
            return Err(LedgerRepoError::CorruptData);
        }
    }
    validate_audit_graph(&meta.value, &expenses, &settlements, &audits, &operations)?;
    Ok((
        LedgerState {
            meta: Some(meta),
            expenses,
            settlements,
            stop_links,
            audits,
            operations,
        },
        budget,
    ))
}

async fn load_meta(
    repo: &DynamoUserRepo,
    trip_id: &str,
) -> Result<Option<Loaded<LedgerMetaRecord>>, LedgerRepoError> {
    repo.ledger_get(&trip_pk(trip_id), LEDGER_META_SK)
        .await?
        .map(|item| decode_ledger_meta(&item, trip_id))
        .transpose()
}

fn same_meta(
    left: &Option<Loaded<LedgerMetaRecord>>,
    right: &Option<Loaded<LedgerMetaRecord>>,
) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.revision == right.revision && left.value == right.value,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LedgerResultKey {
    Expense(String),
    Settlement(String),
}

#[derive(Debug, Clone)]
struct CreateProvenance {
    actor_id: String,
    created_at: String,
    result: LedgerAuditValue,
}

pub(super) fn validate_audit_graph(
    meta: &LedgerMetaRecord,
    expenses: &HashMap<String, Loaded<Expense>>,
    settlements: &HashMap<String, Loaded<Settlement>>,
    audits: &[Loaded<LedgerAuditRecord>],
    operations: &HashMap<String, Loaded<LedgerOperationRecord>>,
) -> Result<(), LedgerRepoError> {
    if audits.is_empty() {
        return if expenses.is_empty() && settlements.is_empty() && operations.is_empty() {
            Ok(())
        } else {
            Err(LedgerRepoError::CorruptData)
        };
    }
    let ordered_audits = ordered_audit_chain(meta, audits)?;

    let mut expense_audits = HashMap::<String, Vec<&LedgerAuditRecord>>::new();
    let mut settlement_audits = HashMap::<String, Vec<&LedgerAuditRecord>>::new();
    for audit in ordered_audits {
        match audit.action {
            LedgerAuditAction::ExpenseCreated
            | LedgerAuditAction::ExpenseUpdated
            | LedgerAuditAction::ExpenseDeleted => expense_audits
                .entry(audit.entity_id.clone())
                .or_default()
                .push(audit),
            LedgerAuditAction::SettlementCreated => settlement_audits
                .entry(audit.entity_id.clone())
                .or_default()
                .push(audit),
        }
    }

    let mut provenance = HashMap::<LedgerResultKey, CreateProvenance>::new();
    for (expense_id, chain) in &expense_audits {
        if chain.is_empty()
            || chain[0].action != LedgerAuditAction::ExpenseCreated
            || chain
                .iter()
                .skip(1)
                .any(|audit| audit.action == LedgerAuditAction::ExpenseCreated)
        {
            return Err(LedgerRepoError::CorruptData);
        }
        let create = chain[0];
        let Some(LedgerAuditValue::Expense(created)) = create.after.as_ref() else {
            return Err(LedgerRepoError::CorruptData);
        };
        let mut current = created.clone();
        let mut revision = 1_u64;
        let mut deleted = false;
        for next in chain.iter().skip(1).copied() {
            if deleted
                || !matches!(
                    next.before.as_ref(),
                    Some(LedgerAuditValue::Expense(before)) if before == &current
                )
            {
                return Err(LedgerRepoError::CorruptData);
            }
            match (&next.action, &next.after) {
                (LedgerAuditAction::ExpenseUpdated, Some(LedgerAuditValue::Expense(after))) => {
                    current = after.clone();
                    revision = revision
                        .checked_add(1)
                        .ok_or(LedgerRepoError::CorruptData)?;
                }
                (LedgerAuditAction::ExpenseDeleted, None) => deleted = true,
                _ => return Err(LedgerRepoError::CorruptData),
            }
        }
        match (deleted, expenses.get(expense_id)) {
            (true, None) => {}
            (false, Some(stored)) if stored.value == current && stored.revision == revision => {}
            _ => return Err(LedgerRepoError::CorruptData),
        }
        if provenance
            .insert(
                LedgerResultKey::Expense(expense_id.clone()),
                CreateProvenance {
                    actor_id: create.actor_id.clone(),
                    created_at: create.created_at.clone(),
                    result: LedgerAuditValue::Expense(created.clone()),
                },
            )
            .is_some()
        {
            return Err(LedgerRepoError::CorruptData);
        }
    }
    if expenses
        .keys()
        .any(|expense_id| !expense_audits.contains_key(expense_id))
    {
        return Err(LedgerRepoError::CorruptData);
    }

    for (settlement_id, chain) in &settlement_audits {
        if chain.len() != 1 || chain[0].action != LedgerAuditAction::SettlementCreated {
            return Err(LedgerRepoError::CorruptData);
        }
        let create = chain[0];
        let Some(LedgerAuditValue::Settlement(created)) = create.after.as_ref() else {
            return Err(LedgerRepoError::CorruptData);
        };
        if !settlements
            .get(settlement_id)
            .is_some_and(|stored| stored.revision == 1 && stored.value == *created)
        {
            return Err(LedgerRepoError::CorruptData);
        }
        provenance.insert(
            LedgerResultKey::Settlement(settlement_id.clone()),
            CreateProvenance {
                actor_id: create.actor_id.clone(),
                created_at: create.created_at.clone(),
                result: LedgerAuditValue::Settlement(created.clone()),
            },
        );
    }
    if settlements
        .keys()
        .any(|settlement_id| !settlement_audits.contains_key(settlement_id))
    {
        return Err(LedgerRepoError::CorruptData);
    }

    for operation in operations.values() {
        validate_operation(&operation.value)?;
        let key = result_key(&operation.value.result);
        let Some(created) = provenance.remove(&key) else {
            return Err(LedgerRepoError::CorruptData);
        };
        if operation.value.actor_id != created.actor_id
            || operation.value.created_at != created.created_at
            || operation.value.result != created.result
        {
            return Err(LedgerRepoError::CorruptData);
        }
    }
    if provenance.is_empty() {
        Ok(())
    } else {
        Err(LedgerRepoError::CorruptData)
    }
}

fn ordered_audit_chain<'a>(
    meta: &LedgerMetaRecord,
    audits: &'a [Loaded<LedgerAuditRecord>],
) -> Result<Vec<&'a LedgerAuditRecord>, LedgerRepoError> {
    let head_id = meta
        .audit_head_id
        .as_deref()
        .ok_or(LedgerRepoError::CorruptData)?;
    let head_at = meta
        .audit_head_at
        .as_deref()
        .ok_or(LedgerRepoError::CorruptData)?;
    let mut by_id = HashMap::<&str, &LedgerAuditRecord>::with_capacity(audits.len());
    for audit in audits {
        if by_id
            .insert(audit.value.id.as_str(), &audit.value)
            .is_some()
        {
            return Err(LedgerRepoError::CorruptData);
        }
    }

    let mut newest_first = Vec::with_capacity(audits.len());
    let mut seen = HashSet::with_capacity(audits.len());
    let mut next_id = Some(head_id);
    while let Some(id) = next_id {
        if !seen.insert(id) {
            return Err(LedgerRepoError::CorruptData);
        }
        let audit = by_id.get(id).copied().ok_or(LedgerRepoError::CorruptData)?;
        newest_first.push(audit);
        next_id = audit.previous_audit_id.as_deref();
    }
    if newest_first.len() != audits.len()
        || newest_first[0].created_at != head_at
        || newest_first.windows(2).any(|pair| {
            match (utc(&pair[0].created_at), utc(&pair[1].created_at)) {
                (Ok(newer), Ok(older)) => newer < older,
                _ => true,
            }
        })
    {
        return Err(LedgerRepoError::CorruptData);
    }
    newest_first.reverse();
    Ok(newest_first)
}

fn result_key(value: &LedgerAuditValue) -> LedgerResultKey {
    match value {
        LedgerAuditValue::Expense(expense) => LedgerResultKey::Expense(expense.id.clone()),
        LedgerAuditValue::Settlement(settlement) => {
            LedgerResultKey::Settlement(settlement.id.clone())
        }
    }
}

async fn load_current_plan_graph(
    repo: &DynamoUserRepo,
    trip_id: &str,
    trip: &LoadedTripMeta,
    budget: &mut LedgerReadBudget,
) -> Result<Option<CurrentPlanGraph>, LedgerRepoError> {
    let (Some(plan_id), Some(version)) = (
        trip.value.current_plan_id.as_deref(),
        trip.value.current_plan_version,
    ) else {
        return Ok(None);
    };
    let pk = trip_pk(trip_id);
    let items = repo
        .ledger_query(
            &pk,
            &format!("{}#", plan_prefix(version)),
            MAX_LEDGER_ROWS,
            budget,
        )
        .await?;
    let mut plan = None::<(Plan, u64, String)>;
    let mut days = HashMap::<String, (Day, u64, String)>::new();
    let mut stops = Vec::<(Stop, u64, String)>::new();
    let mut stop_ids = HashSet::new();
    for item in items {
        let entity = string(&item, ENTITY_TYPE).map_err(record_error)?;
        let sk = string(&item, SK).map_err(record_error)?;
        match entity.as_str() {
            PLAN_ENTITY => {
                let stored: Stored<Plan> =
                    decode_record(&item, &pk, &sk, PLAN_ENTITY).map_err(record_error)?;
                if stored.revision == 0
                    || sk != plan_sk(version)
                    || stored.value.id != plan_id
                    || stored.value.trip_id != trip_id
                    || stored.value.version != version
                    || plan.replace((stored.value, stored.revision, sk)).is_some()
                {
                    return Err(LedgerRepoError::CorruptData);
                }
            }
            DAY_ENTITY => {
                let stored: Stored<Day> =
                    decode_record(&item, &pk, &sk, DAY_ENTITY).map_err(record_error)?;
                if stored.revision == 0
                    || stored.value.plan_id != plan_id
                    || sk != day_sk(version, &stored.value)
                    || days
                        .insert(stored.value.id.clone(), (stored.value, stored.revision, sk))
                        .is_some()
                {
                    return Err(LedgerRepoError::CorruptData);
                }
            }
            STOP_ENTITY => {
                let stored: Stored<Stop> =
                    decode_record(&item, &pk, &sk, STOP_ENTITY).map_err(record_error)?;
                if stored.revision == 0
                    || !stored.value.seq.is_finite()
                    || stored.value.seq <= 0.0
                    || stored.value.seq.fract() != 0.0
                    || !stop_ids.insert(stored.value.id.clone())
                {
                    return Err(LedgerRepoError::CorruptData);
                }
                stops.push((stored.value, stored.revision, sk));
            }
            _ => return Err(LedgerRepoError::CorruptData),
        }
    }
    let (plan, plan_revision, plan_sort_key) = plan.ok_or(LedgerRepoError::CorruptData)?;
    let canonical_days = days
        .values()
        .map(|(day, _, _)| day.clone())
        .collect::<Vec<_>>();
    let canonical_stops = stops
        .iter()
        .map(|(stop, _, _)| stop.clone())
        .collect::<Vec<_>>();
    if validate_stored_plan_graph(&plan, &canonical_days, &canonical_stops, trip_id, version)
        .is_err()
    {
        return Err(LedgerRepoError::CorruptData);
    }
    let mut loaded_stops = HashMap::new();
    for (stop, revision, sort_key) in stops {
        let day = days.get(&stop.day_id).ok_or(LedgerRepoError::CorruptData)?;
        if sort_key != stop_sk(version, &day.0, &stop)
            || loaded_stops
                .insert(
                    stop.id.clone(),
                    LoadedStop {
                        value: stop,
                        revision,
                        sort_key,
                    },
                )
                .is_some()
        {
            return Err(LedgerRepoError::CorruptData);
        }
    }
    Ok(Some(CurrentPlanGraph {
        plan_revision,
        plan_sort_key,
        stops: loaded_stops,
    }))
}

async fn prepare_stop_changes(
    repo: &DynamoUserRepo,
    trip_id: &str,
    trip: &LoadedTripMeta,
    old: Option<(&str, &str)>,
    new: Option<(&str, &str)>,
    budget: &mut LedgerReadBudget,
) -> Result<Vec<TransactWriteItem>, LedgerRepoError> {
    debug_assert!(old.map(|value| value.0) != new.map(|value| value.0));
    let graph = load_current_plan_graph(repo, trip_id, trip, budget).await?;
    if new.is_some() && graph.is_none() {
        return Err(LedgerRepoError::NotFound);
    }
    let mut actions = Vec::new();
    let mut touched_current_stop = false;
    if let (Some((stop_id, expense_id)), Some(graph)) = (old, graph.as_ref())
        && let Some(stop) = graph.stops.get(stop_id)
    {
        actions.push(stop_effect(repo, trip_id, stop, expense_id, false)?);
        touched_current_stop = true;
    }
    if let (Some((stop_id, expense_id)), Some(graph)) = (new, graph.as_ref()) {
        let stop = graph.stops.get(stop_id).ok_or(LedgerRepoError::NotFound)?;
        actions.push(stop_effect(repo, trip_id, stop, expense_id, true)?);
        touched_current_stop = true;
    }
    if touched_current_stop {
        let graph = graph.as_ref().ok_or(LedgerRepoError::CorruptData)?;
        actions.insert(
            0,
            condition_action(repo.entity_revision_condition(
                trip_pk(trip_id),
                graph.plan_sort_key.clone(),
                PLAN_ENTITY,
                graph.plan_revision,
            )),
        );
    }
    Ok(actions)
}

fn stop_effect(
    repo: &DynamoUserRepo,
    trip_id: &str,
    loaded: &LoadedStop,
    expense_id: &str,
    link: bool,
) -> Result<TransactWriteItem, LedgerRepoError> {
    let current = loaded
        .value
        .booking
        .as_ref()
        .and_then(|booking| booking.ledger_entry_id.as_deref());
    if link && current.is_some() {
        return Err(LedgerRepoError::CorruptData);
    }
    if !link && current.is_some_and(|current| current != expense_id) {
        return Err(LedgerRepoError::CorruptData);
    }
    let Some(booking) = loaded.value.booking.as_ref() else {
        return if link {
            // `ledgerEntryId` lives inside Booking. Creating a claim without a
            // booking-side pointer would immediately make the reverse-link
            // graph corrupt, and the ledger must not invent booking details.
            Err(LedgerRepoError::Conflict)
        } else {
            Err(LedgerRepoError::CorruptData)
        };
    };
    if (link && booking.ledger_entry_id.is_none())
        || (!link && booking.ledger_entry_id.as_deref() == Some(expense_id))
    {
        let mut updated = loaded.value.clone();
        updated
            .booking
            .as_mut()
            .ok_or(LedgerRepoError::CorruptData)?
            .ledger_entry_id = link.then(|| expense_id.to_string());
        let next_revision = loaded
            .revision
            .checked_add(1)
            .ok_or(LedgerRepoError::CorruptData)?;
        let item = encode_record(
            trip_pk(trip_id),
            loaded.sort_key.clone(),
            STOP_ENTITY,
            &updated,
            next_revision,
        )
        .map_err(record_error)?;
        Ok(put_action(repo.revision_put(item, loaded.revision)))
    } else {
        Ok(condition_action(repo.entity_revision_condition(
            trip_pk(trip_id),
            loaded.sort_key.clone(),
            STOP_ENTITY,
            loaded.revision,
        )))
    }
}

fn mutation_prelude(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    referenced_members: &HashSet<String>,
    trip: &LoadedTripMeta,
) -> Vec<TransactWriteItem> {
    let mut actions = vec![condition_action(repo.ledger_membership_condition(
        trip_id,
        actor,
        RequiredLedgerRole::Editor,
    ))];
    let mut members = referenced_members
        .iter()
        .filter(|user_id| user_id.as_str() != actor.0)
        .cloned()
        .collect::<Vec<_>>();
    members.sort();
    actions.extend(members.into_iter().map(|user_id| {
        condition_action(repo.ledger_membership_condition(
            trip_id,
            &UserId(user_id),
            RequiredLedgerRole::Any,
        ))
    }));
    actions.push(condition_action(repo.ledger_trip_condition(trip_id, trip)));
    actions
}

fn expense_member_ids(expense: &Expense) -> HashSet<String> {
    let mut ids = expense_participant_ids(&expense.split)
        .into_iter()
        .map(str::to_string)
        .collect::<HashSet<_>>();
    ids.insert(expense.paid_by.clone());
    ids
}

fn context(trip: &LoadedTripMeta) -> LedgerTripContext {
    LedgerTripContext {
        base_currency: trip.value.base_currency.clone(),
        trip_revision: trip.revision,
    }
}

fn validate_context(
    expected: &LedgerTripContext,
    actual: &LoadedTripMeta,
) -> Result<(), LedgerRepoError> {
    if expected.base_currency == actual.value.base_currency
        && expected.trip_revision == actual.revision
    {
        Ok(())
    } else {
        Err(LedgerRepoError::Conflict)
    }
}

fn validate_replacement(before: &Expense, after: &Expense) -> Result<(), LedgerRepoError> {
    if before.id != after.id
        || before.trip_id != after.trip_id
        || before.created_at != after.created_at
        || before.receipt_photo_url != after.receipt_photo_url
        || (before.currency == after.currency && before.fx_rate_to_base != after.fx_rate_to_base)
    {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(())
}

fn link_delta(old: Option<&str>, new: &Expense) -> i32 {
    match (old, new.linked_stop_id.as_deref()) {
        (None, Some(_)) => 1,
        (Some(_), None) => -1,
        _ => 0,
    }
}

fn next_meta(
    state: &LedgerState,
    trip_id: &str,
    expense_delta: i32,
    settlement_delta: i32,
    link_delta: i32,
    audit: Option<&LedgerAuditRecord>,
    operation_delta: u32,
) -> Result<LedgerMetaRecord, LedgerRepoError> {
    let current = state
        .meta
        .as_ref()
        .map(|meta| meta.value.clone())
        .unwrap_or(LedgerMetaRecord {
            trip_id: trip_id.to_string(),
            expense_count: 0,
            settlement_count: 0,
            stop_link_count: 0,
            audit_count: 0,
            operation_count: 0,
            audit_head_id: None,
            audit_head_at: None,
        });
    let expense_count = apply_count(current.expense_count, expense_delta)?;
    let settlement_count = apply_count(current.settlement_count, settlement_delta)?;
    let stop_link_count = apply_count(current.stop_link_count, link_delta)?;
    let audit_count = current
        .audit_count
        .checked_add(if audit.is_some() { 1 } else { 0 })
        .ok_or(LedgerRepoError::SafetyLimitExceeded)?;
    let operation_count = current
        .operation_count
        .checked_add(operation_delta)
        .ok_or(LedgerRepoError::SafetyLimitExceeded)?;
    if expense_count as usize > MAX_LEDGER_ROWS
        || settlement_count as usize > MAX_LEDGER_ROWS
        || stop_link_count > expense_count
        || audit_count > MAX_LEDGER_AUDITS as u64
        || operation_count as usize > MAX_LEDGER_OPERATIONS
    {
        return Err(LedgerRepoError::SafetyLimitExceeded);
    }
    Ok(LedgerMetaRecord {
        trip_id: trip_id.to_string(),
        expense_count,
        settlement_count,
        stop_link_count,
        audit_count,
        operation_count,
        audit_head_id: audit
            .map(|value| value.id.clone())
            .or(current.audit_head_id),
        audit_head_at: audit
            .map(|value| value.created_at.clone())
            .or(current.audit_head_at),
    })
}

fn apply_count(value: u32, delta: i32) -> Result<u32, LedgerRepoError> {
    if delta >= 0 {
        value
            .checked_add(delta as u32)
            .ok_or(LedgerRepoError::SafetyLimitExceeded)
    } else {
        value
            .checked_sub(delta.unsigned_abs())
            .ok_or(LedgerRepoError::CorruptData)
    }
}

fn meta_put(
    repo: &DynamoUserRepo,
    state: &LedgerState,
    next: LedgerMetaRecord,
) -> Result<aws_sdk_dynamodb::types::Put, LedgerRepoError> {
    match &state.meta {
        Some(meta) => {
            let next_revision = meta
                .revision
                .checked_add(1)
                .ok_or(LedgerRepoError::CorruptData)?;
            Ok(repo.snapshot_put(
                encode_ledger_meta(&next, next_revision)?,
                meta.revision,
                &meta.raw_data,
            ))
        }
        None => Ok(repo.create_only_put(encode_ledger_meta(&next, 1)?)),
    }
}

fn validate_next_audit(
    state: &LedgerState,
    audit: &LedgerAuditRecord,
) -> Result<(), LedgerRepoError> {
    if state.audits.len() >= MAX_LEDGER_AUDITS
        || state
            .audits
            .iter()
            .any(|stored| stored.value.id == audit.id)
    {
        return Err(LedgerRepoError::SafetyLimitExceeded);
    }
    if let Some(meta) = &state.meta {
        let head_at = meta
            .value
            .audit_head_at
            .as_deref()
            .ok_or(LedgerRepoError::CorruptData)?;
        if utc(&audit.created_at)? < utc(head_at)? {
            return Err(LedgerRepoError::Conflict);
        }
    }
    if audit.previous_audit_id != audit_predecessor(state) {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(())
}

fn audit_predecessor(state: &LedgerState) -> Option<String> {
    state
        .meta
        .as_ref()
        .and_then(|meta| meta.value.audit_head_id.clone())
}

fn ledger_operation(
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
    result: LedgerAuditValue,
    created_at: &str,
) -> Result<LedgerOperationRecord, LedgerRepoError> {
    if !valid_operation_input(idempotency_key, request_hash) {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(LedgerOperationRecord {
        trip_id: trip_id.to_string(),
        key_hash: operation_key_hash(idempotency_key),
        actor_id: actor.0.clone(),
        request_hash: request_hash.to_string(),
        result,
        created_at: created_at.to_string(),
    })
}

fn replay_operation(
    state: &LedgerState,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<Option<LedgerAuditValue>, LedgerRepoError> {
    if !valid_operation_input(idempotency_key, request_hash) {
        return Err(LedgerRepoError::CorruptData);
    }
    let key_hash = operation_key_hash(idempotency_key);
    let Some(operation) = state.operations.get(&key_hash) else {
        return Ok(None);
    };
    if operation.value.actor_id != actor.0 || operation.value.request_hash != request_hash {
        return Err(LedgerRepoError::Conflict);
    }
    Ok(Some(operation.value.result.clone()))
}

fn valid_operation_input(idempotency_key: &str, request_hash: &str) -> bool {
    !idempotency_key.is_empty()
        && idempotency_key.len() <= 128
        && idempotency_key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
        && request_hash.len() == 64
        && request_hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn expense_audit(
    audit_id: &str,
    actor: &UserId,
    action: LedgerAuditAction,
    before: Option<Expense>,
    after: Option<Expense>,
    previous_audit_id: Option<String>,
    created_at: &str,
) -> Result<LedgerAuditRecord, LedgerRepoError> {
    let source = before
        .as_ref()
        .or(after.as_ref())
        .ok_or(LedgerRepoError::CorruptData)?;
    let entity_id = source.id.clone();
    let trip_id = source.trip_id.clone();
    Ok(LedgerAuditRecord {
        id: audit_id.to_string(),
        trip_id,
        actor_id: actor.0.clone(),
        action,
        entity_id,
        before: before.map(LedgerAuditValue::Expense),
        after: after.map(LedgerAuditValue::Expense),
        previous_audit_id,
        created_at: created_at.to_string(),
    })
}

fn settlement_audit(
    new: &NewSettlement,
    trip_id: &str,
    actor: &UserId,
    previous_audit_id: Option<String>,
) -> LedgerAuditRecord {
    LedgerAuditRecord {
        id: new.audit_id.clone(),
        trip_id: trip_id.to_string(),
        actor_id: actor.0.clone(),
        action: LedgerAuditAction::SettlementCreated,
        entity_id: new.settlement.id.clone(),
        before: None,
        after: Some(LedgerAuditValue::Settlement(new.settlement.clone())),
        previous_audit_id,
        created_at: new.audit_at.clone(),
    }
}

fn guarded_delete(
    repo: &DynamoUserRepo,
    partition_key: String,
    sort_key: String,
    entity: &str,
    revision: u64,
) -> Delete {
    Delete::builder()
        .table_name(&repo.table_name)
        .set_key(Some(item_key(partition_key, sort_key)))
        .condition_expression("#entity = :entity AND #revision = :revision")
        .expression_attribute_names("#entity", ENTITY_TYPE)
        .expression_attribute_names("#revision", REVISION)
        .expression_attribute_values(":entity", AttributeValue::S(entity.into()))
        .expression_attribute_values(":revision", AttributeValue::N(revision.to_string()))
        .build()
        .expect("ledger delete is complete")
}

fn enforce_transaction_limit(actions: &[TransactWriteItem]) -> Result<(), LedgerRepoError> {
    if actions.len() > 100 {
        Err(LedgerRepoError::SafetyLimitExceeded)
    } else {
        Ok(())
    }
}

fn enforce_response_limit<T: serde::Serialize>(value: T) -> Result<(), LedgerRepoError> {
    let bytes = serde_json::to_vec(&value)
        .map_err(|_| LedgerRepoError::CorruptData)?
        .len();
    if bytes > MAX_LEDGER_BYTES {
        Err(LedgerRepoError::SafetyLimitExceeded)
    } else {
        Ok(())
    }
}

fn utc(value: &str) -> Result<DateTime<chrono::FixedOffset>, LedgerRepoError> {
    if value.len() > 64 || !value.ends_with('Z') {
        return Err(LedgerRepoError::CorruptData);
    }
    let timestamp =
        DateTime::parse_from_rfc3339(value).map_err(|_| LedgerRepoError::CorruptData)?;
    if timestamp.offset().local_minus_utc() != 0 {
        return Err(LedgerRepoError::CorruptData);
    }
    Ok(timestamp)
}

async fn committed_expense_and_audit(
    repo: &DynamoUserRepo,
    trip_id: &str,
    _expected: &Expense,
    audit: &LedgerAuditRecord,
) -> Result<bool, LedgerRepoError> {
    committed_audit(repo, trip_id, audit).await
}

async fn replay_expense_operation(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<Option<Expense>, LedgerRepoError> {
    let trip = repo.ledger_trip_meta(trip_id).await?;
    let (state, _) = load_complete_ledger_state(repo, trip_id, &trip).await?;
    match replay_operation(&state, actor, idempotency_key, request_hash)? {
        None => Ok(None),
        Some(LedgerAuditValue::Expense(expense)) => Ok(Some(expense)),
        Some(LedgerAuditValue::Settlement(_)) => Err(LedgerRepoError::Conflict),
    }
}

async fn replay_settlement_operation(
    repo: &DynamoUserRepo,
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    request_hash: &str,
) -> Result<Option<Settlement>, LedgerRepoError> {
    let trip = repo.ledger_trip_meta(trip_id).await?;
    let (state, _) = load_complete_ledger_state(repo, trip_id, &trip).await?;
    match replay_operation(&state, actor, idempotency_key, request_hash)? {
        None => Ok(None),
        Some(LedgerAuditValue::Settlement(settlement)) => Ok(Some(settlement)),
        Some(LedgerAuditValue::Expense(_)) => Err(LedgerRepoError::Conflict),
    }
}

async fn committed_audit(
    repo: &DynamoUserRepo,
    trip_id: &str,
    expected: &LedgerAuditRecord,
) -> Result<bool, LedgerRepoError> {
    let trip = repo.ledger_trip_meta(trip_id).await?;
    let (state, _) = load_complete_ledger_state(repo, trip_id, &trip).await?;
    Ok(state
        .audits
        .iter()
        .any(|audit| audit.value == *expected && audit.revision == 1))
}
