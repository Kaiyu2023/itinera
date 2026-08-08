use std::{path::Path, sync::Arc, time::Duration};

use itinera_adapters::sqlite::{
    EXPECTED_SCHEMA_VERSION, EXPECTED_SQLITE_SOURCE_ID, EXPECTED_SQLITE_VERSION,
    SQLITE_BUSY_TIMEOUT_MILLIS, SQLITE_POOL_MAX_CONNECTIONS, SQLITE_WAL_AUTOCHECKPOINT_PAGES,
    SqliteDb, SqliteDbError,
};
use itinera_core::ports::user::UserRepo;
use sqlx::{
    AssertSqlSafe, Connection, SqlSafeStr,
    migrate::{Migration, MigrationType, Migrator},
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tokio::sync::Barrier;

use super::support::{TestDatabase, seed_trip, seed_user};

type PlanProvenanceColumns = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn migrations_through_three() -> Migrator {
    Migrator::with_migrations(vec![
        Migration::new(
            1,
            "users trips".into(),
            MigrationType::Simple,
            include_str!("../../migrations/0001_users_trips.sql").into_sql_str(),
            false,
        ),
        Migration::new(
            2,
            "candidates plans".into(),
            MigrationType::Simple,
            include_str!("../../migrations/0002_candidates_plans.sql").into_sql_str(),
            true,
        ),
        Migration::new(
            3,
            "content history".into(),
            MigrationType::Simple,
            include_str!("../../migrations/0003_content_history.sql").into_sql_str(),
            false,
        ),
    ])
}

fn migrations_through_four() -> Migrator {
    let mut migrations = migrations_through_three()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    migrations.push(Migration::new(
        4,
        "proposals polls".into(),
        MigrationType::Simple,
        include_str!("../../migrations/0004_proposals_polls.sql").into_sql_str(),
        true,
    ));
    Migrator::with_migrations(migrations)
}

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
    let drifted_version = format!("PRAGMA user_version = {}", EXPECTED_SCHEMA_VERSION + 1);
    sqlx::query(AssertSqlSafe(drifted_version))
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
async fn migration_two_upgrades_a_real_version_one_file_without_losing_trip_data() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("upgrade.db");
    std::fs::File::create(&path).expect("real SQLite file");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .expect("open version-one pool");
    Migrator::with_migrations(vec![Migration::new(
        1,
        "users trips".into(),
        MigrationType::Simple,
        include_str!("../../migrations/0001_users_trips.sql").into_sql_str(),
        false,
    )])
    .run(&pool)
    .await
    .expect("apply only migration one");
    sqlx::query(
        "INSERT INTO users (id, email, display_name, revision) \
         VALUES ('leader', 'leader@example.com', 'Leader', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_email_claims (email_digest, user_id) \
         VALUES ('0e67f001ed4530dbe3e5065e73d9472163ca629f367c7bd1907c18893b36ffa9', 'leader')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO trips ( \
             id, name, status, start_date, end_date, base_currency, created_at, revision \
         ) VALUES ( \
             'trip-a', 'Trip A', 'dreaming', '2026-08-07', '2026-08-09', \
             'GBP', '2026-08-07T12:00:00Z', 1 \
         )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO trip_memberships (trip_id, user_id, role, joined_at, revision) \
         VALUES ('trip-a', 'leader', 'leader', '2026-08-07T12:00:00Z', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    SqliteDb::migrate(&path)
        .await
        .expect("upgrade to migration two");
    let database = SqliteDb::open(&path).await.expect("open upgraded database");
    let preserved: (String, String, i64) = sqlx::query_as(
        "SELECT t.name, m.role, t.revision \
         FROM trips AS t JOIN trip_memberships AS m ON m.trip_id = t.id \
         WHERE t.id = 'trip-a' AND m.user_id = 'leader'",
    )
    .fetch_one(database.pool())
    .await
    .expect("preserved trip and membership");
    assert_eq!(preserved, ("Trip A".into(), "leader".into(), 1));
    let strict_tables: Vec<(String, i64)> = sqlx::query_as(
        "SELECT name, strict FROM pragma_table_list \
         WHERE name IN ( \
             'trip_places', 'candidates', 'plans', 'plan_days', \
             'stop_identities', 'plan_stops' \
         ) ORDER BY name",
    )
    .fetch_all(database.pool())
    .await
    .unwrap();
    assert_eq!(strict_tables.len(), 6);
    assert!(strict_tables.iter().all(|(_, strict)| *strict == 1));
    let exact_pointer_columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_foreign_key_list('trips') WHERE \"table\" = 'plans'",
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert_eq!(exact_pointer_columns, 3);
    database.close().await;
    directory.close().expect("remove upgraded fixture");
}

#[tokio::test]
async fn later_migrations_upgrade_a_real_version_two_file_without_rewriting_existing_capabilities()
{
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("history-upgrade.db");
    std::fs::File::create(&path).expect("real SQLite file");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .expect("open version-two pool");
    Migrator::with_migrations(vec![
        Migration::new(
            1,
            "users trips".into(),
            MigrationType::Simple,
            include_str!("../../migrations/0001_users_trips.sql").into_sql_str(),
            false,
        ),
        Migration::new(
            2,
            "candidates plans".into(),
            MigrationType::Simple,
            include_str!("../../migrations/0002_candidates_plans.sql").into_sql_str(),
            true,
        ),
    ])
    .run(&pool)
    .await
    .expect("apply migrations one and two");
    sqlx::query(
        "INSERT INTO users (id, email, display_name, revision) \
         VALUES ('leader', 'leader@example.com', 'Leader', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO trips ( \
             id, name, status, start_date, end_date, base_currency, created_at, revision \
         ) VALUES ( \
             'trip-a', 'Trip A', 'dreaming', '2026-08-07', '2026-08-09', \
             'GBP', '2026-08-07T12:00:00Z', 7 \
         )",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    SqliteDb::migrate(&path)
        .await
        .expect("upgrade version two through discussions");
    let database = SqliteDb::open(&path).await.expect("open upgraded database");
    let preserved: (String, i64) =
        sqlx::query_as("SELECT name, revision FROM trips WHERE id = 'trip-a'")
            .fetch_one(database.pool())
            .await
            .unwrap();
    let schema: (i64, i64, i64) = sqlx::query_as(
        "SELECT \
             (SELECT user_version FROM pragma_user_version), \
             (SELECT strict FROM pragma_table_list WHERE name = 'content_edits'), \
             (SELECT COUNT(*) FROM _sqlx_migrations)",
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert_eq!(preserved, ("Trip A".into(), 7));
    assert_eq!(schema, (5, 1, 5));
    database.close().await;
    directory.close().expect("remove upgraded fixture");
}

#[tokio::test]
async fn migration_four_preserves_a_real_version_three_current_plan_and_adds_strict_governance() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("governance-upgrade.db");
    std::fs::File::create(&path).expect("real SQLite file");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .expect("open version-three pool");
    migrations_through_three()
        .run(&pool)
        .await
        .expect("apply migrations one through three");
    sqlx::query(
        "INSERT INTO users (id, email, display_name, revision) \
         VALUES ('leader', 'leader@example.com', 'Leader', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO trips ( \
             id, name, status, start_date, end_date, base_currency, created_at, revision \
         ) VALUES ( \
             'trip-a', 'Trip A', 'dreaming', '2026-08-07', '2026-08-09', \
             'GBP', '2026-08-07T12:00:00Z', 3 \
         )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO trip_memberships (trip_id, user_id, role, joined_at, revision) \
         VALUES ('trip-a', 'leader', 'leader', '2026-08-07T12:00:00Z', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO plans ( \
             trip_id, version, id, created_from_proposal_id, created_at, revision \
         ) VALUES ('trip-a', 1, 'plan-1', NULL, '2026-08-07T12:00:00Z', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE trips SET current_plan_id = 'plan-1', current_plan_version = 1 \
         WHERE id = 'trip-a'",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    SqliteDb::migrate(&path)
        .await
        .expect("upgrade version three through discussions");
    let database = SqliteDb::open(&path).await.expect("open upgraded database");
    let current: (String, i64, String, i64) = sqlx::query_as(
        "SELECT t.current_plan_id, t.current_plan_version, p.id, p.revision \
         FROM trips AS t \
         JOIN plans AS p \
           ON p.trip_id = t.id \
          AND p.id = t.current_plan_id \
          AND p.version = t.current_plan_version \
         WHERE t.id = 'trip-a'",
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert_eq!(current, ("plan-1".into(), 1, "plan-1".into(), 1));
    let provenance: PlanProvenanceColumns = sqlx::query_as(
        "SELECT applied_change_set_json, application_entity_ids_json, \
                structural_audits_json, base_structure_hash, structure_hash \
         FROM plans WHERE trip_id = 'trip-a' AND version = 1",
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert_eq!(provenance, (None, None, None, None, None));
    let provenance_columns = sqlx::query_scalar::<_, String>(
        "SELECT name FROM pragma_table_info('plans') \
         WHERE name IN ( \
             'applied_change_set_json', 'application_entity_ids_json', \
             'structural_audits_json', 'base_structure_hash', 'structure_hash' \
         ) ORDER BY cid",
    )
    .fetch_all(database.pool())
    .await
    .unwrap();
    assert_eq!(
        provenance_columns,
        [
            "applied_change_set_json",
            "application_entity_ids_json",
            "structural_audits_json",
            "base_structure_hash",
            "structure_hash",
        ]
    );
    let strict_tables: Vec<(String, i64)> = sqlx::query_as(
        "SELECT name, strict FROM pragma_table_list \
         WHERE name IN ( \
             'proposals', 'polls', 'poll_options', 'poll_ballots', \
             'poll_ballot_options', 'proposal_content_edits' \
         ) ORDER BY name",
    )
    .fetch_all(database.pool())
    .await
    .unwrap();
    assert_eq!(strict_tables.len(), 6);
    assert!(strict_tables.iter().all(|(_, strict)| *strict == 1));
    let replacement_provenance: (i64, i64) = sqlx::query_as(
        "SELECT \
             (SELECT COUNT(*) FROM pragma_table_info('polls') \
              WHERE name = 'replaces_poll_id'), \
             (SELECT COUNT(*) FROM pragma_foreign_key_list('polls') \
              WHERE \"table\" = 'polls' AND \"from\" = 'replaces_poll_id')",
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert_eq!(replacement_provenance, (1, 1));
    let schema: (i64, i64, i64) = sqlx::query_as(
        "SELECT \
             (SELECT user_version FROM pragma_user_version), \
             (SELECT COUNT(*) FROM _sqlx_migrations), \
             (SELECT COUNT(*) FROM pragma_foreign_key_list('plans') \
              WHERE \"table\" = 'proposals')",
    )
    .fetch_one(database.pool())
    .await
    .unwrap();
    assert_eq!(schema, (5, 5, 2));
    database.close().await;
    directory.close().expect("remove upgraded fixture");
}

#[tokio::test]
async fn migration_five_preserves_version_four_data_and_adds_strict_discussions() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("discussion-upgrade.db");
    std::fs::File::create(&path).expect("real SQLite file");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .expect("open version-four pool");
    migrations_through_four()
        .run(&pool)
        .await
        .expect("apply migrations one through four");
    sqlx::query(
        "INSERT INTO users (id, email, display_name, revision) \
         VALUES ('leader', 'leader@example.com', 'Leader', 1)",
    )
    .execute(&pool)
    .await
    .expect("seed retained user");
    sqlx::query(
        "INSERT INTO trips ( \
             id, name, status, start_date, end_date, base_currency, created_at, revision \
         ) VALUES ( \
             'trip-a', 'Trip A', 'dreaming', '2026-08-07', '2026-08-09', \
             'GBP', '2026-08-07T12:00:00Z', 7 \
         )",
    )
    .execute(&pool)
    .await
    .expect("seed retained trip");
    pool.close().await;

    SqliteDb::migrate(&path)
        .await
        .expect("upgrade version four through discussions");
    let database = SqliteDb::open(&path).await.expect("open upgraded database");
    let retained: (String, i64) =
        sqlx::query_as("SELECT name, revision FROM trips WHERE id = 'trip-a'")
            .fetch_one(database.pool())
            .await
            .expect("read retained trip");
    assert_eq!(retained, ("Trip A".into(), 7));
    let tables: Vec<(String, i64)> = sqlx::query_as(
        "SELECT name, strict FROM pragma_table_list \
         WHERE name IN ('discussion_threads', 'discussion_comments', 'comment_reactions') \
         ORDER BY name",
    )
    .fetch_all(database.pool())
    .await
    .expect("read discussion schema");
    assert_eq!(tables.len(), 3);
    assert!(tables.iter().all(|(_, strict)| *strict == 1));
    let schema: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
             (SELECT user_version FROM pragma_user_version), \
             (SELECT COUNT(*) FROM _sqlx_migrations), \
             (SELECT COUNT(*) FROM pragma_foreign_key_list('discussion_comments')), \
             (SELECT COUNT(*) FROM pragma_foreign_key_list('comment_reactions'))",
    )
    .fetch_one(database.pool())
    .await
    .expect("read discussion schema metadata");
    assert_eq!(schema, (5, 5, 3, 4));
    database.close().await;
    directory.close().expect("remove upgraded fixture");
}

#[tokio::test]
async fn migration_four_rolls_back_an_orphan_legacy_proposal_pointer_atomically() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("governance-rollback.db");
    std::fs::File::create(&path).expect("real SQLite file");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .expect("open version-three pool");
    migrations_through_three()
        .run(&pool)
        .await
        .expect("apply migrations one through three");
    sqlx::query(
        "INSERT INTO trips ( \
             id, name, status, start_date, end_date, base_currency, created_at, revision \
         ) VALUES ( \
             'trip-a', 'Trip A', 'dreaming', '2026-08-07', '2026-08-09', \
             'GBP', '2026-08-07T12:00:00Z', 1 \
         )",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO plans ( \
             trip_id, version, id, created_from_proposal_id, created_at, revision \
         ) VALUES ( \
             'trip-a', 2, 'orphan-plan', 'missing-proposal', \
             '2026-08-07T13:00:00Z', 1 \
         )",
    )
    .execute(&pool)
    .await
    .expect("version three has no proposal parent foreign key");
    pool.close().await;

    assert!(matches!(
        SqliteDb::migrate(&path).await,
        Err(SqliteDbError::Migration(_))
    ));
    let mut connection = super::support::raw_connection(&path).await;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let migration_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let governance_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master \
         WHERE type = 'table' AND name IN ( \
             'proposals', 'polls', 'poll_options', 'poll_ballots', \
             'poll_ballot_options', 'proposal_content_edits', 'plans_v4' \
         )",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let orphan: (String, String) = sqlx::query_as(
        "SELECT id, created_from_proposal_id FROM plans \
         WHERE trip_id = 'trip-a' AND version = 2",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(version, 3);
    assert_eq!(migration_count, 3);
    assert_eq!(governance_tables, 0);
    assert_eq!(orphan, ("orphan-plan".into(), "missing-proposal".into()));
    connection.close().await.unwrap();
    directory.close().expect("remove rolled-back fixture");
}

#[tokio::test]
async fn migration_two_rolls_back_every_schema_change_for_an_orphan_legacy_pointer() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("incompatible-upgrade.db");
    std::fs::File::create(&path).expect("real SQLite file");
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .foreign_keys(true),
        )
        .await
        .expect("open version-one pool");
    Migrator::with_migrations(vec![Migration::new(
        1,
        "users trips".into(),
        MigrationType::Simple,
        include_str!("../../migrations/0001_users_trips.sql").into_sql_str(),
        false,
    )])
    .run(&pool)
    .await
    .expect("apply only migration one");
    sqlx::query(
        "INSERT INTO trips ( \
             id, name, status, start_date, end_date, base_currency, \
             current_plan_id, current_plan_version, created_at, revision \
         ) VALUES ( \
             'trip-a', 'Trip A', 'dreaming', '2026-08-07', '2026-08-09', \
             'GBP', 'legacy-plan', 1, '2026-08-07T12:00:00Z', 1 \
         )",
    )
    .execute(&pool)
    .await
    .expect("version one permits an unbound current-plan pointer");
    pool.close().await;

    assert!(matches!(
        SqliteDb::migrate(&path).await,
        Err(SqliteDbError::Migration(_))
    ));
    let mut connection = super::support::raw_connection(&path).await;
    let version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let migration_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    let partial_tables: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master \
         WHERE type = 'table' AND name IN ( \
             'trip_places', 'candidates', 'plans', 'plan_days', \
             'stop_identities', 'plan_stops', 'trips_v2' \
         )",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    let legacy_version: i64 =
        sqlx::query_scalar("SELECT current_plan_version FROM trips WHERE id = 'trip-a'")
            .fetch_one(&mut connection)
            .await
            .unwrap();
    assert_eq!(version, 1);
    assert_eq!(migration_count, 1);
    assert_eq!(partial_tables, 0);
    assert_eq!(legacy_version, 1);
    connection.close().await.unwrap();
    directory.close().expect("remove rolled-back fixture");
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
