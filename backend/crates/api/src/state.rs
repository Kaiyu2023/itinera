use std::sync::Arc;

use itinera_core::ports::{auth::IdentityProvider, id_gen::IdGen, user::UserRepo};

#[derive(Clone)]
pub struct AppState {
    pub identity: Arc<dyn IdentityProvider>,
    pub users: Arc<dyn UserRepo>,
    pub id_gen: Arc<dyn IdGen>,
}
