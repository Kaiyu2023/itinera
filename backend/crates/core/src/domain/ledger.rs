use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpenseCategory {
    Lodging,
    Food,
    Transport,
    Tickets,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ShareParticipant {
    pub user_id: String,
    pub weight: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExactParticipant {
    pub user_id: String,
    pub amount: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ExpenseSplit {
    Even { participant_ids: Vec<String> },
    Shares { participants: Vec<ShareParticipant> },
    Exact { participants: Vec<ExactParticipant> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Expense {
    pub id: String,
    pub trip_id: String,
    pub paid_by: String,
    pub amount: f64,
    pub currency: String,
    pub fx_rate_to_base: f64,
    pub category: ExpenseCategory,
    pub split: ExpenseSplit,
    pub note: String,
    pub receipt_photo_url: Option<String>,
    pub linked_stop_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settlement {
    pub id: String,
    pub trip_id: String,
    pub from_user: String,
    pub to_user: String,
    pub amount: f64,
    pub settled_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LedgerBalance {
    pub user_id: String,
    pub paid: f64,
    pub owed: f64,
    pub net: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SuggestedTransfer {
    pub from_user: String,
    pub to_user: String,
    pub amount: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LedgerView {
    pub expenses: Vec<Expense>,
    pub settlements: Vec<Settlement>,
    pub balances: Vec<LedgerBalance>,
    pub suggested_transfers: Vec<SuggestedTransfer>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn split_union_matches_the_strict_discriminated_contract() {
        let split: ExpenseSplit = serde_json::from_value(json!({
            "kind": "exact",
            "participants": [{"userId": "user-a", "amount": 5.0}]
        }))
        .expect("valid split");
        assert!(matches!(split, ExpenseSplit::Exact { .. }));
        assert!(
            serde_json::from_value::<ExpenseSplit>(json!({
                "kind": "even",
                "participantIds": ["user-a"],
                "forged": true
            }))
            .is_err()
        );
    }
}
