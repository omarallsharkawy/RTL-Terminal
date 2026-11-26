//! Error types for [`crate::Store`] and [`crate::StoreHandle`].

use std::fmt;
use std::io;

/// Errors returned by local storage operations.
#[derive(Debug)]
pub enum StoreError {
    /// Filesystem error preparing or securing the database path.
    Io(io::Error),
    /// SQLite error from `rusqlite`.
    Sqlite(rusqlite::Error),
    /// The on-disk database has a higher `PRAGMA user_version` than this build supports.
    /// The database was opened read-only and was not modified or downgraded.
    NewerSchema { found: u32, supported: u32 },
    /// No platform data directory could be resolved from the environment.
    NoDataDir,
    /// The background store thread is unavailable (disabled or failed to open).
    Unavailable,
    /// Caller supplied invalid input (e.g. empty path or unsupported kind).
    InvalidInput(&'static str),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "store I/O error: {err}"),
            Self::Sqlite(err) => write!(f, "store SQLite error: {err}"),
            Self::NewerSchema { found, supported } => write!(
                f,
                "database schema version {found} is newer than supported version {supported}; \
                 opened read-only without downgrading"
            ),
            Self::NoDataDir => write!(f, "could not resolve Twitty data directory"),
            Self::Unavailable => write!(f, "local store is unavailable"),
            Self::InvalidInput(msg) => write!(f, "invalid store input: {msg}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Sqlite(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for StoreError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err)
    }
}
