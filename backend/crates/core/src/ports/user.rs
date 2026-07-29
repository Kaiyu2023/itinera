use async_trait::async_trait;

use crate::domain::user::{Email, User};

#[derive(Debug, Clone, thiserror::Error)]
pub enum UserRepoError {
    #[error("The user repo is unavailable")]
    UserRepoUnavailable,
    #[error("The operation on the user repo is invalid")]
    InvalidOperation,
}

#[async_trait]
pub trait UserRepo: Send + Sync {
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, UserRepoError>;
    async fn insert(&self, user: User) -> Result<(), UserRepoError>;
}
