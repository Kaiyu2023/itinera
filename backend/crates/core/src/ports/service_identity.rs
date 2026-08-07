use async_trait::async_trait;

use crate::domain::{
    service_identity::{ServiceGrant, ServiceIdentity},
    user::UserId,
};

use super::auth::ServiceCommonName;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ServiceIdentityRepoError {
    #[error("service identity storage is unavailable")]
    Unavailable,
    #[error("service identity storage returned corrupt data")]
    CorruptData,
    #[error("service identity not found")]
    NotFound,
    #[error("the actor does not have permission for this service identity")]
    Forbidden,
    #[error("the service identity conflicts with current state")]
    Conflict,
    #[error("that Cloudflare service credential is already mapped")]
    DuplicateCredential,
    #[error("the service credential is unknown, revoked, or expired")]
    CredentialRejected,
    #[error("the service identity exceeded its usage limit")]
    RateLimited,
    #[error("the service identity safety limit was exceeded")]
    SafetyLimitExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewServiceIdentity {
    pub identity: ServiceIdentity,
    pub common_name: ServiceCommonName,
}

#[async_trait]
pub trait ServiceIdentityRepo: Send + Sync {
    async fn list_service_identities(
        &self,
        owner: &UserId,
    ) -> Result<Vec<ServiceIdentity>, ServiceIdentityRepoError>;

    async fn create_service_identity(
        &self,
        owner: &UserId,
        new_identity: NewServiceIdentity,
    ) -> Result<ServiceIdentity, ServiceIdentityRepoError>;

    async fn revoke_service_identity(
        &self,
        owner: &UserId,
        service_id: &str,
        revoked_at: &str,
    ) -> Result<(), ServiceIdentityRepoError>;

    /// Resolve one verified Access `common_name` and atomically consume one
    /// request from its current UTC-hour allowance.
    async fn authenticate_service(
        &self,
        common_name: &ServiceCommonName,
        used_at: &str,
    ) -> Result<ServiceGrant, ServiceIdentityRepoError>;
}
