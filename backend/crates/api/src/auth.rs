use axum::extract::FromRequestParts;
use itinera_core::{
    domain::{
        service_identity::{ServiceIdentity, ServiceScope},
        user::User,
    },
    ports::{auth::Identity, authorization::TripAuthorizationContext, user::UserRepoError},
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

    pub fn require_human_trip(self) -> Result<TripAuthorizationContext, ApiError> {
        match self.authority {
            Authority::Human => Ok(TripAuthorizationContext::human(self.user.id)),
            Authority::Service(_) => Err(ApiError::forbidden()),
        }
    }

    pub fn require_trip_read(self, trip_id: &str) -> Result<TripAuthorizationContext, ApiError> {
        match self.authority {
            Authority::Human => Ok(TripAuthorizationContext::human(self.user.id)),
            Authority::Service(identity) if !identity.has_scope(ServiceScope::Read) => {
                Err(ApiError::forbidden())
            }
            Authority::Service(identity) if !identity.permits_trip(trip_id) => {
                Err(ApiError::not_found())
            }
            Authority::Service(identity) => {
                Ok(TripAuthorizationContext::service(self.user.id, identity.id))
            }
        }
    }

    pub fn require_trip_list(
        self,
    ) -> Result<(TripAuthorizationContext, Option<Vec<String>>), ApiError> {
        match self.authority {
            Authority::Human => Ok((TripAuthorizationContext::human(self.user.id), None)),
            Authority::Service(identity) if !identity.has_scope(ServiceScope::Read) => {
                Err(ApiError::forbidden())
            }
            Authority::Service(identity) => Ok((
                TripAuthorizationContext::service(self.user.id, identity.id),
                Some(identity.trip_ids),
            )),
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

#[cfg(test)]
mod tests {
    use itinera_core::{
        domain::{
            service_identity::{ServiceIdentity, ServiceScope},
            user::{Email, User, UserId},
        },
        ports::authorization::TripAuthorizationContext,
    };

    use super::{AuthenticatedPrincipal, Authority};

    #[test]
    fn service_trip_authorization_retains_owner_and_service_id() {
        let principal = AuthenticatedPrincipal {
            user: User {
                id: UserId("owner-a".into()),
                email: Email::parse("owner@example.com").expect("valid email"),
                display_name: Some("Owner".into()),
            },
            authority: Authority::Service(Box::new(ServiceIdentity {
                id: "service-a".into(),
                name: "Automation".into(),
                client_id_hint: "cdef.access".into(),
                scopes: vec![ServiceScope::Read],
                trip_ids: vec!["trip-a".into()],
                expires_at: "2026-08-09T00:00:00Z".into(),
                last_used_at: None,
                revoked_at: None,
                created_at: "2026-08-08T00:00:00Z".into(),
            })),
        };

        assert_eq!(
            principal
                .require_trip_read("trip-a")
                .expect("read is authorized"),
            TripAuthorizationContext::service(UserId("owner-a".into()), "service-a".into())
        );
    }
}
