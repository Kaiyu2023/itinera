//! DynamoDB ledger repository.
//!
//! Expenses, settlements, stop-link claims, aggregate metadata, and audit
//! events form an isolated capability. The ledger metadata revision protects
//! bounded graph validation from racing a concurrent monetary write.

use async_trait::async_trait;
use itinera_core::{
    domain::{
        ledger::{Expense, Settlement},
        user::UserId,
    },
    ports::ledger::{
        ExpenseReplacement, LedgerData, LedgerRepo, LedgerRepoError, LedgerTripContext, NewExpense,
        NewSettlement, VersionedExpense,
    },
};

use super::DynamoUserRepo;

mod access;
mod operations;
pub(in crate::dynamodb) mod records;

#[cfg(test)]
mod tests;

#[async_trait]
impl LedgerRepo for DynamoUserRepo {
    async fn get_ledger_data(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<LedgerData, LedgerRepoError> {
        operations::get_ledger_data(self, trip_id, actor).await
    }

    async fn get_trip_context(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<LedgerTripContext, LedgerRepoError> {
        operations::get_trip_context(self, trip_id, actor).await
    }

    async fn replay_expense_creation(
        &self,
        trip_id: &str,
        actor: &UserId,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<Option<Expense>, LedgerRepoError> {
        operations::replay_expense_creation(self, trip_id, actor, idempotency_key, request_hash)
            .await
    }

    async fn replay_settlement_creation(
        &self,
        trip_id: &str,
        actor: &UserId,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<Option<Settlement>, LedgerRepoError> {
        operations::replay_settlement_creation(self, trip_id, actor, idempotency_key, request_hash)
            .await
    }

    async fn get_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        expense_id: &str,
    ) -> Result<VersionedExpense, LedgerRepoError> {
        operations::get_expense(self, trip_id, actor, expense_id).await
    }

    async fn add_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewExpense,
    ) -> Result<Expense, LedgerRepoError> {
        operations::add_expense(self, trip_id, actor, new).await
    }

    async fn replace_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        replacement: ExpenseReplacement,
    ) -> Result<Expense, LedgerRepoError> {
        operations::replace_expense(self, trip_id, actor, replacement).await
    }

    async fn delete_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        expense_id: &str,
        audit_id: &str,
        audit_at: &str,
    ) -> Result<(), LedgerRepoError> {
        operations::delete_expense(self, trip_id, actor, expense_id, audit_id, audit_at).await
    }

    async fn add_settlement(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewSettlement,
    ) -> Result<Settlement, LedgerRepoError> {
        operations::add_settlement(self, trip_id, actor, new).await
    }
}

fn record_error(_: itinera_core::ports::trip::TripRepoError) -> LedgerRepoError {
    LedgerRepoError::CorruptData
}
