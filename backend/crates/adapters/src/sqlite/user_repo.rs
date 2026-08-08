//! SQLite implementation of stable users and canonical email claims.

use async_trait::async_trait;
use itinera_core::{
    domain::user::{Email, User, UserId},
    ports::user::{UserRepo, UserRepoError},
};
use sqlx::{FromRow, Sqlite, Transaction};

use super::{
    SqliteDb,
    codec::{CodecError, checked_revision, email_digest, validate_id, validate_optional_text},
    row::SqliteRowExt,
};

#[derive(Debug, FromRow)]
struct UserRow {
    id: String,
    user_id: Option<String>,
    email: String,
    display_name: Option<String>,
    revision: i64,
    email_digest: Option<String>,
}

impl UserRow {
    fn into_user(
        self,
        expected_email: Option<&Email>,
        expected_digest: Option<&str>,
    ) -> Result<User, UserRepoError> {
        let email = Email::parse_canonical(&self.email).map_err(|_| UserRepoError::CorruptData)?;
        validate_id(&self.id).map_err(map_codec)?;
        validate_optional_text(self.display_name.as_deref(), 200).map_err(map_codec)?;
        checked_revision(self.revision).map_err(map_codec)?;
        let digest = email_digest(&email);
        if self.user_id.as_deref() != Some(self.id.as_str())
            || self.email_digest.as_deref() != Some(digest.as_str())
            || expected_email.is_some_and(|expected| expected != &email)
            || expected_digest.is_some_and(|expected| expected != digest)
        {
            return Err(UserRepoError::CorruptData);
        }
        Ok(User {
            id: UserId(self.id),
            email,
            display_name: self.display_name,
        })
    }
}

#[derive(Clone)]
pub struct SqliteUserRepo {
    db: SqliteDb,
}

impl SqliteUserRepo {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }

    pub fn db(&self) -> &SqliteDb {
        &self.db
    }
}

#[async_trait]
impl UserRepo for SqliteUserRepo {
    async fn find_by_email(&self, email: &Email) -> Result<Option<User>, UserRepoError> {
        let digest = email_digest(email);
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;
        let row = sqlx::query(
            "SELECT u.id, u.email, u.display_name, u.revision, c.email_digest, c.user_id \
             FROM user_email_claims AS c \
             LEFT JOIN users AS u ON u.id = c.user_id \
             WHERE c.email_digest = ?",
        )
        .bind(&digest)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| UserRepoError::UserRepoUnavailable)?;

        let result = match row {
            Some(row) => Some(
                row.decode::<UserRow>()?
                    .into_user(Some(email), Some(&digest))?,
            ),
            None => {
                let orphan: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email = ?")
                    .bind(email.as_str())
                    .fetch_one(&mut *transaction)
                    .await
                    .map_err(|_| UserRepoError::UserRepoUnavailable)?;
                if orphan != 0 {
                    return Err(UserRepoError::CorruptData);
                }
                None
            }
        };
        self.db
            .commit(transaction)
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;
        Ok(result)
    }

    async fn find_by_id(&self, user_id: &UserId) -> Result<Option<User>, UserRepoError> {
        validate_id(&user_id.0).map_err(map_codec)?;
        let mut transaction = self
            .db
            .pool()
            .begin()
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;
        let row = sqlx::query(
            "SELECT u.id, u.email, u.display_name, u.revision, c.email_digest, c.user_id \
             FROM users AS u \
             LEFT JOIN user_email_claims AS c ON c.user_id = u.id \
             WHERE u.id = ?",
        )
        .bind(&user_id.0)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|_| UserRepoError::UserRepoUnavailable)?;
        let result = row
            .as_ref()
            .map(|row| row.decode::<UserRow>()?.into_user(None, None))
            .transpose()?;
        self.db
            .commit(transaction)
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;
        Ok(result)
    }

    async fn insert(&self, user: User) -> Result<(), UserRepoError> {
        validate_user(&user)?;
        let digest = email_digest(&user.email);
        let mut transaction = self
            .db
            .begin_immediate()
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;

        if let Some(row) = claim_row(&mut transaction, &digest).await? {
            row.decode::<UserRow>()?
                .into_user(Some(&user.email), Some(&digest))?;
            return Err(UserRepoError::DuplicateEmail(user.email));
        }

        let existing_id: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE id = ?")
            .bind(&user.id.0)
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;
        let existing_email: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE email = ?")
            .bind(user.email.as_str())
            .fetch_one(&mut *transaction)
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;
        if existing_email != 0 {
            return Err(UserRepoError::CorruptData);
        }
        if existing_id != 0 {
            return Err(UserRepoError::UserRepoUnavailable);
        }

        sqlx::query("INSERT INTO users (id, email, display_name, revision) VALUES (?, ?, ?, 1)")
            .bind(&user.id.0)
            .bind(user.email.as_str())
            .bind(&user.display_name)
            .execute(&mut *transaction)
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;
        sqlx::query("INSERT INTO user_email_claims (email_digest, user_id) VALUES (?, ?)")
            .bind(&digest)
            .bind(&user.id.0)
            .execute(&mut *transaction)
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)?;
        self.db
            .commit(transaction)
            .await
            .map_err(|_| UserRepoError::UserRepoUnavailable)
    }
}

async fn claim_row(
    transaction: &mut Transaction<'static, Sqlite>,
    digest: &str,
) -> Result<Option<sqlx::sqlite::SqliteRow>, UserRepoError> {
    sqlx::query(
        "SELECT u.id, u.email, u.display_name, u.revision, c.email_digest, c.user_id \
         FROM user_email_claims AS c \
         LEFT JOIN users AS u ON u.id = c.user_id \
         WHERE c.email_digest = ?",
    )
    .bind(digest)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(|_| UserRepoError::UserRepoUnavailable)
}

fn validate_user(user: &User) -> Result<(), UserRepoError> {
    validate_id(&user.id.0).map_err(map_codec)?;
    validate_optional_text(user.display_name.as_deref(), 200).map_err(map_codec)
}

fn map_codec(_error: CodecError) -> UserRepoError {
    UserRepoError::CorruptData
}
