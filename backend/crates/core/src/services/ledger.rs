use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::DateTime;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    domain::{
        ledger::{
            Expense, ExpenseCategory, ExpenseSplit, LedgerBalance, LedgerView, Settlement,
            SuggestedTransfer,
        },
        user::UserId,
    },
    ports::{
        clock::Clock,
        fx_rate::{FxRateError, FxRateProvider},
        id_gen::IdGen,
        ledger::{ExpenseReplacement, LedgerRepo, LedgerRepoError, NewExpense, NewSettlement},
    },
};

use super::validation::{ValidationError, http_url, text_len};

pub const MAX_LEDGER_ROWS: usize = 1_000;
pub const MAX_LEDGER_PEOPLE: usize = 1_000;
pub const MAX_LEDGER_RESPONSE_BYTES: usize = 4 * 1_024 * 1_024;
pub const MAX_SPLIT_PARTICIPANTS: usize = 50;
pub const MAX_EXPENSE_NOTE_CHARS: usize = 10_000;
pub const MAX_MONEY_AMOUNT: f64 = 1_000_000_000.0;
pub const MAX_FX_RATE: f64 = 1_000_000.0;
const EXACT_SPLIT_EPSILON: f64 = 0.000_001;
const MAX_IDEMPOTENCY_KEY_BYTES: usize = 128;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddExpenseInput {
    pub paid_by: String,
    pub amount: f64,
    pub currency: String,
    pub category: ExpenseCategory,
    pub split: ExpenseSplit,
    pub note: String,
    pub linked_stop_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExpensePatch {
    pub paid_by: Option<String>,
    pub amount: Option<f64>,
    pub currency: Option<String>,
    pub category: Option<ExpenseCategory>,
    pub split: Option<ExpenseSplit>,
    pub note: Option<String>,
    pub linked_stop_id: Option<Option<String>>,
}

impl ExpensePatch {
    fn is_empty(&self) -> bool {
        self.paid_by.is_none()
            && self.amount.is_none()
            && self.currency.is_none()
            && self.category.is_none()
            && self.split.is_none()
            && self.note.is_none()
            && self.linked_stop_id.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddSettlementInput {
    pub from_user: String,
    pub to_user: String,
    pub amount: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct LedgerRequestContext<'a> {
    pub trip_id: &'a str,
    pub actor: &'a UserId,
}

#[derive(Debug, thiserror::Error)]
pub enum LedgerServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] LedgerRepoError),
    #[error(transparent)]
    FxRate(#[from] FxRateError),
    #[error("ledger data cannot be computed safely")]
    CorruptData,
}

pub async fn get_ledger(
    repo: &dyn LedgerRepo,
    trip_id: &str,
    actor: &UserId,
) -> Result<LedgerView, LedgerServiceError> {
    validate_id(trip_id, "trip id is invalid")?;
    let data = repo.get_ledger_data(trip_id, actor).await?;
    compute_ledger(
        trip_id,
        &data.base_currency,
        data.current_member_ids,
        data.expenses,
        data.settlements,
    )
}

pub async fn add_expense(
    repo: &dyn LedgerRepo,
    rates: &dyn FxRateProvider,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    request: LedgerRequestContext<'_>,
    idempotency_key: &str,
    input: AddExpenseInput,
) -> Result<Expense, LedgerServiceError> {
    let trip_id = request.trip_id;
    let actor = request.actor;
    validate_id(trip_id, "trip id is invalid")?;
    validate_idempotency_key(idempotency_key)?;
    validate_add_expense_input(&input)?;
    let request_hash = expense_creation_request_hash(&input)?;
    if let Some(expense) = repo
        .replay_expense_creation(trip_id, actor, idempotency_key, &request_hash)
        .await?
    {
        validate_stored_expense(trip_id, &expense)?;
        return Ok(expense);
    }
    let context = repo.get_trip_context(trip_id, actor).await?;
    validate_currency(&context.base_currency)?;
    let fx_rate_to_base = rate_to_base(rates, &input.currency, &context.base_currency).await?;
    let created_at = clock.now();
    let expense = Expense {
        id: ids.new_id(),
        trip_id: trip_id.to_string(),
        paid_by: input.paid_by,
        amount: input.amount,
        currency: input.currency,
        fx_rate_to_base,
        category: input.category,
        split: input.split,
        note: input.note,
        receipt_photo_url: None,
        linked_stop_id: input.linked_stop_id,
        created_at: created_at.clone(),
    };
    validate_stored_expense(trip_id, &expense)?;
    repo.add_expense(
        trip_id,
        actor,
        NewExpense {
            expense,
            context,
            idempotency_key: idempotency_key.to_string(),
            request_hash,
            audit_id: ids.new_id(),
            audit_at: created_at,
        },
    )
    .await
    .map_err(Into::into)
}

pub async fn update_expense(
    repo: &dyn LedgerRepo,
    rates: &dyn FxRateProvider,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    request: LedgerRequestContext<'_>,
    expense_id: &str,
    patch: ExpensePatch,
) -> Result<Expense, LedgerServiceError> {
    let trip_id = request.trip_id;
    let actor = request.actor;
    validate_id(trip_id, "trip id is invalid")?;
    validate_id(expense_id, "expense id is invalid")?;
    if patch.is_empty() {
        return Err(ValidationError("expense patch must contain at least one field").into());
    }
    validate_patch_fields(&patch)?;
    let current = repo.get_expense(trip_id, actor, expense_id).await?;
    validate_stored_expense(trip_id, &current.expense)?;
    validate_currency(&current.context.base_currency)?;

    let original = current.expense;
    let mut expense = original.clone();
    if let Some(value) = patch.paid_by {
        expense.paid_by = value;
    }
    if let Some(value) = patch.amount {
        expense.amount = value;
    }
    if let Some(value) = patch.category {
        expense.category = value;
    }
    if let Some(value) = patch.split {
        expense.split = value;
    }
    if let Some(value) = patch.note {
        expense.note = value;
    }
    if let Some(value) = patch.linked_stop_id {
        expense.linked_stop_id = value;
    }
    if let Some(currency) = patch.currency
        && currency != expense.currency
    {
        expense.fx_rate_to_base =
            rate_to_base(rates, &currency, &current.context.base_currency).await?;
        expense.currency = currency;
    }
    validate_stored_expense(trip_id, &expense)?;
    if expense == original {
        return Ok(expense);
    }
    let audit_at = clock.now();
    repo.replace_expense(
        trip_id,
        actor,
        ExpenseReplacement {
            expense,
            expected_revision: current.revision,
            context: current.context,
            audit_id: ids.new_id(),
            audit_at,
        },
    )
    .await
    .map_err(Into::into)
}

pub async fn delete_expense(
    repo: &dyn LedgerRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    expense_id: &str,
) -> Result<(), LedgerServiceError> {
    validate_id(trip_id, "trip id is invalid")?;
    validate_id(expense_id, "expense id is invalid")?;
    repo.delete_expense(trip_id, actor, expense_id, &ids.new_id(), &clock.now())
        .await
        .map_err(Into::into)
}

pub async fn add_settlement(
    repo: &dyn LedgerRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    trip_id: &str,
    actor: &UserId,
    idempotency_key: &str,
    input: AddSettlementInput,
) -> Result<Settlement, LedgerServiceError> {
    validate_id(trip_id, "trip id is invalid")?;
    validate_idempotency_key(idempotency_key)?;
    validate_id(&input.from_user, "fromUser is invalid")?;
    validate_id(&input.to_user, "toUser is invalid")?;
    if input.from_user == input.to_user {
        return Err(ValidationError("a settlement must be between two different members").into());
    }
    validate_amount(input.amount, "settlement amount is invalid")?;
    let request_hash = settlement_creation_request_hash(&input)?;
    if let Some(settlement) = repo
        .replay_settlement_creation(trip_id, actor, idempotency_key, &request_hash)
        .await?
    {
        validate_stored_settlement(trip_id, &settlement)?;
        return Ok(settlement);
    }
    let settled_at = clock.now();
    let settlement = Settlement {
        id: ids.new_id(),
        trip_id: trip_id.to_string(),
        from_user: input.from_user,
        to_user: input.to_user,
        amount: input.amount,
        settled_at: settled_at.clone(),
    };
    validate_stored_settlement(trip_id, &settlement)?;
    repo.add_settlement(
        trip_id,
        actor,
        NewSettlement {
            settlement,
            idempotency_key: idempotency_key.to_string(),
            request_hash,
            audit_id: ids.new_id(),
            audit_at: settled_at,
        },
    )
    .await
    .map_err(Into::into)
}

async fn rate_to_base(
    rates: &dyn FxRateProvider,
    currency: &str,
    base_currency: &str,
) -> Result<f64, LedgerServiceError> {
    let rate = if currency == base_currency {
        1.0
    } else {
        rates.rate_to_base(currency, base_currency).await?
    };
    if !rate.is_finite() || rate <= 0.0 || rate > MAX_FX_RATE {
        return Err(FxRateError::InvalidResponse.into());
    }
    Ok(rate)
}

fn validate_add_expense_input(input: &AddExpenseInput) -> Result<(), ValidationError> {
    validate_id(&input.paid_by, "paidBy is invalid")?;
    validate_amount(input.amount, "expense amount is invalid")?;
    validate_currency(&input.currency)?;
    validate_split(&input.split, input.amount)?;
    text_len(&input.note, MAX_EXPENSE_NOTE_CHARS)?;
    if let Some(stop_id) = &input.linked_stop_id {
        validate_id(stop_id, "linkedStopId is invalid")?;
    }
    Ok(())
}

fn validate_patch_fields(patch: &ExpensePatch) -> Result<(), ValidationError> {
    if let Some(value) = &patch.paid_by {
        validate_id(value, "paidBy is invalid")?;
    }
    if let Some(value) = patch.amount {
        validate_amount(value, "expense amount is invalid")?;
    }
    if let Some(value) = &patch.currency {
        validate_currency(value)?;
    }
    if let Some(value) = &patch.note {
        text_len(value, MAX_EXPENSE_NOTE_CHARS)?;
    }
    if let Some(Some(value)) = &patch.linked_stop_id {
        validate_id(value, "linkedStopId is invalid")?;
    }
    if let Some(split) = &patch.split {
        validate_split_shape(split)?;
    }
    Ok(())
}

pub fn validate_stored_expense(
    expected_trip_id: &str,
    expense: &Expense,
) -> Result<(), ValidationError> {
    validate_id(&expense.id, "expense id is invalid")?;
    if expense.trip_id != expected_trip_id {
        return Err(ValidationError("expense trip is invalid"));
    }
    validate_id(&expense.paid_by, "expense payer is invalid")?;
    validate_amount(expense.amount, "expense amount is invalid")?;
    validate_currency(&expense.currency)?;
    if !expense.fx_rate_to_base.is_finite()
        || expense.fx_rate_to_base <= 0.0
        || expense.fx_rate_to_base > MAX_FX_RATE
    {
        return Err(ValidationError("expense FX rate is invalid"));
    }
    validate_split(&expense.split, expense.amount)?;
    text_len(&expense.note, MAX_EXPENSE_NOTE_CHARS)?;
    if let Some(url) = &expense.receipt_photo_url
        && http_url(Some(url.clone()))?.as_deref() != Some(url.as_str())
    {
        return Err(ValidationError("receipt photo URL is invalid"));
    }
    if let Some(stop_id) = &expense.linked_stop_id {
        validate_id(stop_id, "linked stop id is invalid")?;
    }
    validate_utc(&expense.created_at, "expense timestamp is invalid")?;
    Ok(())
}

pub fn validate_stored_settlement(
    expected_trip_id: &str,
    settlement: &Settlement,
) -> Result<(), ValidationError> {
    validate_id(&settlement.id, "settlement id is invalid")?;
    if settlement.trip_id != expected_trip_id {
        return Err(ValidationError("settlement trip is invalid"));
    }
    validate_id(&settlement.from_user, "settlement sender is invalid")?;
    validate_id(&settlement.to_user, "settlement recipient is invalid")?;
    if settlement.from_user == settlement.to_user {
        return Err(ValidationError("settlement members must be different"));
    }
    validate_amount(settlement.amount, "settlement amount is invalid")?;
    validate_utc(&settlement.settled_at, "settlement timestamp is invalid")
}

pub fn expense_participant_ids(split: &ExpenseSplit) -> Vec<&str> {
    match split {
        ExpenseSplit::Even { participant_ids } => {
            participant_ids.iter().map(String::as_str).collect()
        }
        ExpenseSplit::Shares { participants } => participants
            .iter()
            .map(|participant| participant.user_id.as_str())
            .collect(),
        ExpenseSplit::Exact { participants } => participants
            .iter()
            .map(|participant| participant.user_id.as_str())
            .collect(),
    }
}

fn validate_split(split: &ExpenseSplit, amount: f64) -> Result<(), ValidationError> {
    validate_split_shape(split)?;
    if let ExpenseSplit::Exact { participants } = split {
        let total = participants
            .iter()
            .try_fold(0.0, |sum, participant| bounded_add(sum, participant.amount))
            .ok_or(ValidationError("exact split total is invalid"))?;
        let tolerance = EXACT_SPLIT_EPSILON.max(amount.abs() * 1e-12);
        if (total - amount).abs() > tolerance {
            return Err(ValidationError(
                "exact split amounts must equal the expense amount",
            ));
        }
    }
    Ok(())
}

fn validate_split_shape(split: &ExpenseSplit) -> Result<(), ValidationError> {
    let ids = expense_participant_ids(split);
    if ids.is_empty() || ids.len() > MAX_SPLIT_PARTICIPANTS {
        return Err(ValidationError(
            "an expense needs between 1 and 50 participants",
        ));
    }
    let mut unique = HashSet::new();
    for id in &ids {
        validate_id(id, "expense participant id is invalid")?;
        if !unique.insert(*id) {
            return Err(ValidationError("expense participants must be unique"));
        }
    }
    match split {
        ExpenseSplit::Even { .. } => {}
        ExpenseSplit::Shares { participants } => {
            if participants.iter().any(|participant| {
                !participant.weight.is_finite()
                    || participant.weight <= 0.0
                    || participant.weight > MAX_MONEY_AMOUNT
            }) {
                return Err(ValidationError("expense share weights are invalid"));
            }
            if participants
                .iter()
                .try_fold(0.0, |sum, participant| bounded_add(sum, participant.weight))
                .is_none()
            {
                return Err(ValidationError("expense share weights are invalid"));
            }
        }
        ExpenseSplit::Exact { participants } => {
            if participants.iter().any(|participant| {
                !participant.amount.is_finite()
                    || participant.amount < 0.0
                    || participant.amount > MAX_MONEY_AMOUNT
            }) {
                return Err(ValidationError("exact split amounts are invalid"));
            }
        }
    }
    Ok(())
}

pub fn validate_currency(value: &str) -> Result<(), ValidationError> {
    if value.len() == 3 && value.bytes().all(|byte| byte.is_ascii_uppercase()) {
        Ok(())
    } else {
        Err(ValidationError(
            "currency must be a three-letter uppercase ISO 4217 code",
        ))
    }
}

fn validate_amount(value: f64, error: &'static str) -> Result<(), ValidationError> {
    if value.is_finite() && value > 0.0 && value <= MAX_MONEY_AMOUNT {
        Ok(())
    } else {
        Err(ValidationError(error))
    }
}

fn validate_id(value: &str, error: &'static str) -> Result<(), ValidationError> {
    if value.is_empty() || value.trim() != value || value.chars().count() > 200 {
        Err(ValidationError(error))
    } else {
        Ok(())
    }
}

fn validate_idempotency_key(value: &str) -> Result<(), ValidationError> {
    if value.is_empty()
        || value.len() > MAX_IDEMPOTENCY_KEY_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        Err(ValidationError(
            "Idempotency-Key must contain 1 to 128 safe ASCII characters",
        ))
    } else {
        Ok(())
    }
}

pub fn expense_creation_request_hash(
    input: &AddExpenseInput,
) -> Result<String, LedgerServiceError> {
    create_request_hash("expense", input)
}

pub fn settlement_creation_request_hash(
    input: &AddSettlementInput,
) -> Result<String, LedgerServiceError> {
    create_request_hash("settlement", input)
}

fn create_request_hash<T: Serialize>(kind: &str, input: &T) -> Result<String, LedgerServiceError> {
    let canonical = serde_json::to_vec(input).map_err(|_| LedgerServiceError::CorruptData)?;
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    digest.update([0]);
    digest.update(canonical);
    let digest = digest.finalize();
    Ok(format!("{digest:x}"))
}

fn validate_utc(value: &str, error: &'static str) -> Result<(), ValidationError> {
    let timestamp = DateTime::parse_from_rfc3339(value).map_err(|_| ValidationError(error))?;
    if value.len() <= 64 && value.ends_with('Z') && timestamp.offset().local_minus_utc() == 0 {
        Ok(())
    } else {
        Err(ValidationError(error))
    }
}

fn bounded_add(left: f64, right: f64) -> Option<f64> {
    let result = left + right;
    result.is_finite().then_some(result)
}

fn bounded_multiply(left: f64, right: f64) -> Option<f64> {
    let result = left * right;
    result.is_finite().then_some(result)
}

pub fn compute_ledger(
    expected_trip_id: &str,
    base_currency: &str,
    current_member_ids: Vec<String>,
    expenses: Vec<Expense>,
    settlements: Vec<Settlement>,
) -> Result<LedgerView, LedgerServiceError> {
    validate_currency(base_currency)?;
    if expenses.len() > MAX_LEDGER_ROWS || settlements.len() > MAX_LEDGER_ROWS {
        return Err(LedgerRepoError::SafetyLimitExceeded.into());
    }
    let mut people = BTreeSet::new();
    for member_id in current_member_ids {
        validate_id(&member_id, "member id is invalid")?;
        if !people.insert(member_id) {
            return Err(LedgerServiceError::CorruptData);
        }
    }
    for expense in &expenses {
        validate_stored_expense(expected_trip_id, expense)?;
        people.insert(expense.paid_by.clone());
        people.extend(
            expense_participant_ids(&expense.split)
                .into_iter()
                .map(str::to_string),
        );
    }
    for settlement in &settlements {
        validate_stored_settlement(expected_trip_id, settlement)?;
        people.insert(settlement.from_user.clone());
        people.insert(settlement.to_user.clone());
    }
    if people.len() > MAX_LEDGER_PEOPLE {
        return Err(LedgerRepoError::SafetyLimitExceeded.into());
    }

    let mut paid = people
        .iter()
        .map(|id| (id.clone(), 0.0))
        .collect::<BTreeMap<_, _>>();
    let mut owed = paid.clone();
    let mut settled = paid.clone();
    for expense in &expenses {
        let in_base = bounded_multiply(expense.amount, expense.fx_rate_to_base)
            .ok_or(LedgerServiceError::CorruptData)?;
        add_to(&mut paid, &expense.paid_by, in_base)?;
        match &expense.split {
            ExpenseSplit::Even { participant_ids } => {
                let each = in_base / participant_ids.len() as f64;
                for user_id in participant_ids {
                    add_to(&mut owed, user_id, each)?;
                }
            }
            ExpenseSplit::Shares { participants } => {
                let total_weight = participants
                    .iter()
                    .try_fold(0.0, |sum, participant| bounded_add(sum, participant.weight));
                let total_weight = total_weight.ok_or(LedgerServiceError::CorruptData)?;
                for participant in participants {
                    add_to(
                        &mut owed,
                        &participant.user_id,
                        in_base * participant.weight / total_weight,
                    )?;
                }
            }
            ExpenseSplit::Exact { participants } => {
                for participant in participants {
                    let share = bounded_multiply(participant.amount, expense.fx_rate_to_base)
                        .ok_or(LedgerServiceError::CorruptData)?;
                    add_to(&mut owed, &participant.user_id, share)?;
                }
            }
        }
    }
    for settlement in &settlements {
        add_to(&mut settled, &settlement.from_user, settlement.amount)?;
        add_to(&mut settled, &settlement.to_user, -settlement.amount)?;
    }

    let balances = people
        .into_iter()
        .map(|user_id| {
            let paid = round2(*paid.get(&user_id).unwrap_or(&0.0));
            let owed = round2(*owed.get(&user_id).unwrap_or(&0.0));
            let net = round2(paid - owed + settled.get(&user_id).copied().unwrap_or(0.0));
            if paid.is_finite() && owed.is_finite() && net.is_finite() {
                Ok(LedgerBalance {
                    user_id,
                    paid,
                    owed,
                    net,
                })
            } else {
                Err(LedgerServiceError::CorruptData)
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let suggested_transfers = simplify_debts(&balances)?;
    let view = LedgerView {
        expenses,
        settlements,
        balances,
        suggested_transfers,
    };
    let response_bytes = serde_json::to_vec(&view)
        .map_err(|_| LedgerServiceError::CorruptData)?
        .len();
    if response_bytes > MAX_LEDGER_RESPONSE_BYTES {
        return Err(LedgerRepoError::SafetyLimitExceeded.into());
    }
    Ok(view)
}

fn add_to(
    values: &mut BTreeMap<String, f64>,
    user_id: &str,
    amount: f64,
) -> Result<(), LedgerServiceError> {
    let current = values
        .get_mut(user_id)
        .ok_or(LedgerServiceError::CorruptData)?;
    *current = bounded_add(*current, amount).ok_or(LedgerServiceError::CorruptData)?;
    Ok(())
}

fn round2(value: f64) -> f64 {
    round_half_away_from_zero(value * 100.0) / 100.0
}

fn round_half_away_from_zero(value: f64) -> f64 {
    value.abs().round().copysign(value)
}

fn simplify_debts(
    balances: &[LedgerBalance],
) -> Result<Vec<SuggestedTransfer>, LedgerServiceError> {
    let mut rounded = balances
        .iter()
        .map(|balance| {
            let value = round_half_away_from_zero(balance.net);
            if !value.is_finite() || value.abs() > i64::MAX as f64 / 2.0 {
                Err(LedgerServiceError::CorruptData)
            } else {
                Ok((balance.user_id.clone(), value as i64))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let residual = rounded
        .iter()
        .try_fold(0_i64, |sum, (_, value)| sum.checked_add(*value))
        .ok_or(LedgerServiceError::CorruptData)?;
    if residual != 0 && !rounded.is_empty() {
        let index = rounded
            .iter()
            .enumerate()
            .max_by(|(_, left), (_, right)| {
                left.1
                    .unsigned_abs()
                    .cmp(&right.1.unsigned_abs())
                    .then_with(|| right.0.cmp(&left.0))
            })
            .map(|(index, _)| index)
            .ok_or(LedgerServiceError::CorruptData)?;
        rounded[index].1 = rounded[index]
            .1
            .checked_sub(residual)
            .ok_or(LedgerServiceError::CorruptData)?;
    }

    let mut creditors = rounded
        .iter()
        .filter(|(_, value)| *value > 0)
        .cloned()
        .collect::<Vec<_>>();
    let mut debtors = rounded
        .iter()
        .filter(|(_, value)| *value < 0)
        .cloned()
        .collect::<Vec<_>>();
    creditors.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    debtors.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    let mut transfers = Vec::new();
    let (mut creditor_index, mut debtor_index) = (0, 0);
    while creditor_index < creditors.len() && debtor_index < debtors.len() {
        let amount = creditors[creditor_index]
            .1
            .min(debtors[debtor_index].1.saturating_neg());
        if amount > 0 {
            transfers.push(SuggestedTransfer {
                from_user: debtors[debtor_index].0.clone(),
                to_user: creditors[creditor_index].0.clone(),
                amount: amount as f64,
            });
        }
        creditors[creditor_index].1 -= amount;
        debtors[debtor_index].1 += amount;
        if creditors[creditor_index].1 == 0 {
            creditor_index += 1;
        }
        if debtors[debtor_index].1 == 0 {
            debtor_index += 1;
        }
    }

    Ok(transfers)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use crate::domain::ledger::{ExactParticipant, ShareParticipant};
    use crate::ports::fx_rate::{FxRateError, FxRateProvider};

    use super::*;

    fn expense(split: ExpenseSplit) -> Expense {
        Expense {
            id: "expense-a".into(),
            trip_id: "trip-a".into(),
            paid_by: "user-a".into(),
            amount: 100.0,
            currency: "EUR".into(),
            fx_rate_to_base: 2.0,
            category: ExpenseCategory::Food,
            split,
            note: "Dinner".into(),
            receipt_photo_url: None,
            linked_stop_id: None,
            created_at: "2026-08-06T10:00:00Z".into(),
        }
    }

    #[test]
    fn stored_expenses_reject_duplicate_members_bad_exact_totals_and_non_utc_times() {
        let mut value = expense(ExpenseSplit::Even {
            participant_ids: vec!["user-a".into(), "user-b".into()],
        });
        assert!(validate_stored_expense("trip-a", &value).is_ok());
        value.split = ExpenseSplit::Even {
            participant_ids: vec!["user-a".into(), "user-a".into()],
        };
        assert!(validate_stored_expense("trip-a", &value).is_err());
        value.split = ExpenseSplit::Exact {
            participants: vec![ExactParticipant {
                user_id: "user-a".into(),
                amount: 99.0,
            }],
        };
        assert!(validate_stored_expense("trip-a", &value).is_err());
        value.split = ExpenseSplit::Even {
            participant_ids: vec!["user-a".into()],
        };
        value.created_at = "2026-08-06T11:00:00+01:00".into();
        assert!(validate_stored_expense("trip-a", &value).is_err());
    }

    #[test]
    fn idempotency_keys_are_bounded_and_use_a_log_safe_alphabet() {
        assert!(validate_idempotency_key("retry_2026-08-06:1").is_ok());
        assert!(validate_idempotency_key("").is_err());
        assert!(validate_idempotency_key(&"x".repeat(MAX_IDEMPOTENCY_KEY_BYTES + 1)).is_err());
        assert!(validate_idempotency_key("contains a space").is_err());
        assert!(validate_idempotency_key("line\nbreak").is_err());
    }

    #[test]
    fn balances_keep_frozen_fx_include_former_members_and_apply_settlements() {
        let expense = expense(ExpenseSplit::Even {
            participant_ids: vec!["user-a".into(), "former-user".into()],
        });
        let settlement = Settlement {
            id: "settlement-a".into(),
            trip_id: "trip-a".into(),
            from_user: "former-user".into(),
            to_user: "user-a".into(),
            amount: 40.0,
            settled_at: "2026-08-06T11:00:00Z".into(),
        };
        let view = compute_ledger(
            "trip-a",
            "GBP",
            vec!["user-a".into()],
            vec![expense],
            vec![settlement],
        )
        .expect("ledger computes");
        assert_eq!(view.balances.len(), 2);
        assert_eq!(
            view.balances
                .iter()
                .find(|balance| balance.user_id == "former-user")
                .map(|balance| balance.net),
            Some(-60.0)
        );
        assert_eq!(
            view.suggested_transfers,
            vec![SuggestedTransfer {
                from_user: "former-user".into(),
                to_user: "user-a".into(),
                amount: 60.0,
            }]
        );
    }

    #[test]
    fn weighted_and_exact_splits_preserve_the_base_total() {
        let weighted = expense(ExpenseSplit::Shares {
            participants: vec![
                ShareParticipant {
                    user_id: "user-a".into(),
                    weight: 1.0,
                },
                ShareParticipant {
                    user_id: "user-b".into(),
                    weight: 3.0,
                },
            ],
        });
        let exact = expense(ExpenseSplit::Exact {
            participants: vec![
                ExactParticipant {
                    user_id: "user-a".into(),
                    amount: 25.0,
                },
                ExactParticipant {
                    user_id: "user-b".into(),
                    amount: 75.0,
                },
            ],
        });
        assert!(
            compute_ledger(
                "trip-a",
                "GBP",
                vec!["user-a".into(), "user-b".into()],
                vec![weighted, exact],
                vec![],
            )
            .is_ok()
        );
    }

    struct FixedRate {
        calls: AtomicUsize,
        value: Result<f64, FxRateError>,
    }

    #[async_trait]
    impl FxRateProvider for FixedRate {
        async fn rate_to_base(
            &self,
            _currency: &str,
            _base_currency: &str,
        ) -> Result<f64, FxRateError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.value
        }
    }

    #[tokio::test]
    async fn same_currency_is_frozen_to_one_without_a_provider_call() {
        let rates = FixedRate {
            calls: AtomicUsize::new(0),
            value: Ok(0.5),
        };
        assert_eq!(
            rate_to_base(&rates, "GBP", "GBP")
                .await
                .expect("same currency"),
            1.0
        );
        assert_eq!(rates.calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            rate_to_base(&rates, "EUR", "GBP")
                .await
                .expect("cross currency"),
            0.5
        );
        assert_eq!(rates.calls.load(Ordering::SeqCst), 1);

        let invalid = FixedRate {
            calls: AtomicUsize::new(0),
            value: Ok(f64::INFINITY),
        };
        assert!(matches!(
            rate_to_base(&invalid, "EUR", "GBP").await,
            Err(LedgerServiceError::FxRate(FxRateError::InvalidResponse))
        ));
    }

    #[test]
    fn small_transfers_keep_the_debtor_who_actually_owes_them() {
        let balances = vec![
            LedgerBalance {
                user_id: "debtor-a".into(),
                paid: 0.0,
                owed: 9.0,
                net: -9.0,
            },
            LedgerBalance {
                user_id: "debtor-b".into(),
                paid: 0.0,
                owed: 2.0,
                net: -2.0,
            },
            LedgerBalance {
                user_id: "creditor".into(),
                paid: 11.0,
                owed: 0.0,
                net: 11.0,
            },
        ];
        assert_eq!(
            simplify_debts(&balances).expect("balances reconcile"),
            vec![
                SuggestedTransfer {
                    from_user: "debtor-a".into(),
                    to_user: "creditor".into(),
                    amount: 9.0,
                },
                SuggestedTransfer {
                    from_user: "debtor-b".into(),
                    to_user: "creditor".into(),
                    amount: 2.0,
                },
            ]
        );
    }

    #[test]
    fn half_units_round_away_from_zero_for_positive_and_negative_balances() {
        assert_eq!(round_half_away_from_zero(0.5), 1.0);
        assert_eq!(round_half_away_from_zero(-0.5), -1.0);
        assert_eq!(round2(1.005), 1.0, "binary input remains deterministic");

        let mut one_unit = expense(ExpenseSplit::Even {
            participant_ids: vec!["user-a".into(), "user-b".into()],
        });
        one_unit.amount = 1.0;
        one_unit.fx_rate_to_base = 1.0;
        let view = compute_ledger(
            "trip-a",
            "GBP",
            vec!["user-a".into(), "user-b".into()],
            vec![one_unit],
            vec![],
        )
        .expect("one-unit split computes");
        assert_eq!(
            view.suggested_transfers,
            vec![SuggestedTransfer {
                from_user: "user-b".into(),
                to_user: "user-a".into(),
                amount: 1.0,
            }]
        );
    }

    #[test]
    fn derived_people_and_final_serialized_response_are_bounded() {
        let too_many_people = (0..=MAX_LEDGER_PEOPLE)
            .map(|index| format!("user-{index}"))
            .collect();
        assert!(matches!(
            compute_ledger("trip-a", "GBP", too_many_people, vec![], vec![]),
            Err(LedgerServiceError::Repository(
                LedgerRepoError::SafetyLimitExceeded
            ))
        ));

        let expenses = (0..500)
            .map(|index| {
                let mut value = expense(ExpenseSplit::Even {
                    participant_ids: vec!["user-a".into()],
                });
                value.id = format!("expense-{index}");
                value.note = "x".repeat(MAX_EXPENSE_NOTE_CHARS);
                value
            })
            .collect();
        assert!(matches!(
            compute_ledger("trip-a", "GBP", vec!["user-a".into()], expenses, vec![]),
            Err(LedgerServiceError::Repository(
                LedgerRepoError::SafetyLimitExceeded
            ))
        ));
    }
}
