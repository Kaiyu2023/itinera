use std::{collections::HashMap, sync::RwLock};

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
    services::service_identities::SERVICE_REQUESTS_PER_HOUR,
};

struct StoredService {
    owner: UserId,
    common_name: String,
    identity: ServiceIdentity,
    uses: u32,
}

/// Router-test-only fake. Runtime builds have no alternate persistence path.
#[derive(Default)]
pub struct TestServiceIdentityRepo {
    services: RwLock<HashMap<String, StoredService>>,
}

impl TestServiceIdentityRepo {
    // This support module is compiled independently into every integration
    // test target; only the service-identity target needs quota control.
    #[allow(dead_code)]
    pub fn set_uses(&self, common_name: &str, uses: u32) {
        if let Some(stored) = self
            .services
            .write()
            .expect("service fake lock")
            .values_mut()
            .find(|stored| stored.common_name == common_name)
        {
            stored.uses = uses;
        }
    }
}

#[async_trait]
impl ServiceIdentityRepo for TestServiceIdentityRepo {
    async fn list_service_identities(
        &self,
        owner: &UserId,
    ) -> Result<Vec<ServiceIdentity>, ServiceIdentityRepoError> {
        let services = self
            .services
            .read()
            .map_err(|_| ServiceIdentityRepoError::Unavailable)?;
        let mut identities: Vec<_> = services
            .values()
            .filter(|stored| &stored.owner == owner)
            .map(|stored| stored.identity.clone())
            .collect();
        identities.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(identities)
    }

    async fn create_service_identity(
        &self,
        owner: &UserId,
        new_identity: NewServiceIdentity,
    ) -> Result<ServiceIdentity, ServiceIdentityRepoError> {
        let mut services = self
            .services
            .write()
            .map_err(|_| ServiceIdentityRepoError::Unavailable)?;
        if services
            .values()
            .any(|stored| stored.common_name == new_identity.common_name.as_str())
        {
            return Err(ServiceIdentityRepoError::DuplicateCredential);
        }
        let identity = new_identity.identity;
        services.insert(
            identity.id.clone(),
            StoredService {
                owner: owner.clone(),
                common_name: new_identity.common_name.as_str().to_string(),
                identity: identity.clone(),
                uses: 0,
            },
        );
        Ok(identity)
    }

    async fn revoke_service_identity(
        &self,
        owner: &UserId,
        service_id: &str,
        revoked_at: &str,
    ) -> Result<(), ServiceIdentityRepoError> {
        let mut services = self
            .services
            .write()
            .map_err(|_| ServiceIdentityRepoError::Unavailable)?;
        let stored = services
            .get_mut(service_id)
            .filter(|stored| &stored.owner == owner)
            .ok_or(ServiceIdentityRepoError::NotFound)?;
        stored
            .identity
            .revoked_at
            .get_or_insert_with(|| revoked_at.to_string());
        Ok(())
    }

    async fn authenticate_service(
        &self,
        common_name: &ServiceCommonName,
        used_at: &str,
    ) -> Result<ServiceGrant, ServiceIdentityRepoError> {
        let mut services = self
            .services
            .write()
            .map_err(|_| ServiceIdentityRepoError::Unavailable)?;
        let stored = services
            .values_mut()
            .find(|stored| stored.common_name == common_name.as_str())
            .ok_or(ServiceIdentityRepoError::CredentialRejected)?;
        if stored.identity.revoked_at.is_some() || stored.identity.expires_at.as_str() <= used_at {
            return Err(ServiceIdentityRepoError::CredentialRejected);
        }
        if stored.uses >= SERVICE_REQUESTS_PER_HOUR {
            return Err(ServiceIdentityRepoError::RateLimited);
        }
        stored.uses += 1;
        stored.identity.last_used_at = Some(used_at.to_string());
        Ok(ServiceGrant {
            owner_id: stored.owner.clone(),
            identity: stored.identity.clone(),
        })
    }
}
