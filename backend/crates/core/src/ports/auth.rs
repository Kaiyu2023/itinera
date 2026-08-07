use async_trait::async_trait;

use crate::domain::user::Email;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Identity {
    Human { email: Email },
    Service { common_name: ServiceCommonName },
}

/// The stable identifier Cloudflare places in a service assertion's
/// `common_name` claim. It is a credential identifier, not a human email.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceCommonName(String);

impl ServiceCommonName {
    pub fn parse(raw: &str) -> Result<Self, AuthError> {
        const SUFFIX: &str = ".access";
        let client_id = raw.strip_suffix(SUFFIX).ok_or(AuthError::InvalidToken)?;
        if client_id.len() != 32
            || !client_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(AuthError::InvalidToken);
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error("the access assertion is malformed or its signature is invalid")]
    InvalidToken,
    #[error("the access token has expired")]
    ExpiredToken,
    #[error("the identity provider is unavailable")]
    IdentityProviderUnavailable,
}

#[async_trait]
pub trait IdentityProvider: Send + Sync {
    async fn authenticate(&self, assertion: &str) -> Result<Identity, AuthError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflare_service_common_names_have_the_exact_client_id_shape() {
        let valid = "e367826f93b8d71185e03fe518aff3b4.access";
        assert_eq!(ServiceCommonName::parse(valid).unwrap().as_str(), valid);

        for invalid in [
            "e367826f93b8d71185e03fe518aff3b4",
            "E367826F93B8D71185E03FE518AFF3B4.access",
            "e367826f93b8d71185e03fe518aff3bg.access",
            "service-client-id.access",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            assert_eq!(
                ServiceCommonName::parse(invalid),
                Err(AuthError::InvalidToken),
                "accepted {invalid:?}"
            );
        }
    }
}
