use std::sync::Arc;

use itinera_core::ports::{
    access_policy::AccessPolicy, auth::IdentityProvider, clock::Clock,
    content_history::ContentHistoryRepo, discussion::DiscussionRepo, fx_rate::FxRateProvider,
    id_gen::IdGen, ledger::LedgerRepo, notice::NoticeRepo, place_catalog::PlaceCatalog,
    poll::PollRepo, proposal::ProposalRepo, service_identity::ServiceIdentityRepo, trip::TripRepo,
    user::UserRepo,
};

#[derive(Clone)]
pub struct AppState {
    pub identity: Arc<dyn IdentityProvider>,
    pub users: Arc<dyn UserRepo>,
    pub trips: Arc<dyn TripRepo>,
    pub content_history: Arc<dyn ContentHistoryRepo>,
    pub proposals: Arc<dyn ProposalRepo>,
    pub polls: Arc<dyn PollRepo>,
    pub discussions: Arc<dyn DiscussionRepo>,
    pub ledger: Arc<dyn LedgerRepo>,
    pub notices: Arc<dyn NoticeRepo>,
    pub service_identities: Arc<dyn ServiceIdentityRepo>,
    pub access_policy: Arc<dyn AccessPolicy>,
    pub place_catalog: Arc<dyn PlaceCatalog>,
    pub fx_rates: Arc<dyn FxRateProvider>,
    pub id_gen: Arc<dyn IdGen>,
    pub clock: Arc<dyn Clock>,
}
