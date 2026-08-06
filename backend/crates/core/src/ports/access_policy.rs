use async_trait::async_trait;

use crate::domain::user::Email;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccessPolicyError {
    #[error("the login policy provider is unavailable")]
    Unavailable,
}

/// App-wide login admission. Trip membership remains authoritative even when
/// revocation is delayed or this provider is unavailable.
#[async_trait]
pub trait AccessPolicy: Send + Sync {
    async fn grant_login(&self, email: &Email) -> Result<(), AccessPolicyError>;
    async fn revoke_login(&self, email: &Email) -> Result<(), AccessPolicyError>;
}
