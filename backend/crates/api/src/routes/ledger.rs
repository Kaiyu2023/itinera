use axum::{
    Json,
    body::Bytes,
    extract::{Path, State, rejection::BytesRejection, rejection::JsonRejection},
    http::{HeaderMap, StatusCode},
};
use itinera_core::{
    domain::ledger::{Expense, ExpenseCategory, ExpenseSplit, LedgerView, Settlement},
    services::ledger::{
        self, AddExpenseInput, AddSettlementInput, ExpensePatch, LedgerRequestContext,
    },
};
use serde::{Deserialize, Deserializer};

use crate::{
    auth::AuthenticatedUser,
    error::ApiError,
    routes::{require_empty_body, required_idempotency_key},
    state::AppState,
};

pub const LEDGER_BODYLESS_LIMIT_BYTES: usize = 1_024;
pub const LEDGER_WRITE_BODY_LIMIT_BYTES: usize = 64 * 1_024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddExpenseRequest {
    paid_by: String,
    amount: f64,
    currency: String,
    category: ExpenseCategory,
    split: ExpenseSplit,
    note: String,
    linked_stop_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateExpenseRequest {
    paid_by: Option<String>,
    amount: Option<f64>,
    currency: Option<String>,
    category: Option<ExpenseCategory>,
    split: Option<ExpenseSplit>,
    note: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_nullable")]
    linked_stop_id: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddSettlementRequest {
    from_user: String,
    to_user: String,
    amount: f64,
}

pub async fn get_ledger(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path(trip_id): Path<String>,
    body: Result<Bytes, BytesRejection>,
) -> Result<Json<LedgerView>, ApiError> {
    require_empty_body(body)?;
    Ok(Json(
        ledger::get_ledger(&*state.ledger, &trip_id, &actor.id).await?,
    ))
}

pub async fn add_expense(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path(trip_id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<AddExpenseRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Expense>), ApiError> {
    let Json(request) = payload?;
    let idempotency_key = required_idempotency_key(&headers)?;
    let expense = ledger::add_expense(
        &*state.ledger,
        &*state.fx_rates,
        &*state.id_gen,
        &*state.clock,
        LedgerRequestContext {
            trip_id: &trip_id,
            actor: &actor.id,
        },
        &idempotency_key,
        AddExpenseInput {
            paid_by: request.paid_by,
            amount: request.amount,
            currency: request.currency,
            category: request.category,
            split: request.split,
            note: request.note,
            linked_stop_id: request.linked_stop_id,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(expense)))
}

pub async fn update_expense(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path((trip_id, expense_id)): Path<(String, String)>,
    payload: Result<Json<UpdateExpenseRequest>, JsonRejection>,
) -> Result<Json<Expense>, ApiError> {
    let Json(request) = payload?;
    Ok(Json(
        ledger::update_expense(
            &*state.ledger,
            &*state.fx_rates,
            &*state.id_gen,
            &*state.clock,
            LedgerRequestContext {
                trip_id: &trip_id,
                actor: &actor.id,
            },
            &expense_id,
            ExpensePatch {
                paid_by: request.paid_by,
                amount: request.amount,
                currency: request.currency,
                category: request.category,
                split: request.split,
                note: request.note,
                linked_stop_id: request.linked_stop_id,
            },
        )
        .await?,
    ))
}

pub async fn delete_expense(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path((trip_id, expense_id)): Path<(String, String)>,
    body: Result<Bytes, BytesRejection>,
) -> Result<StatusCode, ApiError> {
    require_empty_body(body)?;
    ledger::delete_expense(
        &*state.ledger,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &actor.id,
        &expense_id,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn add_settlement(
    State(state): State<AppState>,
    AuthenticatedUser(actor): AuthenticatedUser,
    Path(trip_id): Path<String>,
    headers: HeaderMap,
    payload: Result<Json<AddSettlementRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<Settlement>), ApiError> {
    let Json(request) = payload?;
    let idempotency_key = required_idempotency_key(&headers)?;
    let settlement = ledger::add_settlement(
        &*state.ledger,
        &*state.id_gen,
        &*state.clock,
        &trip_id,
        &actor.id,
        &idempotency_key,
        AddSettlementInput {
            from_user: request.from_user,
            to_user: request.to_user,
            amount: request.amount,
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(settlement)))
}

fn deserialize_present_nullable<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, StatusCode};
    use serde_json::json;

    use super::{UpdateExpenseRequest, required_idempotency_key};

    #[test]
    fn linked_stop_patch_distinguishes_omission_null_and_value() {
        let omitted: UpdateExpenseRequest =
            serde_json::from_value(json!({"note": "Dinner"})).expect("valid patch");
        let cleared: UpdateExpenseRequest =
            serde_json::from_value(json!({"linkedStopId": null})).expect("valid patch");
        let linked: UpdateExpenseRequest = serde_json::from_value(json!({
            "linkedStopId": "stop-a"
        }))
        .expect("valid patch");
        assert_eq!(omitted.linked_stop_id, None);
        assert_eq!(cleared.linked_stop_id, Some(None));
        assert_eq!(linked.linked_stop_id, Some(Some("stop-a".into())));
    }

    #[test]
    fn idempotency_header_is_required_exactly_once() {
        let missing = required_idempotency_key(&HeaderMap::new()).expect_err("required");
        assert_eq!(missing.status_code, StatusCode::BAD_REQUEST);

        let mut duplicate = HeaderMap::new();
        duplicate.append("idempotency-key", HeaderValue::from_static("first"));
        duplicate.append("idempotency-key", HeaderValue::from_static("second"));
        let duplicate = required_idempotency_key(&duplicate).expect_err("one value only");
        assert_eq!(duplicate.status_code, StatusCode::BAD_REQUEST);
    }
}
