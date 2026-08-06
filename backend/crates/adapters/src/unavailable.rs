use async_trait::async_trait;
use itinera_core::{
    domain::{trip::Place, user::Email},
    ports::{
        access_policy::{AccessPolicy, AccessPolicyError},
        place_catalog::{PlaceCatalog, PlaceCatalogError},
    },
};

/// Fail-closed placeholder until the Cloudflare policy adapter is wired in
/// Phase B step 4. It never creates a local invite that cannot be redeemed.
pub struct UnavailableAccessPolicy;

#[async_trait]
impl AccessPolicy for UnavailableAccessPolicy {
    async fn grant_login(&self, _email: &Email) -> Result<(), AccessPolicyError> {
        Err(AccessPolicyError::Unavailable)
    }

    async fn revoke_login(&self, _email: &Email) -> Result<(), AccessPolicyError> {
        Err(AccessPolicyError::Unavailable)
    }
}

/// Fail-closed placeholder for the Phase B step 4 provider. Same-trip saved
/// places remain repository-backed, but a catalog response is never invented.
pub struct UnavailablePlaceCatalog;

#[async_trait]
impl PlaceCatalog for UnavailablePlaceCatalog {
    async fn search(&self, _query: &str) -> Result<Vec<Place>, PlaceCatalogError> {
        Err(PlaceCatalogError::Unavailable)
    }

    async fn find(&self, _place_id: &str) -> Result<Option<Place>, PlaceCatalogError> {
        Err(PlaceCatalogError::Unavailable)
    }
}
