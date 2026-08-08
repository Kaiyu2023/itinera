//! Shared SQLite foundation and capability-scoped repository adapters.
//!
//! `SqliteDb` owns only connection setup, migration validation, transaction
//! entry, and mechanical codecs. Capability SQL stays in `user_repo`,
//! `trip_repo`, and `history_repo`; this module must not grow into an
//! all-purpose repository.

use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
    time::Duration,
};

#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[cfg(target_os = "linux")]
use std::borrow::Cow;

use sqlx::{
    Row, SqlSafeStr, Sqlite, SqliteConnection, SqlitePool, Transaction,
    migrate::{Migration, MigrationType, Migrator},
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};

pub use history_repo::SqliteContentHistoryRepo;
pub use trip_repo::SqliteTripRepo;
pub use user_repo::SqliteUserRepo;

pub(crate) mod codec;
mod history_repo;
pub(crate) mod row;
mod trip_repo;
mod user_repo;

pub const SQLITE_POOL_MAX_CONNECTIONS: u32 = 4;
pub const SQLITE_BUSY_TIMEOUT_MILLIS: u64 = 5_000;
pub const SQLITE_WAL_AUTOCHECKPOINT_PAGES: u32 = 1_000;
pub const EXPECTED_SCHEMA_VERSION: i64 = 3;
pub const EXPECTED_SQLITE_VERSION: &str = "3.51.3";
pub const EXPECTED_SQLITE_SOURCE_ID: &str =
    "2026-03-13 10:38:09 737ae4a34738ffa0c3ff7f9bb18df914dd1cad163f28fd6b6e114a344fe6d618";

const BEGIN_IMMEDIATE_ATTEMPTS: usize = 4;
static MIGRATOR: LazyLock<Migrator> = LazyLock::new(|| {
    // Keep this explicit list beside the expected schema version. The SQL is
    // compiled into the binary and `Migration::new` computes the same SHA-384
    // checksum SQLx persists in `_sqlx_migrations`.
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
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteEngineIdentity {
    pub version: String,
    pub source_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SqliteDbError {
    #[error("the SQLite database path must be an absolute local file path")]
    InvalidPath,
    #[error("the SQLite database path is on a network filesystem")]
    NetworkFilesystem,
    #[error("the SQLite database file does not exist")]
    MissingDatabase,
    #[error("the SQLite database path is not a regular file")]
    NotAFile,
    #[error("the bundled SQLite engine does not match the pinned runtime")]
    EngineMismatch,
    #[error("a required SQLite connection setting could not be established")]
    ConnectionInvariant,
    #[error("the SQLite migration version or checksum does not match this image")]
    MigrationMismatch,
    #[error("the SQLite database failed an integrity or foreign-key check")]
    IntegrityFailure,
    #[error("SQLite is unavailable")]
    Unavailable(#[source] sqlx::Error),
    #[error("SQLite migration failed")]
    Migration(#[source] sqlx::migrate::MigrateError),
    #[error("the database filesystem could not be classified safely")]
    FilesystemUnknown,
}

/// One checked, bounded pool shared by all SQLite capability repositories.
#[derive(Clone)]
pub struct SqliteDb {
    pool: SqlitePool,
    path: PathBuf,
    engine: SqliteEngineIdentity,
    #[cfg(test)]
    commit_ack_loss: Arc<AtomicBool>,
}

impl SqliteDb {
    /// Open an already-migrated database. Normal application startup never
    /// applies migrations; a missing or stale schema fails closed.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, SqliteDbError> {
        let path = validate_database_path(path.as_ref(), false)?;
        let pool = open_pool(&path, false, SQLITE_POOL_MAX_CONNECTIONS).await?;
        let engine = match validate_engine(&pool).await {
            Ok(engine) => engine,
            Err(error) => {
                pool.close().await;
                return Err(error);
            }
        };
        if let Err(error) = validate_schema(&pool).await {
            pool.close().await;
            return Err(error);
        }
        Ok(Self {
            pool,
            path,
            engine,
            #[cfg(test)]
            commit_ack_loss: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Explicit maintenance entry point. The application must be stopped when
    /// this is used outside tests; normal startup deliberately never calls it.
    pub async fn migrate(path: impl AsRef<Path>) -> Result<(), SqliteDbError> {
        let path = validate_database_path(path.as_ref(), true)?;
        let pool = open_pool(&path, true, 1).await?;
        if let Err(error) = validate_engine(&pool).await {
            pool.close().await;
            return Err(error);
        }
        if let Err(error) = MIGRATOR.run(&pool).await {
            pool.close().await;
            return Err(SqliteDbError::Migration(error));
        }
        if let Err(error) = validate_schema(&pool).await {
            pool.close().await;
            return Err(error);
        }
        if let Err(error) = validate_integrity(&pool).await {
            pool.close().await;
            return Err(error);
        }
        pool.close().await;
        Ok(())
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn engine(&self) -> &SqliteEngineIdentity {
        &self.engine
    }

    pub async fn validate(&self) -> Result<(), SqliteDbError> {
        validate_engine(&self.pool).await?;
        validate_schema(&self.pool).await?;
        sqlx::query_scalar::<_, i64>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(SqliteDbError::Unavailable)?;
        Ok(())
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    pub(crate) async fn begin_immediate(
        &self,
    ) -> Result<Transaction<'static, Sqlite>, sqlx::Error> {
        let mut last = None;
        for attempt in 0..BEGIN_IMMEDIATE_ATTEMPTS {
            match self.pool.begin_with("BEGIN IMMEDIATE").await {
                Ok(transaction) => return Ok(transaction),
                Err(error) if is_busy(&error) && attempt + 1 < BEGIN_IMMEDIATE_ATTEMPTS => {
                    last = Some(error);
                    tokio::task::yield_now().await;
                }
                Err(error) => return Err(error),
            }
        }
        Err(last.expect("the bounded BEGIN IMMEDIATE loop records a busy error"))
    }

    pub(crate) async fn commit(
        &self,
        transaction: Transaction<'static, Sqlite>,
    ) -> Result<(), sqlx::Error> {
        transaction.commit().await?;
        // A lost acknowledgement after SQLite commits is intentionally
        // indistinguishable from other commit failures to repository callers.
        // Mutations must never replay automatically in this state.
        #[cfg(test)]
        if self.commit_ack_loss.swap(false, Ordering::SeqCst) {
            return Err(sqlx::Error::Protocol(
                "injected loss of the SQLite commit acknowledgement".into(),
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    fn inject_commit_ack_loss(&self) {
        self.commit_ack_loss.store(true, Ordering::SeqCst);
    }
}

async fn open_pool(
    path: &Path,
    create_if_missing: bool,
    connections: u32,
) -> Result<SqlitePool, SqliteDbError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(create_if_missing)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Full)
        .busy_timeout(Duration::from_millis(SQLITE_BUSY_TIMEOUT_MILLIS))
        .pragma(
            "wal_autocheckpoint",
            SQLITE_WAL_AUTOCHECKPOINT_PAGES.to_string(),
        );

    SqlitePoolOptions::new()
        .min_connections(connections)
        .max_connections(connections)
        .acquire_timeout(Duration::from_secs(6))
        .after_connect(|connection, _metadata| {
            Box::pin(async move { verify_connection(connection).await })
        })
        .connect_with(options)
        .await
        .map_err(classify_connect_error)
}

async fn verify_connection(connection: &mut SqliteConnection) -> Result<(), sqlx::Error> {
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&mut *connection)
        .await?;
    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut *connection)
        .await?;
    let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
        .fetch_one(&mut *connection)
        .await?;
    let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(&mut *connection)
        .await?;
    let autocheckpoint: i64 = sqlx::query_scalar("PRAGMA wal_autocheckpoint")
        .fetch_one(&mut *connection)
        .await?;
    let version: String = sqlx::query_scalar("SELECT sqlite_version()")
        .fetch_one(&mut *connection)
        .await?;
    let source_id: String = sqlx::query_scalar("SELECT sqlite_source_id()")
        .fetch_one(&mut *connection)
        .await?;

    if foreign_keys != 1
        || !journal_mode.eq_ignore_ascii_case("wal")
        || synchronous != 2
        || busy_timeout != SQLITE_BUSY_TIMEOUT_MILLIS as i64
        || autocheckpoint != SQLITE_WAL_AUTOCHECKPOINT_PAGES as i64
        || version != EXPECTED_SQLITE_VERSION
        || source_id != EXPECTED_SQLITE_SOURCE_ID
    {
        return Err(sqlx::Error::Protocol(
            "SQLite connection invariant mismatch".into(),
        ));
    }
    Ok(())
}

async fn validate_engine(pool: &SqlitePool) -> Result<SqliteEngineIdentity, SqliteDbError> {
    let row = sqlx::query("SELECT sqlite_version() AS version, sqlite_source_id() AS source_id")
        .fetch_one(pool)
        .await
        .map_err(SqliteDbError::Unavailable)?;
    let identity = SqliteEngineIdentity {
        version: row.try_get("version").map_err(SqliteDbError::Unavailable)?,
        source_id: row
            .try_get("source_id")
            .map_err(SqliteDbError::Unavailable)?,
    };
    if identity.version != EXPECTED_SQLITE_VERSION
        || identity.source_id != EXPECTED_SQLITE_SOURCE_ID
    {
        return Err(SqliteDbError::EngineMismatch);
    }
    Ok(identity)
}

async fn validate_schema(pool: &SqlitePool) -> Result<(), SqliteDbError> {
    let user_version: i64 = sqlx::query_scalar("PRAGMA user_version")
        .fetch_one(pool)
        .await
        .map_err(SqliteDbError::Unavailable)?;
    if user_version != EXPECTED_SCHEMA_VERSION {
        return Err(SqliteDbError::MigrationMismatch);
    }

    let applied =
        sqlx::query("SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(pool)
            .await
            .map_err(|_| SqliteDbError::MigrationMismatch)?;
    let expected = MIGRATOR.iter().collect::<Vec<_>>();
    if applied.len() != expected.len() {
        return Err(SqliteDbError::MigrationMismatch);
    }
    for (row, migration) in applied.iter().zip(expected) {
        let version: i64 = row
            .try_get("version")
            .map_err(|_| SqliteDbError::MigrationMismatch)?;
        let success: bool = row
            .try_get("success")
            .map_err(|_| SqliteDbError::MigrationMismatch)?;
        let checksum: Vec<u8> = row
            .try_get("checksum")
            .map_err(|_| SqliteDbError::MigrationMismatch)?;
        if version != migration.version || !success || checksum != migration.checksum.as_ref() {
            return Err(SqliteDbError::MigrationMismatch);
        }
    }
    Ok(())
}

async fn validate_integrity(pool: &SqlitePool) -> Result<(), SqliteDbError> {
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(pool)
        .await
        .map_err(SqliteDbError::Unavailable)?;
    let foreign_key_violations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(pool)
            .await
            .map_err(SqliteDbError::Unavailable)?;
    if integrity != "ok" || foreign_key_violations != 0 {
        return Err(SqliteDbError::IntegrityFailure);
    }
    Ok(())
}

fn classify_connect_error(error: sqlx::Error) -> SqliteDbError {
    if matches!(&error, sqlx::Error::Protocol(message) if message.contains("invariant")) {
        SqliteDbError::ConnectionInvariant
    } else {
        SqliteDbError::Unavailable(error)
    }
}

pub(crate) fn is_busy(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|database| database.code())
        .is_some_and(|code| matches!(code.as_ref(), "5" | "6" | "SQLITE_BUSY" | "SQLITE_LOCKED"))
}

fn validate_database_path(path: &Path, allow_missing: bool) -> Result<PathBuf, SqliteDbError> {
    if !path.is_absolute() || looks_like_memory_or_uri(path) {
        return Err(SqliteDbError::InvalidPath);
    }
    // Reject UNC and mapped network drives before canonicalization can touch
    // the remote endpoint (and potentially disclose host credentials).
    #[cfg(windows)]
    reject_network_filesystem(path)?;
    let parent = path.parent().ok_or(SqliteDbError::InvalidPath)?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|_| SqliteDbError::InvalidPath)?;

    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            let canonical_file = path
                .canonicalize()
                .map_err(|_| SqliteDbError::InvalidPath)?;
            reject_network_filesystem(&canonical_file)?;
            return Ok(canonical_file);
        }
        Ok(_) => return Err(SqliteDbError::NotAFile),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && allow_missing => {
            reject_network_filesystem(&canonical_parent)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(SqliteDbError::MissingDatabase);
        }
        Err(_) => return Err(SqliteDbError::InvalidPath),
    }

    Ok(canonical_parent.join(path.file_name().ok_or(SqliteDbError::InvalidPath)?))
}

fn looks_like_memory_or_uri(path: &Path) -> bool {
    let value = path.to_string_lossy();
    value == ":memory:"
        || value.starts_with("file:")
        || value.contains("mode=memory")
        || path.file_name().is_some_and(|name| name == ":memory:")
}

#[cfg(windows)]
fn reject_network_filesystem(path: &Path) -> Result<(), SqliteDbError> {
    use std::{
        os::windows::ffi::OsStrExt,
        path::{Component, Prefix},
    };
    use windows_sys::Win32::Storage::FileSystem::GetDriveTypeW;

    const DRIVE_REMOTE: u32 = 4;

    if matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _))
    ) {
        return Err(SqliteDbError::NetworkFilesystem);
    }
    let root = path
        .ancestors()
        .last()
        .ok_or(SqliteDbError::FilesystemUnknown)?;
    let mut wide = root.as_os_str().encode_wide().collect::<Vec<_>>();
    wide.push(0);
    // SAFETY: `wide` is a live, NUL-terminated UTF-16 root path for the entire
    // duration of the call, and GetDriveTypeW does not retain the pointer.
    let drive_type = unsafe { GetDriveTypeW(wide.as_ptr()) };
    if drive_type == 0 || drive_type == 1 {
        return Err(SqliteDbError::FilesystemUnknown);
    }
    if drive_type == DRIVE_REMOTE {
        return Err(SqliteDbError::NetworkFilesystem);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn reject_network_filesystem(path: &Path) -> Result<(), SqliteDbError> {
    let mountinfo = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|_| SqliteDbError::FilesystemUnknown)?;
    let file_system =
        filesystem_type_for_path(&mountinfo, path).ok_or(SqliteDbError::FilesystemUnknown)?;
    if matches!(
        file_system.as_str(),
        "9p" | "afs"
            | "ceph"
            | "cifs"
            | "davfs"
            | "fuse.sshfs"
            | "glusterfs"
            | "nfs"
            | "nfs4"
            | "smb3"
            | "sshfs"
    ) {
        return Err(SqliteDbError::NetworkFilesystem);
    }
    // The production EBS volume is ext4 or XFS. A small local allowlist keeps
    // an unfamiliar distributed/FUSE filesystem from being treated as safe by
    // default; overlay/tmpfs are retained for containerized real-file tests.
    if matches!(
        file_system.as_str(),
        "btrfs" | "ext2" | "ext3" | "ext4" | "overlay" | "tmpfs" | "xfs"
    ) {
        Ok(())
    } else {
        Err(SqliteDbError::FilesystemUnknown)
    }
}

#[cfg(target_os = "linux")]
fn filesystem_type_for_path(mountinfo: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in mountinfo.lines() {
        let (left, right) = line.split_once(" - ")?;
        let mut fields = left.split_whitespace();
        let mount_point = fields.nth(4)?;
        let decoded = decode_mountinfo_path(mount_point);
        let candidate = Path::new(decoded.as_ref());
        if path.starts_with(candidate) {
            let length = candidate.as_os_str().len();
            let file_system = right.split_whitespace().next()?.to_string();
            if best.as_ref().is_none_or(|(current, _)| length > *current) {
                best = Some((length, file_system));
            }
        }
    }
    best.map(|(_, file_system)| file_system)
}

#[cfg(target_os = "linux")]
fn decode_mountinfo_path(value: &str) -> Cow<'_, str> {
    if !value.contains('\\') {
        return Cow::Borrowed(value);
    }
    Cow::Owned(
        value
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\"),
    )
}

#[cfg(not(any(windows, target_os = "linux")))]
fn reject_network_filesystem(_path: &Path) -> Result<(), SqliteDbError> {
    Err(SqliteDbError::FilesystemUnknown)
}

#[cfg(test)]
mod commit_tests {
    use itinera_core::{
        domain::{
            trip::{Invite, NewInviteInput, NewTripInput, Trip},
            user::{Email, User, UserId},
        },
        ports::{
            authorization::TripAuthorizationContext,
            trip::{TripRepo, TripRepoError},
            user::UserRepo,
        },
    };

    use super::{SqliteDb, SqliteTripRepo, SqliteUserRepo};

    const NOW: &str = "2026-08-07T12:00:00.000Z";

    #[tokio::test]
    async fn a_lost_commit_acknowledgement_is_indeterminate_and_never_replayed() {
        let directory = tempfile::Builder::new()
            .prefix("itinera-ambiguous-commit-")
            .tempdir()
            .expect("create local temporary directory");
        let path = directory.path().join("itinera.db");
        std::fs::File::create(&path).expect("create a real empty SQLite file");
        SqliteDb::migrate(&path).await.expect("apply migration");
        let db = SqliteDb::open(&path).await.expect("open migrated database");
        let users = SqliteUserRepo::new(db.clone());
        let trips = SqliteTripRepo::new(db.clone());
        let leader = User {
            id: UserId("leader".into()),
            email: Email::parse("leader@example.com").unwrap(),
            display_name: Some("Leader".into()),
        };
        users.insert(leader.clone()).await.unwrap();
        let authorization = TripAuthorizationContext::human(leader.id.clone());
        trips
            .create_trip(
                &authorization,
                Trip::create(NewTripInput {
                    id: "trip-a".into(),
                    name: "Trip A".into(),
                    start_date: "2026-08-07".into(),
                    end_date: "2026-08-09".into(),
                    base_currency: "GBP".into(),
                    creator_id: leader.id.0.clone(),
                    created_at: NOW.into(),
                })
                .unwrap(),
            )
            .await
            .unwrap();

        db.inject_commit_ack_loss();
        let result = trips
            .create_invite(
                "trip-a",
                &authorization,
                Invite::create(NewInviteInput {
                    id: "invite-a".into(),
                    trip_id: "trip-a".into(),
                    email: Email::parse("invitee@example.com").unwrap(),
                    invited_by: leader.id.0.clone(),
                    created_at: NOW.into(),
                })
                .unwrap(),
            )
            .await;
        assert!(matches!(result, Err(TripRepoError::Unavailable)));

        let persisted: (i64, String, i64) = sqlx::query_as(
            "SELECT COUNT(*), MIN(status), MIN(revision) \
             FROM trip_invites WHERE trip_id = 'trip-a' AND id = 'invite-a'",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(persisted, (1, "pending".into(), 1));

        drop(trips);
        drop(users);
        db.close().await;
        drop(db);
        directory.close().expect("close real SQLite test files");
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::path::Path;

    use super::filesystem_type_for_path;

    #[test]
    fn mountinfo_uses_the_longest_matching_mount() {
        let input = concat!(
            "1 0 8:1 / / rw - ext4 /dev/root rw\n",
            "2 1 0:2 / /srv/data rw - nfs server:/data rw\n",
        );
        assert_eq!(
            filesystem_type_for_path(input, Path::new("/srv/data/itinera")),
            Some("nfs".into())
        );
        assert_eq!(
            filesystem_type_for_path(input, Path::new("/var/lib/itinera")),
            Some("ext4".into())
        );
    }
}
