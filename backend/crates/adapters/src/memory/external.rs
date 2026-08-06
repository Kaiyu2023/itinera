use async_trait::async_trait;
use itinera_core::{
    domain::{trip::Place, user::Email},
    ports::{
        access_policy::{AccessPolicy, AccessPolicyError},
        place_catalog::{PlaceCatalog, PlaceCatalogError},
    },
};

/// Development-only login policy. The insecure identity adapter already lets
/// any asserted email through, so grant/revoke are intentionally no-ops here.
pub struct DevAccessPolicy;

#[async_trait]
impl AccessPolicy for DevAccessPolicy {
    async fn grant_login(&self, _email: &Email) -> Result<(), AccessPolicyError> {
        Ok(())
    }

    async fn revoke_login(&self, _email: &Email) -> Result<(), AccessPolicyError> {
        Ok(())
    }
}

/// Empty development catalog. Manual and same-trip saved places work without
/// provider credentials; Google-backed results arrive in Phase B step 4.
pub struct EmptyPlaceCatalog;

#[async_trait]
impl PlaceCatalog for EmptyPlaceCatalog {
    async fn search(&self, _query: &str) -> Result<Vec<Place>, PlaceCatalogError> {
        Ok(vec![])
    }

    async fn find(&self, _place_id: &str) -> Result<Option<Place>, PlaceCatalogError> {
        Ok(None)
    }
}
