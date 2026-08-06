use std::sync::Arc;

use itinera_core::ports::{
    access_policy::AccessPolicy, auth::IdentityProvider, clock::Clock,
    content_history::ContentHistoryRepo, id_gen::IdGen, place_catalog::PlaceCatalog,
    proposal::ProposalRepo, trip::TripRepo, user::UserRepo,
};

#[derive(Clone)]
pub struct AppState {
    pub identity: Arc<dyn IdentityProvider>,
    pub users: Arc<dyn UserRepo>,
    pub trips: Arc<dyn TripRepo>,
    pub content_history: Arc<dyn ContentHistoryRepo>,
    pub proposals: Arc<dyn ProposalRepo>,
    pub access_policy: Arc<dyn AccessPolicy>,
    pub place_catalog: Arc<dyn PlaceCatalog>,
    pub id_gen: Arc<dyn IdGen>,
    pub clock: Arc<dyn Clock>,
}
