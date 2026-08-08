use std::path::{Path, PathBuf};

use itinera_adapters::sqlite::{
    SqliteContentHistoryRepo, SqliteDb, SqlitePollRepo, SqliteProposalRepo, SqliteTripRepo,
    SqliteUserRepo,
};
use itinera_core::{
    domain::{
        trip::{NewTripInput, Trip},
        user::{Email, User, UserId},
    },
    ports::{authorization::TripAuthorizationContext, trip::TripRepo, user::UserRepo},
};
use sha2::{Digest, Sha256};
use sqlx::{Connection, SqliteConnection, sqlite::SqliteConnectOptions};
use tempfile::TempDir;

pub const NOW: &str = "2026-08-07T12:00:00.000Z";

pub struct TestDatabase {
    directory: TempDir,
    pub path: PathBuf,
    pub db: SqliteDb,
}

impl TestDatabase {
    pub async fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("itinera-sqlite-")
            .tempdir()
            .expect("create local temporary directory");
        let path = directory.path().join("itinera.db");
        std::fs::File::create(&path).expect("create a real empty SQLite file");
        SqliteDb::migrate(&path).await.expect("apply migrations");
        let db = SqliteDb::open(&path).await.expect("open migrated database");
        Self {
            directory,
            path,
            db,
        }
    }

    pub fn users(&self) -> SqliteUserRepo {
        SqliteUserRepo::new(self.db.clone())
    }

    pub fn trips(&self) -> SqliteTripRepo {
        SqliteTripRepo::new(self.db.clone())
    }

    pub fn history(&self) -> SqliteContentHistoryRepo {
        SqliteContentHistoryRepo::new(self.db.clone())
    }

    pub fn proposals(&self) -> SqliteProposalRepo {
        SqliteProposalRepo::new(self.db.clone())
    }

    pub fn polls(&self) -> SqlitePollRepo {
        SqlitePollRepo::new(self.db.clone())
    }

    pub async fn shutdown(self) {
        self.db.close().await;
        drop(self.db);
        self.directory
            .close()
            .expect("temporary database files close cleanly");
    }
}

pub async fn raw_connection(path: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(path)
            .foreign_keys(true),
    )
    .await
    .expect("open raw test connection")
}

pub fn user(id: &str, email: &str) -> User {
    User {
        id: UserId(id.to_string()),
        email: Email::parse(email).expect("valid fixture email"),
        display_name: Some(id.to_string()),
    }
}

pub async fn seed_user(repo: &SqliteUserRepo, id: &str, email: &str) -> User {
    let value = user(id, email);
    repo.insert(value.clone()).await.expect("seed user");
    value
}

pub fn trip(id: &str, creator: &User) -> Trip {
    Trip::create(NewTripInput {
        id: id.to_string(),
        name: format!("Trip {id}"),
        start_date: "2026-08-07".to_string(),
        end_date: "2026-08-09".to_string(),
        base_currency: "GBP".to_string(),
        creator_id: creator.id.0.clone(),
        created_at: NOW.to_string(),
    })
    .expect("valid fixture trip")
}

pub async fn seed_trip(repo: &SqliteTripRepo, id: &str, creator: &User) -> Trip {
    repo.create_trip(
        &TripAuthorizationContext::human(creator.id.clone()),
        trip(id, creator),
    )
    .await
    .expect("seed trip")
}

pub fn digest(email: &str) -> String {
    let bytes = Sha256::digest(email.as_bytes());
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
