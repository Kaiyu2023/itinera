//! Mechanical dispatch from a SQLite row to a capability-owned record type.

use sqlx::sqlite::SqliteRow;

/// A type that owns both its row shape and its persisted-data invariants.
pub(crate) trait SqliteRecord: Sized {
    type Error;

    fn try_from_sqlite_row(row: &SqliteRow) -> Result<Self, Self::Error>;
}

pub(crate) trait SqliteRowExt {
    fn decode<T>(&self) -> Result<T, T::Error>
    where
        T: SqliteRecord;
}

impl SqliteRowExt for SqliteRow {
    fn decode<T>(&self) -> Result<T, T::Error>
    where
        T: SqliteRecord,
    {
        T::try_from_sqlite_row(self)
    }
}
