use std::{path::Path, sync::Arc, time::Duration};

use itinera_adapters::sqlite::{
    EXPECTED_SCHEMA_VERSION, EXPECTED_SQLITE_SOURCE_ID, EXPECTED_SQLITE_VERSION,
    SQLITE_BUSY_TIMEOUT_MILLIS, SQLITE_POOL_MAX_CONNECTIONS, SQLITE_WAL_AUTOCHECKPOINT_PAGES,
    SqliteDb, SqliteDbError,
};
use itinera_core::ports::user::UserRepo;
use sqlx::Connection;
use tokio::sync::Barrier;

use super::support::{TestDatabase, seed_trip, seed_user};

#[tokio::test]
async fn migration_from_a_real_empty_file_opens_the_pinned_bounded_pool() {
    let database = TestDatabase::new().await;
    assert!(database.path.is_absolute());
    assert!(database.path.is_file());
    assert_eq!(
        database.db.path(),
        database.path.canonicalize().expect("canonical test path")
    );
    assert_eq!(database.db.engine().version, EXPECTED_SQLITE_VERSION);
    assert_eq!(database.db.engine().source_id, EXPECTED_SQLITE_SOURCE_ID);
    assert_eq!(database.db.pool().size(), SQLITE_POOL_MAX_CONNECTIONS);

    let barrier = Arc::new(Barrier::new(SQLITE_POOL_MAX_CONNECTIONS as usize + 1));
    let mut tasks = Vec::new();
    for _ in 0..SQLITE_POOL_MAX_CONNECTIONS {
        let pool = database.db.pool().clone();
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            let mut connection = pool.acquire().await.expect("acquire configured connection");
            let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
                .fetch_one(&mut *connection)
                .await
                .expect("read foreign_keys");
            let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
                .fetch_one(&mut *connection)
                .await
                .expect("read journal_mode");
            let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
                .fetch_one(&mut *connection)
                .await
                .expect("read synchronous");
            let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
                .fetch_one(&mut *connection)
                .await
                .expect("read busy_timeout");
            let autocheckpoint: i64 = sqlx::query_scalar("PRAGMA wal_autocheckpoint")
                .fetch_one(&mut *connection)
                .await
                .expect("read wal_autocheckpoint");
            barrier.wait().await;
            (
                foreign_keys,
                journal_mode,
                synchronous,
                busy_timeout,
                autocheckpoint,
            )
        }));
    }
    tokio::time::timeout(Duration::from_secs(5), barrier.wait())
        .await
        .expect("four connections are concurrently available");
    for task in tasks {
        let values = task.await.expect("connection task");
        assert_eq!(values.0, 1);
        assert_eq!(values.1.to_ascii_lowercase(), "wal");
        assert_eq!(values.2, 2);
        assert_eq!(values.3, SQLITE_BUSY_TIMEOUT_MILLIS as i64);
        assert_eq!(values.4, SQLITE_WAL_AUTOCHECKPOINT_PAGES as i64);
    }
    database.db.validate().await.expect("readiness validation");
    database.shutdown().await;
}

#[tokio::test]
async fn normal_open_never_auto_migrates_an_empty_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("empty.db");
    std::fs::File::create(&path).expect("empty file");

    let result = SqliteDb::open(&path).await;
    assert!(matches!(result, Err(SqliteDbError::MigrationMismatch)));

    let mut connection = super::support::raw_connection(&path).await;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .expect("read version");
    let migration_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master \
         WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_one(&mut connection)
    .await
    .expect("inspect schema");
    assert_eq!(version, 0);
    assert_eq!(migration_table, 0);
    connection.close().await.expect("close raw connection");
    directory.close().expect("remove temp directory");
}

#[tokio::test]
async fn startup_rejects_checksum_and_schema_version_drift() {
    let database = TestDatabase::new().await;
    sqlx::query("UPDATE _sqlx_migrations SET checksum = X'00'")
        .execute(database.db.pool())
        .await
        .expect("tamper checksum");
    database.db.close().await;
    assert!(matches!(
        SqliteDb::open(&database.path).await,
        Err(SqliteDbError::MigrationMismatch)
    ));
    database.shutdown().await;

    let database = TestDatabase::new().await;
    sqlx::query("PRAGMA user_version = 2")
        .execute(database.db.pool())
        .await
        .expect("tamper schema version");
    database.db.close().await;
    assert!(matches!(
        SqliteDb::open(&database.path).await,
        Err(SqliteDbError::MigrationMismatch)
    ));
    database.shutdown().await;
}

#[tokio::test]
async fn explicit_migration_is_repeatable_but_requires_an_absolute_local_file() {
    assert!(matches!(
        SqliteDb::migrate(Path::new("relative.db")).await,
        Err(SqliteDbError::InvalidPath)
    ));
    assert!(matches!(
        SqliteDb::open(Path::new(":memory:")).await,
        Err(SqliteDbError::InvalidPath)
    ));
    #[cfg(windows)]
    assert!(matches!(
        SqliteDb::migrate(Path::new(r"\\server\share\itinera.db")).await,
        Err(SqliteDbError::NetworkFilesystem)
    ));

    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("repeat.db");
    std::fs::File::create(&path).expect("empty file");
    SqliteDb::migrate(&path).await.expect("first migration");
    SqliteDb::migrate(&path).await.expect("second migration");
    let database = SqliteDb::open(&path).await.expect("open migrated fixture");
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(database.pool())
        .await
        .expect("read schema version");
    assert_eq!(version, EXPECTED_SCHEMA_VERSION);
    database.close().await;
    directory.close().expect("remove temp directory");
}

#[tokio::test]
async fn strict_schema_rejects_null_wrong_type_foreign_key_enum_date_and_json_rows() {
    let database = TestDatabase::new().await;
    let users = database.users();
    let trips = database.trips();
    let leader = seed_user(&users, "leader", "leader@example.com").await;
    seed_trip(&trips, "trip-a", &leader).await;

    let strict_rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT name, strict FROM pragma_table_list \
         WHERE name IN ('users', 'user_email_claims', 'trips', 'trip_memberships', 'trip_invites') \
         ORDER BY name",
    )
    .fetch_all(database.db.pool())
    .await
    .expect("inspect strict tables");
    assert_eq!(strict_rows.len(), 5);
    assert!(strict_rows.iter().all(|(_, strict)| *strict == 1));

    for statement in [
        "INSERT INTO users (id, email, revision) VALUES (NULL, 'null@example.com', 1)",
        "INSERT INTO users (id, email, revision) VALUES (X'CAFE', 'blob@example.com', 1)",
        "INSERT INTO trip_memberships (trip_id, user_id, role, joined_at, revision) VALUES ('missing', 'leader', 'member', '2026-08-07T12:00:00Z', 1)",
        "INSERT INTO trip_memberships (trip_id, user_id, role, joined_at, revision) VALUES ('trip-a', 'leader', 'owner', '2026-08-07T12:00:00Z', 1)",
        "UPDATE trips SET start_date = '2026-02-30' WHERE id = 'trip-a'",
        "UPDATE trips SET stop_kind_labels_json = '{broken' WHERE id = 'trip-a'",
        "INSERT INTO trip_invites (trip_id, email_digest, id, email, invited_by, status, created_at, revision) VALUES ('trip-a', 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'bad', 'bad@example.com', 'missing-user', 'pending', '2026-08-07T12:00:00Z', 1)",
    ] {
        assert!(
            sqlx::query(statement)
                .execute(database.db.pool())
                .await
                .is_err(),
            "schema unexpectedly accepted {statement}"
        );
    }

    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(database.db.pool())
        .await
        .expect("integrity check");
    let foreign_keys: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
        .fetch_one(database.db.pool())
        .await
        .expect("foreign-key check");
    assert_eq!(integrity, "ok");
    assert_eq!(foreign_keys, 0);

    drop(trips);
    drop(users);
    database.shutdown().await;
}

#[tokio::test]
async fn bundled_wal_runtime_handles_concurrent_writers_and_passive_checkpoints() {
    let database = TestDatabase::new().await;
    let barrier = Arc::new(Barrier::new(25));
    let mut writers = Vec::new();
    for index in 0..24 {
        let repo = database.users();
        let barrier = barrier.clone();
        writers.push(tokio::spawn(async move {
            barrier.wait().await;
            repo.insert(super::support::user(
                &format!("checkpoint-user-{index:02}"),
                &format!("checkpoint-{index:02}@example.com"),
            ))
            .await
        }));
    }
    let pool = database.db.pool().clone();
    let checkpoint = tokio::spawn(async move {
        barrier.wait().await;
        for _ in 0..16 {
            let _: (i64, i64, i64) = sqlx::query_as("PRAGMA wal_checkpoint(PASSIVE)")
                .fetch_one(&pool)
                .await
                .expect("passive checkpoint");
            tokio::task::yield_now().await;
        }
    });
    for writer in writers {
        writer
            .await
            .expect("writer task")
            .expect("serialized write");
    }
    checkpoint.await.expect("checkpoint task");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(database.db.pool())
        .await
        .unwrap();
    assert_eq!(count, 24);
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(database.db.pool())
        .await
        .unwrap();
    assert_eq!(integrity, "ok");
    database.shutdown().await;
}
