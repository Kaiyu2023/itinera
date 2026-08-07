use crate::{
    domain::user::{Email, User, UserId},
    ports::{
        id_gen::IdGen,
        user::{UserRepo, UserRepoError},
    },
};

#[derive(Debug, Clone, thiserror::Error)]
pub enum ProvisionError {
    #[error("User storage error")]
    RepoError(#[from] UserRepoError),
    #[error("User record not found after conflict")]
    VanishedAfterConflict(Email),
}

pub async fn get_or_provision(
    repo: &dyn UserRepo,
    id_gen: &dyn IdGen,
    email: Email,
) -> Result<User, ProvisionError> {
    if let Some(user) = repo.find_by_email(&email).await? {
        return Ok(user);
    }
    let new_user = User {
        id: UserId(id_gen.new_id()),
        email,
        display_name: None,
    };
    match repo.insert(new_user.clone()).await {
        Ok(()) => Ok(new_user),
        Err(UserRepoError::DuplicateEmail(_)) => repo
            .find_by_email(&new_user.email)
            .await?
            .ok_or(ProvisionError::VanishedAfterConflict(new_user.email)),
        Err(other) => Err(other.into()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;

    struct FixedIdGen(&'static str);

    impl IdGen for FixedIdGen {
        fn new_id(&self) -> String {
            self.0.to_string()
        }
    }

    /// Scripted rather than stateful: each test declares what successive lookups
    /// return and what `insert` does. A store that merely holds users could never
    /// reach the conflict branches, which need the email absent at lookup time and
    /// present at insert time.
    struct FakeUserRepo {
        lookups: Mutex<VecDeque<Result<Option<User>, UserRepoError>>>,
        insert_result: Result<(), UserRepoError>,
        inserted: Mutex<Vec<User>>,
    }

    impl FakeUserRepo {
        fn new(
            lookups: Vec<Result<Option<User>, UserRepoError>>,
            insert_result: Result<(), UserRepoError>,
        ) -> Self {
            Self {
                lookups: Mutex::new(lookups.into()),
                insert_result,
                inserted: Mutex::new(Vec::new()),
            }
        }

        fn empty() -> Self {
            Self::new(vec![Ok(None)], Ok(()))
        }

        fn holding(existing: User) -> Self {
            Self::new(vec![Ok(Some(existing))], Ok(()))
        }

        fn losing_race_to(winner: User) -> Self {
            Self::new(vec![Ok(None), Ok(Some(winner))], Err(duplicate()))
        }

        fn losing_race_then_empty() -> Self {
            Self::new(vec![Ok(None), Ok(None)], Err(duplicate()))
        }

        fn unavailable() -> Self {
            Self::new(vec![Err(UserRepoError::UserRepoUnavailable)], Ok(()))
        }

        fn inserted(&self) -> Vec<User> {
            self.inserted.lock().expect("lock poisoned").clone()
        }
    }

    #[async_trait]
    impl UserRepo for FakeUserRepo {
        async fn find_by_email(&self, _email: &Email) -> Result<Option<User>, UserRepoError> {
            self.lookups
                .lock()
                .expect("lock poisoned")
                .pop_front()
                .expect("more lookups than this test scripted")
        }

        async fn find_by_id(&self, _user_id: &UserId) -> Result<Option<User>, UserRepoError> {
            panic!("provisioning never looks users up by id")
        }

        async fn insert(&self, user: User) -> Result<(), UserRepoError> {
            self.inserted.lock().expect("lock poisoned").push(user);
            self.insert_result.clone()
        }
    }

    const ADDRESS: &str = "cloud.strife@proton.me";

    fn email() -> Email {
        Email::parse(ADDRESS).expect("fixture email should parse")
    }

    fn duplicate() -> UserRepoError {
        UserRepoError::DuplicateEmail(email())
    }

    fn user(id: &str) -> User {
        User {
            id: UserId(id.to_string()),
            email: email(),
            display_name: None,
        }
    }

    #[tokio::test]
    async fn returns_the_existing_user_without_inserting() {
        let existing = user("stored");
        let repo = FakeUserRepo::holding(existing.clone());

        let got = get_or_provision(&repo, &FixedIdGen("generated"), email())
            .await
            .expect("lookup of an existing user should succeed");

        assert_eq!(got, existing);
        assert!(
            repo.inserted().is_empty(),
            "an existing user must not be written again"
        );
    }

    #[tokio::test]
    async fn provisions_an_unknown_email_with_a_generated_id() {
        let repo = FakeUserRepo::empty();

        let got = get_or_provision(&repo, &FixedIdGen("generated"), email())
            .await
            .expect("provisioning should succeed");

        assert_eq!(got.id, UserId("generated".to_string()));
        assert_eq!(got.email, email());
        assert_eq!(got.display_name, None, "the name is prompted after login");
        assert_eq!(repo.inserted(), vec![got]);
    }

    #[tokio::test]
    async fn a_lost_race_returns_the_winning_record() {
        // Another request provisioned the same person between our lookup and our
        // insert. That is benign: the stored record wins and ours is discarded.
        let winner = user("winner");
        let repo = FakeUserRepo::losing_race_to(winner.clone());

        let got = get_or_provision(&repo, &FixedIdGen("ours"), email())
            .await
            .expect("a lost race is not a failure");

        assert_eq!(got, winner);
        assert_ne!(got.id, UserId("ours".to_string()));
    }

    #[tokio::test]
    async fn a_conflict_with_nothing_stored_afterwards_is_an_error() {
        let repo = FakeUserRepo::losing_race_then_empty();

        let err = get_or_provision(&repo, &FixedIdGen("ours"), email())
            .await
            .expect_err("an unstored user must never be reported as provisioned");

        assert!(matches!(err, ProvisionError::VanishedAfterConflict(_)));
    }

    #[tokio::test]
    async fn storage_failure_propagates() {
        let repo = FakeUserRepo::unavailable();

        let err = get_or_provision(&repo, &FixedIdGen("generated"), email())
            .await
            .expect_err("an unavailable store should fail");

        assert!(matches!(
            err,
            ProvisionError::RepoError(UserRepoError::UserRepoUnavailable)
        ));
    }
}
