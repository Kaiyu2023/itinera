use async_trait::async_trait;
use itinera_core::{
    domain::user::Email,
    ports::auth::{AuthError, Identity, IdentityProvider},
};

pub struct DevIdentityProvider;

#[async_trait]
impl IdentityProvider for DevIdentityProvider {
    async fn authenticate(&self, assertion: &str) -> Result<Identity, AuthError> {
        if let Ok(email) = Email::parse(assertion) {
            Ok(Identity::Human { email })
        } else {
            Err(AuthError::InvalidToken)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn accepts_an_email_only_when_the_development_feature_is_compiled() {
        let identity = DevIdentityProvider
            .authenticate("  Cloud.Strife@Proton.ME  ")
            .await
            .expect("a valid development identity should authenticate");

        assert!(matches!(
            identity,
            Identity::Human { email } if email.as_str() == "cloud.strife@proton.me"
        ));
        assert_eq!(
            DevIdentityProvider.authenticate("not-an-email").await,
            Err(AuthError::InvalidToken)
        );
    }
}
