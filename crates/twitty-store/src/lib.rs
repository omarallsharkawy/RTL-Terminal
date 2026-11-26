#![forbid(unsafe_code)]

//! Local storage for Twitty: SQLite (history, project memory, Drive index) with FTS5 search,
//! versioned schema migrations, mandatory secret redaction, a background worker handle, and an
//! OS keyring abstraction.

pub mod error;
pub mod fts;
pub mod migrations;
pub mod paths;
pub mod secrets;
pub mod store;
pub mod worker;

pub use error::StoreError;
pub use fts::escape_fts5_query;
pub use migrations::{
    apply_migrations, read_user_version, Migration, CURRENT_SCHEMA_VERSION, MIGRATIONS,
};
pub use paths::{db_path_unix, db_path_windows, default_db_path, resolve_project_root};
pub use secrets::{
    InMemorySecretStore, SecretError, SecretStore, SecretValue, SystemSecretStore, SECRET_SERVICE,
};
pub use store::{
    now_epoch_ms, DriveIndexEntry, HistoryEntry, MemoryEntry, MemoryKind, MemoryUpdate, NewCommand,
    NewMemory, Store,
};
pub use worker::StoreHandle;
