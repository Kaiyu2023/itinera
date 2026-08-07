use async_trait::async_trait;

use crate::domain::{
    ledger::{Expense, Settlement},
    user::UserId,
};

#[derive(Debug, Clone, PartialEq)]
pub struct LedgerData {
    pub base_currency: String,
    pub current_member_ids: Vec<String>,
    pub expenses: Vec<Expense>,
    pub settlements: Vec<Settlement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerTripContext {
    pub base_currency: String,
    pub trip_revision: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VersionedExpense {
    pub expense: Expense,
    pub revision: u64,
    pub context: LedgerTripContext,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewExpense {
    pub expense: Expense,
    pub context: LedgerTripContext,
    pub idempotency_key: String,
    pub request_hash: String,
    pub audit_id: String,
    pub audit_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExpenseReplacement {
    pub expense: Expense,
    pub expected_revision: u64,
    pub context: LedgerTripContext,
    pub audit_id: String,
    pub audit_at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewSettlement {
    pub settlement: Settlement,
    pub idempotency_key: String,
    pub request_hash: String,
    pub audit_id: String,
    pub audit_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LedgerRepoError {
    #[error("ledger storage is unavailable")]
    Unavailable,
    #[error("ledger storage contains corrupt data")]
    CorruptData,
    #[error("ledger resource not found")]
    NotFound,
    #[error("ledger operation is forbidden")]
    Forbidden,
    #[error("ledger operation conflicts with current state")]
    Conflict,
    #[error("ledger safety limit exceeded")]
    SafetyLimitExceeded,
}

#[async_trait]
pub trait LedgerRepo: Send + Sync {
    async fn get_ledger_data(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<LedgerData, LedgerRepoError>;

    async fn get_trip_context(
        &self,
        trip_id: &str,
        actor: &UserId,
    ) -> Result<LedgerTripContext, LedgerRepoError>;

    /// Replays the original create response for an exact operation-key and
    /// request-hash match. A reused key with different provenance or input is
    /// a conflict rather than a new write.
    async fn replay_expense_creation(
        &self,
        trip_id: &str,
        actor: &UserId,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<Option<Expense>, LedgerRepoError>;

    async fn replay_settlement_creation(
        &self,
        trip_id: &str,
        actor: &UserId,
        idempotency_key: &str,
        request_hash: &str,
    ) -> Result<Option<Settlement>, LedgerRepoError>;

    async fn get_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        expense_id: &str,
    ) -> Result<VersionedExpense, LedgerRepoError>;

    async fn add_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewExpense,
    ) -> Result<Expense, LedgerRepoError>;

    async fn replace_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        replacement: ExpenseReplacement,
    ) -> Result<Expense, LedgerRepoError>;

    async fn delete_expense(
        &self,
        trip_id: &str,
        actor: &UserId,
        expense_id: &str,
        audit_id: &str,
        audit_at: &str,
    ) -> Result<(), LedgerRepoError>;

    async fn add_settlement(
        &self,
        trip_id: &str,
        actor: &UserId,
        new: NewSettlement,
    ) -> Result<Settlement, LedgerRepoError>;
}
