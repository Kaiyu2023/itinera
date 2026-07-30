use async_trait::async_trait;
use itinera_core::{
    domain::user::Email,
    ports::identity::{AuthError, Identity, IdentityProvider},
};

pub struct DevIdentityProvider;

#[async_trait]
impl IdentityProvider for DevIdentityProvider {
    async fn authenticate(&self, assertion: &str) -> Result<Identity, AuthError> {
        if let Ok(email) = Email::parse(assertion) {
            Ok(Identity { email })
        } else {
            Err(AuthError::InvalidToken)
        }
    }
}
