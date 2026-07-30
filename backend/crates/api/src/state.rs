use std::sync::Arc;

use itinera_core::ports::{identity::IdentityProvider, ids::IdGen, user::UserRepo};

#[derive(Clone)]
pub struct AppState {
    pub identity: Arc<dyn IdentityProvider>,
    pub users: Arc<dyn UserRepo>,
    pub id_gen: Arc<dyn IdGen>,
}
