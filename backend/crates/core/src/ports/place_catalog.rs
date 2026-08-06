use async_trait::async_trait;

use crate::domain::trip::Place;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlaceCatalogError {
    #[error("the place catalog is unavailable")]
    Unavailable,
    #[error("the place was not found")]
    NotFound,
}

#[async_trait]
pub trait PlaceCatalog: Send + Sync {
    async fn search(&self, query: &str) -> Result<Vec<Place>, PlaceCatalogError>;
    async fn find(&self, place_id: &str) -> Result<Option<Place>, PlaceCatalogError>;
}
