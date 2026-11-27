#![forbid(unsafe_code)]

//! Local storage for Twitty: SQLite (history, project memory, rules approvals, Drive index) with
//! FTS5 search, versioned schema migrations, mandatory secret redaction, a background worker
//! handle, and an OS keyring abstraction.

pub mod commands;
pub mod env;
pub mod error;
pub mod fts;
pub mod migrations;
pub mod paths;
pub mod project;
pub mod rules;
pub mod secrets;
pub mod store;
pub mod worker;

pub use commands::{
    compute_frequent_commands, FrequentCommand, AUTO_COMMAND_SOURCE,
    DEFAULT_FREQUENT_COMMAND_LIMIT, DEFAULT_MIN_COMMAND_RUNS,
};
pub use env::{detect_project_env, EnvNote, AUTO_ENV_SOURCE};
pub use error::StoreError;
pub use fts::escape_fts5_query;
pub use migrations::{
    apply_migrations, read_user_version, Migration, CURRENT_SCHEMA_VERSION, MIGRATIONS,
};
pub use paths::{db_path_unix, db_path_windows, default_db_path, resolve_project_root};
pub use project::{is_path_excluded, is_path_excluded_with_home, is_project_excluded};
pub use rules::{
    inspect_rules_files_with_approvals, RulesApproval, RulesFileInfo, RulesFileState,
    MAX_RULES_FILE_BYTES, RULES_FILES,
};
pub use secrets::{
    InMemorySecretStore, SecretError, SecretStore, SecretValue, SystemSecretStore, SECRET_SERVICE,
};
pub use store::{
    now_epoch_ms, DriveIndexEntry, ForgetReport, HistoryEntry, MemoryEntry, MemoryKind,
    MemoryUpdate, NewCommand, NewMemory, ProjectMemorySnapshot, Store,
};
pub use twitty_core::sha256::{self, sha256, sha256_hex};
pub use worker::StoreHandle;
