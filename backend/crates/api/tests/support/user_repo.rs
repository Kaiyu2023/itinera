//! User repository fake scoped to API route tests.

use std::{collections::HashMap, sync::RwLock};

use async_trait::async_trait;
use itinera_core::{
    domain::user::{Email, User, UserId},
    ports::user::{UserRepo, UserRepoError},
};

pub struct TestUserRepo {
    users: RwLock<HashMap<Email, User>>,
}

impl TestUserRepo {
    pub fn new() -> Self {
        TestUserRepo {
            users: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for TestUserRepo {
    fn default() -> Self {
        TestUserRepo::new()
    }
}

#[async_trait]
impl UserRepo for TestUserRepo {
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, UserRepoError> {
        let user = self
            .users
            .read()
            .map_err(|_| UserRepoError::UserRepoUnavailable)?
            .get(email)
            .cloned();
        Ok(user)
    }

    async fn find_by_id(&self, user_id: &UserId) -> Result<Option<User>, UserRepoError> {
        let user = self
            .users
            .read()
            .map_err(|_| UserRepoError::UserRepoUnavailable)?
            .values()
            .find(|user| &user.id == user_id)
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
