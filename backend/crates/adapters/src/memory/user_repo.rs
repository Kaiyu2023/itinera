use std::{collections::HashMap, sync::RwLock};

use async_trait::async_trait;
use itinera_core::{
    domain::user::{Email, User},
    ports::user::{UserRepo, UserRepoError},
};

pub struct InMemoryUserRepo {
    users: RwLock<HashMap<Email, User>>,
}

impl InMemoryUserRepo {
    pub fn new() -> Self {
        InMemoryUserRepo {
            users: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for InMemoryUserRepo {
    fn default() -> Self {
        InMemoryUserRepo::new()
    }
}

#[async_trait]
impl UserRepo for InMemoryUserRepo {
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, UserRepoError> {
        let user = self
            .users
            .read()
            .map_err(|_| UserRepoError::UserRepoUnavailable)?
            .get(email)
            .cloned();
        Ok(user)
    }

    async fn insert(&self, user: User) -> Result<(), UserRepoError> {
        let mut users = self
            .users
            .write()
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;
        if users.contains_key(&user.email) {
            return Err(UserRepoError::DuplicateEmail(user.email));
        }
        users.insert(user.email.clone(), user);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itinera_core::domain::user::UserId;

    fn user(id: &str, email: &str) -> User {
        User {
            id: UserId(id.to_string()),
            email: Email::parse(email).expect("fixture email should parse"),
            display_name: None,
        }
    }

    #[tokio::test]
    async fn insert_then_find_returns_the_user() {
        let repo = InMemoryUserRepo::new();
        let cloud = user("u1", "cloud.strife@proton.me");
        repo.insert(cloud.clone())
            .await
            .expect("insert should succeed");

        let found = repo
            .find_by_email(&cloud.email)
            .await
            .expect("lookup should succeed");

        assert_eq!(found, Some(cloud));
    }

    #[tokio::test]
    async fn find_by_email_is_none_for_an_unknown_email() {
        let repo = InMemoryUserRepo::new();
        let missing = Email::parse("tifa.lockhart@proton.me").expect("should parse");

        let found = repo
            .find_by_email(&missing)
            .await
            .expect("lookup should succeed");

        assert_eq!(found, None);
    }

    #[tokio::test]
    async fn duplicate_insert_is_rejected_and_keeps_the_original() {
        // Mirrors the unique constraint on users.email: the second write must fail
        // *and* leave the stored record untouched.
        let repo = InMemoryUserRepo::new();
        let original = user("u1", "cloud.strife@proton.me");
        let impostor = user("u2", "cloud.strife@proton.me");
        repo.insert(original.clone())
            .await
            .expect("first insert should succeed");

        let err = repo
            .insert(impostor)
            .await
            .expect_err("second insert should be rejected");

        assert!(matches!(err, UserRepoError::DuplicateEmail(_)));
        let found = repo
            .find_by_email(&original.email)
            .await
            .expect("lookup should succeed");
        assert_eq!(found, Some(original));
    }

    #[tokio::test]
    async fn lookup_finds_the_user_whatever_the_spelling() {
        // Access may hand us any casing on a later login; every spelling of one
        // mailbox has to reach the account already stored, or /me provisions a
        // duplicate.
        let repo = InMemoryUserRepo::new();
        let cloud = user("u1", "cloud.strife@proton.me");
        repo.insert(cloud.clone())
            .await
            .expect("insert should succeed");

        let shouted = Email::parse("  Cloud.Strife@Proton.ME  ").expect("should parse");
        let found = repo
            .find_by_email(&shouted)
            .await
            .expect("lookup should succeed");

        assert_eq!(found, Some(cloud));
    }
}
