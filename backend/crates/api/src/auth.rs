use axum::extract::FromRequestParts;
use itinera_core::{
    domain::{
        service_identity::{ServiceIdentity, ServiceScope},
        user::User,
    },
    ports::{auth::Identity, user::UserRepoError},
    services::{provisioning::get_or_provision, service_identities::authenticate_service},
};

use crate::{error::ApiError, state::AppState};

const ASSERTION_HEADER: &str = "cf-access-jwt-assertion";

pub struct AuthenticatedPrincipal {
    user: User,
    authority: Authority,
}

enum Authority {
    Human,
    Service(Box<ServiceIdentity>),
}

impl AuthenticatedPrincipal {
    pub fn require_human(self) -> Result<User, ApiError> {
        match self.authority {
            Authority::Human => Ok(self.user),
            Authority::Service(_) => Err(ApiError::forbidden()),
        }
    }

    pub fn require_trip_read(self, trip_id: &str) -> Result<User, ApiError> {
        match self.authority {
            Authority::Human => Ok(self.user),
            Authority::Service(identity) if !identity.has_scope(ServiceScope::Read) => {
                Err(ApiError::forbidden())
            }
            Authority::Service(identity) if !identity.permits_trip(trip_id) => {
                Err(ApiError::not_found())
            }
            Authority::Service(_) => Ok(self.user),
        }
    }

    pub fn require_trip_list(self) -> Result<(User, Option<Vec<String>>), ApiError> {
        match self.authority {
            Authority::Human => Ok((self.user, None)),
            Authority::Service(identity) if !identity.has_scope(ServiceScope::Read) => {
                Err(ApiError::forbidden())
            }
            Authority::Service(identity) => Ok((self.user, Some(identity.trip_ids))),
        }
    }
}

impl FromRequestParts<AppState> for AuthenticatedPrincipal {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        if let Some(assertion_header) = parts.headers.get(ASSERTION_HEADER)
            && let Ok(assertion) = assertion_header.to_str()
        {
            match state.identity.authenticate(assertion).await? {
                Identity::Human { email } => {
                    let user = get_or_provision(&*state.users, &*state.id_gen, email).await?;
                    Ok(AuthenticatedPrincipal {
                        user,
                        authority: Authority::Human,
                    })
                }
                Identity::Service { common_name } => {
                    let grant = authenticate_service(
                        &*state.service_identities,
                        &*state.clock,
                        &common_name,
                    )
                    .await?;
                    let user = state
                        .users
                        .find_by_id(&grant.owner_id)
                        .await?
                        .ok_or(UserRepoError::CorruptData)?;
                    Ok(AuthenticatedPrincipal {
                        user,
                        authority: Authority::Service(Box::new(grant.identity)),
                    })
                }
            }
        } else {
            Err(ApiError::missing_credentials())
        }
    }
}
