use crate::domain::user::UserId;

/// The principal whose authority must be rechecked by a trip repository.
///
/// Authentication may resolve a service to its human owner, but the service
/// identity remains part of the authorization decision. Repositories must not
/// reduce `Service` to only `owner_id` before checking the retained mapping,
/// scope, trip allowlist, and owner membership in their transaction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TripAuthorizationContext {
    Human {
        user_id: UserId,
    },
    Service {
        owner_id: UserId,
        service_id: String,
    },
}

impl TripAuthorizationContext {
    pub fn human(user_id: UserId) -> Self {
        Self::Human { user_id }
    }

    pub fn service(owner_id: UserId, service_id: String) -> Self {
        Self::Service {
            owner_id,
            service_id,
        }
    }

    /// Stable user whose direct trip membership is authoritative.
    pub fn owner_id(&self) -> &UserId {
        match self {
            Self::Human { user_id } => user_id,
            Self::Service { owner_id, .. } => owner_id,
        }
    }

    /// Returns the actor only when the authenticated principal is human.
    pub fn human_user_id(&self) -> Option<&UserId> {
        match self {
            Self::Human { user_id } => Some(user_id),
            Self::Service { .. } => None,
        }
    }

    pub fn service_id(&self) -> Option<&str> {
        match self {
            Self::Human { .. } => None,
            Self::Service { service_id, .. } => Some(service_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TripAuthorizationContext;
    use crate::domain::user::UserId;

    #[test]
    fn service_context_retains_both_owner_and_service_identity() {
        let owner = UserId("owner-a".into());
        let context = TripAuthorizationContext::service(owner.clone(), "service-a".into());

        assert_eq!(context.owner_id(), &owner);
        assert_eq!(context.human_user_id(), None);
        assert_eq!(context.service_id(), Some("service-a"));
    }
}
