//! Owner-scoped Cloudflare Access service mappings and hourly usage limits.
//!
//! This capability deliberately stays separate from trip persistence. A
//! service credential resolves through a hashed global claim to one owner
//! mapping; trip repositories still perform their own strongly consistent
//! direct-membership authorization for every data operation.

use async_trait::async_trait;
use itinera_core::{
    domain::{
        service_identity::{ServiceGrant, ServiceIdentity},
        user::UserId,
    },
    ports::{
        auth::ServiceCommonName,
        service_identity::{NewServiceIdentity, ServiceIdentityRepo, ServiceIdentityRepoError},
    },
};

use super::DynamoUserRepo;

mod access;
mod operations;
mod records;

#[cfg(test)]
mod tests;

#[async_trait]
impl ServiceIdentityRepo for DynamoUserRepo {
    async fn list_service_identities(
        &self,
        owner: &UserId,
    ) -> Result<Vec<ServiceIdentity>, ServiceIdentityRepoError> {
        operations::list_service_identities(self, owner).await
    }

    async fn create_service_identity(
        &self,
        owner: &UserId,
        new_identity: NewServiceIdentity,
    ) -> Result<ServiceIdentity, ServiceIdentityRepoError> {
        operations::create_service_identity(self, owner, new_identity).await
    }

    async fn revoke_service_identity(
        &self,
        owner: &UserId,
        service_id: &str,
        revoked_at: &str,
    ) -> Result<(), ServiceIdentityRepoError> {
        operations::revoke_service_identity(self, owner, service_id, revoked_at).await
    }

    async fn authenticate_service(
        &self,
        common_name: &ServiceCommonName,
        used_at: &str,
    ) -> Result<ServiceGrant, ServiceIdentityRepoError> {
        operations::authenticate_service(self, common_name, used_at).await
    }
}
