use std::collections::HashSet;

use chrono::{DateTime, Duration, SecondsFormat, Utc};

use crate::{
    domain::{
        service_identity::{ServiceGrant, ServiceIdentity, ServiceScope},
        user::UserId,
    },
    ports::{
        auth::ServiceCommonName,
        clock::Clock,
        id_gen::IdGen,
        service_identity::{NewServiceIdentity, ServiceIdentityRepo, ServiceIdentityRepoError},
    },
};

use super::validation::{ValidationError, required_text};

pub const MAX_SERVICE_IDENTITIES: usize = 50;
pub const MAX_SERVICE_NAME_CHARS: usize = 80;
pub const MAX_SERVICE_TRIPS: usize = 20;
pub const SERVICE_REQUESTS_PER_HOUR: u32 = 300;
const ALLOWED_TTL_HOURS: [u16; 4] = [1, 8, 24, 168];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterServiceIdentityInput {
    pub name: String,
    pub client_id: String,
    pub scopes: Vec<ServiceScope>,
    pub trip_ids: Vec<String>,
    pub ttl_hours: u16,
}

#[derive(Debug, thiserror::Error)]
pub enum ServiceIdentityServiceError {
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repository(#[from] ServiceIdentityRepoError),
}

pub async fn list_service_identities(
    repo: &dyn ServiceIdentityRepo,
    owner: &UserId,
) -> Result<Vec<ServiceIdentity>, ServiceIdentityServiceError> {
    validate_id(&owner.0, "authenticated user id is invalid")?;
    let identities = repo.list_service_identities(owner).await?;
    if identities.len() > MAX_SERVICE_IDENTITIES {
        return Err(ServiceIdentityRepoError::SafetyLimitExceeded.into());
    }
    for identity in &identities {
        validate_stored_identity(identity).map_err(|_| ServiceIdentityRepoError::CorruptData)?;
    }
    Ok(identities)
}

pub async fn register_service_identity(
    repo: &dyn ServiceIdentityRepo,
    ids: &dyn IdGen,
    clock: &dyn Clock,
    owner: &UserId,
    input: RegisterServiceIdentityInput,
) -> Result<ServiceIdentity, ServiceIdentityServiceError> {
    validate_id(&owner.0, "authenticated user id is invalid")?;
    let name = required_text(
        input.name,
        "name is required and must be at most 80 characters",
        MAX_SERVICE_NAME_CHARS,
    )?;
    let common_name = ServiceCommonName::parse(&input.client_id).map_err(|_| {
        ValidationError("clientId is not a valid Cloudflare service-token client ID")
    })?;
    let scopes = normalize_scopes(input.scopes)?;
    let trip_ids = normalize_trip_ids(input.trip_ids)?;
    if !ALLOWED_TTL_HOURS.contains(&input.ttl_hours) {
        return Err(ValidationError("ttlHours must be one of 1, 8, 24, or 168").into());
    }
    let created = utc_now(clock)?;
    let expires = created
        .checked_add_signed(Duration::hours(i64::from(input.ttl_hours)))
        .ok_or(ValidationError("ttlHours produces an invalid expiry"))?;
    let id = ids.new_id();
    validate_id(&id, "generated service identity id is invalid")?;
    let identity = ServiceIdentity {
        id,
        name,
        client_id_hint: client_id_hint(common_name.as_str()),
        scopes,
        trip_ids,
        expires_at: expires.to_rfc3339_opts(SecondsFormat::AutoSi, true),
        last_used_at: None,
        revoked_at: None,
        created_at: created.to_rfc3339_opts(SecondsFormat::AutoSi, true),
    };
    validate_stored_identity(&identity)?;
    let stored = repo
        .create_service_identity(
            owner,
            NewServiceIdentity {
                identity: identity.clone(),
                common_name,
            },
        )
        .await?;
    if stored != identity {
        return Err(ServiceIdentityRepoError::CorruptData.into());
    }
    validate_stored_identity(&stored).map_err(|_| ServiceIdentityRepoError::CorruptData)?;
    Ok(stored)
}

pub async fn revoke_service_identity(
    repo: &dyn ServiceIdentityRepo,
    clock: &dyn Clock,
    owner: &UserId,
    service_id: &str,
) -> Result<(), ServiceIdentityServiceError> {
    validate_id(&owner.0, "authenticated user id is invalid")?;
    validate_id(service_id, "serviceIdentityId is invalid")?;
    let revoked_at = utc_now(clock)?.to_rfc3339_opts(SecondsFormat::AutoSi, true);
    repo.revoke_service_identity(owner, service_id, &revoked_at)
        .await?;
    Ok(())
}

pub async fn authenticate_service(
    repo: &dyn ServiceIdentityRepo,
    clock: &dyn Clock,
    common_name: &ServiceCommonName,
) -> Result<ServiceGrant, ServiceIdentityServiceError> {
    let now = utc_now(clock)?;
    let used_at = now.to_rfc3339_opts(SecondsFormat::AutoSi, true);
    let grant = repo.authenticate_service(common_name, &used_at).await?;
    validate_id(&grant.owner_id.0, "stored service owner id is invalid")
        .map_err(|_| ServiceIdentityRepoError::CorruptData)?;
    validate_stored_identity(&grant.identity).map_err(|_| ServiceIdentityRepoError::CorruptData)?;
    if grant.identity.revoked_at.is_some()
        || parse_utc(
            &grant.identity.expires_at,
            "stored service expiry is invalid",
        )? <= now
    {
        return Err(ServiceIdentityRepoError::CredentialRejected.into());
    }
    Ok(grant)
}

fn normalize_scopes(scopes: Vec<ServiceScope>) -> Result<Vec<ServiceScope>, ValidationError> {
    if scopes.is_empty() || scopes.len() > 2 {
        return Err(ValidationError("scopes must contain read and/or propose"));
    }
    let unique: HashSet<_> = scopes.iter().copied().collect();
    if unique.len() != scopes.len() {
        return Err(ValidationError("scopes must not contain duplicates"));
    }
    let mut normalized = Vec::with_capacity(unique.len());
    if unique.contains(&ServiceScope::Read) {
        normalized.push(ServiceScope::Read);
    }
    if unique.contains(&ServiceScope::Propose) {
        normalized.push(ServiceScope::Propose);
    }
    Ok(normalized)
}

fn normalize_trip_ids(trip_ids: Vec<String>) -> Result<Vec<String>, ValidationError> {
    if trip_ids.is_empty() || trip_ids.len() > MAX_SERVICE_TRIPS {
        return Err(ValidationError(
            "tripIds must contain between 1 and 20 trips",
        ));
    }
    let mut seen = HashSet::with_capacity(trip_ids.len());
    let mut normalized = Vec::with_capacity(trip_ids.len());
    for trip_id in trip_ids {
        validate_id(&trip_id, "tripIds contains an invalid trip id")?;
        if !seen.insert(trip_id.clone()) {
            return Err(ValidationError("tripIds must not contain duplicates"));
        }
        normalized.push(trip_id);
    }
    normalized.sort();
    Ok(normalized)
}

fn validate_stored_identity(identity: &ServiceIdentity) -> Result<(), ValidationError> {
    validate_id(&identity.id, "stored service identity id is invalid")?;
    required_text(
        identity.name.clone(),
        "stored service identity name is invalid",
        MAX_SERVICE_NAME_CHARS,
    )?;
    if identity.client_id_hint.is_empty()
        || identity.client_id_hint.len() > 16
        || !identity.client_id_hint.is_ascii()
    {
        return Err(ValidationError("stored service client ID hint is invalid"));
    }
    if normalize_scopes(identity.scopes.clone())? != identity.scopes
        || normalize_trip_ids(identity.trip_ids.clone())? != identity.trip_ids
    {
        return Err(ValidationError(
            "stored service scopes or trips are not canonical",
        ));
    }
    let created = parse_utc(
        &identity.created_at,
        "stored service creation time is invalid",
    )?;
    let expires = parse_utc(&identity.expires_at, "stored service expiry is invalid")?;
    if expires <= created || expires - created > Duration::hours(168) {
        return Err(ValidationError("stored service lifetime is invalid"));
    }
    if let Some(last_used) = &identity.last_used_at {
        let last_used = parse_utc(last_used, "stored service last-used time is invalid")?;
        if last_used < created || last_used > expires {
            return Err(ValidationError(
                "stored service last-used time predates creation",
            ));
        }
        if let Some(revoked) = &identity.revoked_at
            && last_used > parse_utc(revoked, "stored service revocation time is invalid")?
        {
            return Err(ValidationError(
                "stored service last-used time follows revocation",
            ));
        }
    }
    if let Some(revoked) = &identity.revoked_at {
        let revoked = parse_utc(revoked, "stored service revocation time is invalid")?;
        if revoked < created {
            return Err(ValidationError(
                "stored service revocation predates creation",
            ));
        }
    }
    Ok(())
}

fn client_id_hint(common_name: &str) -> String {
    common_name.chars().take(8).collect()
}

fn utc_now(clock: &dyn Clock) -> Result<DateTime<Utc>, ValidationError> {
    parse_utc(&clock.now(), "clock returned an invalid UTC timestamp")
}

fn parse_utc(value: &str, message: &'static str) -> Result<DateTime<Utc>, ValidationError> {
    let parsed = DateTime::parse_from_rfc3339(value).map_err(|_| ValidationError(message))?;
    if parsed.offset().local_minus_utc() != 0 {
        return Err(ValidationError(message));
    }
    Ok(parsed.with_timezone(&Utc))
}

fn validate_id(value: &str, message: &'static str) -> Result<(), ValidationError> {
    if value.is_empty()
        || value.len() > 200
        || value.trim() != value
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(ValidationError(message))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;

    struct FixedClock(&'static str);
    impl Clock for FixedClock {
        fn now(&self) -> String {
            self.0.into()
        }
    }

    struct FixedIds(&'static str);
    impl IdGen for FixedIds {
        fn new_id(&self) -> String {
            self.0.into()
        }
    }

    #[derive(Default)]
    struct CapturingRepo(Mutex<Option<NewServiceIdentity>>);

    #[async_trait]
    impl ServiceIdentityRepo for CapturingRepo {
        async fn list_service_identities(
            &self,
            _owner: &UserId,
        ) -> Result<Vec<ServiceIdentity>, ServiceIdentityRepoError> {
            Ok(vec![])
        }

        async fn create_service_identity(
            &self,
            _owner: &UserId,
            new_identity: NewServiceIdentity,
        ) -> Result<ServiceIdentity, ServiceIdentityRepoError> {
            *self.0.lock().expect("capture lock") = Some(new_identity.clone());
            Ok(new_identity.identity)
        }

        async fn revoke_service_identity(
            &self,
            _owner: &UserId,
            _service_id: &str,
            _revoked_at: &str,
        ) -> Result<(), ServiceIdentityRepoError> {
            Ok(())
        }

        async fn authenticate_service(
            &self,
            _common_name: &ServiceCommonName,
            _used_at: &str,
        ) -> Result<ServiceGrant, ServiceIdentityRepoError> {
            Err(ServiceIdentityRepoError::CredentialRejected)
        }
    }

    #[tokio::test]
    async fn registration_normalizes_bounded_scopes_trips_and_expiry_without_returning_a_secret() {
        let repo = CapturingRepo::default();
        let created = register_service_identity(
            &repo,
            &FixedIds("service-a"),
            &FixedClock("2026-08-06T12:00:00Z"),
            &UserId("owner-a".into()),
            RegisterServiceIdentityInput {
                name: "  Claude research  ".into(),
                client_id: "1234567890abcdef1234567890abcdef.access".into(),
                scopes: vec![ServiceScope::Propose, ServiceScope::Read],
                trip_ids: vec!["trip-b".into(), "trip-a".into()],
                ttl_hours: 24,
            },
        )
        .await
        .expect("registration succeeds");

        assert_eq!(created.name, "Claude research");
        assert_eq!(created.client_id_hint, "12345678");
        assert_eq!(
            created.scopes,
            vec![ServiceScope::Read, ServiceScope::Propose]
        );
        assert_eq!(created.trip_ids, vec!["trip-a", "trip-b"]);
        assert_eq!(created.expires_at, "2026-08-07T12:00:00Z");
        assert!(
            !serde_json::to_string(&created)
                .unwrap()
                .contains("1234567890abcdef1234567890abcdef.access")
        );
    }

    #[tokio::test]
    async fn registration_rejects_duplicate_or_empty_authority_and_unsupported_lifetimes() {
        let repo = CapturingRepo::default();
        for input in [
            RegisterServiceIdentityInput {
                name: "assistant".into(),
                client_id: "1234567890abcdef1234567890abcdef.access".into(),
                scopes: vec![ServiceScope::Read, ServiceScope::Read],
                trip_ids: vec!["trip-a".into()],
                ttl_hours: 24,
            },
            RegisterServiceIdentityInput {
                name: "assistant".into(),
                client_id: "1234567890abcdef1234567890abcdef.access".into(),
                scopes: vec![ServiceScope::Read],
                trip_ids: vec![],
                ttl_hours: 24,
            },
            RegisterServiceIdentityInput {
                name: "assistant".into(),
                client_id: "1234567890abcdef1234567890abcdef.access".into(),
                scopes: vec![ServiceScope::Read],
                trip_ids: vec!["trip-a".into()],
                ttl_hours: 2,
            },
        ] {
            assert!(
                register_service_identity(
                    &repo,
                    &FixedIds("service-a"),
                    &FixedClock("2026-08-06T12:00:00Z"),
                    &UserId("owner-a".into()),
                    input,
                )
                .await
                .is_err()
            );
        }
    }
}
