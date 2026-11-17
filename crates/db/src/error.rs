//! Typed errors of the repository API.

use std::fmt;

/// A repository operation failed.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// The record does not exist.
    #[error("the record was not found")]
    NotFound,
    /// The change conflicts with existing data (unique or state constraint).
    #[error("the change conflicts with existing data")]
    Conflict,
    /// The operation is declared but not implemented yet (filled in by its owning work package).
    #[error("the database operation {operation} is not implemented yet")]
    NotImplemented {
        /// `module::function` of the operation.
        operation: &'static str,
    },
    /// A stored value is malformed (e.g. a timestamp or id column).
    #[error("a stored value in {column} is malformed")]
    Corrupt {
        /// The column.
        column: &'static str,
    },
    /// A stored JSON document could not be written or read.
    #[error("a stored document could not be encoded or decoded")]
    Json(#[from] serde_json::Error),
    /// The database itself failed.
    #[error("the database failed")]
    Storage(#[from] StorageError),
}

impl DbError {
    /// The SQLite (extended) result code of a database failure, e.g. `517` for `SQLITE_BUSY_SNAPSHOT`; for logs,
    /// never carries data values.
    pub fn sqlite_code(&self) -> Option<String> {
        match self {
            Self::Storage(StorageError(Inner::Sqlx(error))) => match error.as_ref() {
                sqlx::Error::Database(database) => database.code().map(|code| code.into_owned()),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether the database was busy or locked (`SQLITE_BUSY`, `SQLITE_LOCKED` or an extended code of either):
    /// a transient failure the caller may retry.
    pub fn is_busy(&self) -> bool {
        self.sqlite_code()
            .and_then(|code| code.parse::<i32>().ok())
            .is_some_and(|code| matches!(code & 0xff, 5 | 6))
    }
}

/// Opaque database failure; no sqlx type is reachable through the public API (plan A2).
pub struct StorageError(Inner);

enum Inner {
    Sqlx(Box<sqlx::Error>),
    Migrate(Box<sqlx::migrate::MigrateError>),
}

impl fmt::Debug for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Inner::Sqlx(error) => f.debug_tuple("StorageError").field(error).finish(),
            Inner::Migrate(error) => f.debug_tuple("StorageError").field(error).finish(),
        }
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Inner::Sqlx(_) => f.write_str("SQLite query failed"),
            Inner::Migrate(_) => f.write_str("the schema migration failed"),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match &self.0 {
            Inner::Sqlx(error) => Some(error.as_ref()),
            Inner::Migrate(error) => Some(error.as_ref()),
        }
    }
}

impl From<sqlx::Error> for DbError {
    fn from(error: sqlx::Error) -> Self {
        match &error {
            sqlx::Error::RowNotFound => Self::NotFound,
            sqlx::Error::Database(database) if database.is_unique_violation() => Self::Conflict,
            _ => Self::Storage(StorageError(Inner::Sqlx(Box::new(error)))),
        }
    }
}

impl From<sqlx::migrate::MigrateError> for DbError {
    fn from(error: sqlx::migrate::MigrateError) -> Self {
        Self::Storage(StorageError(Inner::Migrate(Box::new(error))))
    }
}
