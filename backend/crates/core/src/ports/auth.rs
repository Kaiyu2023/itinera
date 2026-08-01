use async_trait::async_trait;

use crate::domain::user::Email;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Identity {
    pub email: Email,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("the access assertion is malformed or its signature is invalid")]
    InvalidToken,
    #[error("the access token has expired")]
    ExpiredToken,
    #[error("the identity provider is unavailable")]
    IdentityProviderUnavailable,
}

#[async_trait]
pub trait IdentityProvider: Send + Sync {
    async fn authenticate(&self, assertion: &str) -> Result<Identity, AuthError>;
}
