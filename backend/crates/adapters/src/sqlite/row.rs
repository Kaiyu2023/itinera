//! Mechanical decoding from a SQLite row into a query-shaped struct.

use itinera_core::ports::{trip::TripRepoError, user::UserRepoError};
use sqlx::{FromRow, sqlite::SqliteRow};

#[derive(Debug, Clone, Copy)]
pub(crate) struct CorruptRow;

impl From<CorruptRow> for TripRepoError {
    fn from(_error: CorruptRow) -> Self {
        Self::CorruptData
    }
}

impl From<CorruptRow> for UserRepoError {
    fn from(_error: CorruptRow) -> Self {
        Self::CorruptData
    }
}

pub(crate) trait SqliteRowExt {
    fn decode<T>(&self) -> Result<T, CorruptRow>
    where
        T: for<'row> FromRow<'row, SqliteRow>;
}

impl SqliteRowExt for SqliteRow {
    fn decode<T>(&self) -> Result<T, CorruptRow>
    where
        T: for<'row> FromRow<'row, SqliteRow>,
    {
        T::from_row(self).map_err(|_| CorruptRow)
    }
}
