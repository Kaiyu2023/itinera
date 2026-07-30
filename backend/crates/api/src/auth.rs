use axum::extract::FromRequestParts;
use itinera_core::{domain::user::User, services::provisioning::get_or_provision};

use crate::{error::ApiError, state::AppState};

const ASSERTION_HEADER: &str = "cf-access-jwt-assertion";

pub struct AuthenticatedUser(pub User);

impl FromRequestParts<AppState> for AuthenticatedUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(assertion_header) = parts.headers.get(ASSERTION_HEADER)
            && let Ok(assertion) = assertion_header.to_str()
        {
            let authed_identity = state.identity.authenticate(assertion).await?;
            let user = get_or_provision(&*state.users, &*state.id_gen, authed_identity).await?;
            Ok(AuthenticatedUser(user))
        } else {
            Err(ApiError::missing_credentials())
        }
    }
}
