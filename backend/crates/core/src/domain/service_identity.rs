use serde::{Deserialize, Serialize};

use super::user::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceScope {
    Read,
    Propose,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServiceIdentity {
    pub id: String,
    pub name: String,
    pub client_id_hint: String,
    pub scopes: Vec<ServiceScope>,
    pub trip_ids: Vec<String>,
    pub expires_at: String,
    pub last_used_at: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: String,
}

impl ServiceIdentity {
    pub fn has_scope(&self, scope: ServiceScope) -> bool {
        self.scopes.contains(&scope)
    }

    pub fn permits_trip(&self, trip_id: &str) -> bool {
        self.trip_ids.iter().any(|allowed| allowed == trip_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceGrant {
    pub owner_id: UserId,
    pub identity: ServiceIdentity,
}
