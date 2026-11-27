//! Versioned SQLite schema migrations tracked via `PRAGMA user_version`.

use rusqlite::Connection;

use crate::error::StoreError;

/// Latest schema version supported by this build.
pub const CURRENT_SCHEMA_VERSION: u32 = 3;

/// One forward-only schema migration applied inside a transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Migration {
    pub version: u32,
    pub sql: &'static str,
}

/// Schema v1: command history (+ FTS5), project memory (+ FTS5), and Drive file index.
const SCHEMA_V1: &str = r#"
CREATE TABLE history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    command TEXT NOT NULL,
    exit_code INTEGER NULL,
    duration_ms INTEGER NULL,
    cwd TEXT NOT NULL,
    project_root TEXT NULL,
    shell TEXT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER NULL
);

CREATE INDEX idx_history_started_at ON history(started_at DESC, id DESC);
CREATE INDEX idx_history_project_root ON history(project_root, started_at DESC, id DESC);

CREATE VIRTUAL TABLE history_fts USING fts5(
    command,
    content='history',
    content_rowid='id',
    tokenize='unicode61'
);

CREATE TRIGGER history_ai AFTER INSERT ON history BEGIN
    INSERT INTO history_fts(rowid, command) VALUES (new.id, new.command);
END;

CREATE TRIGGER history_ad AFTER DELETE ON history BEGIN
    INSERT INTO history_fts(history_fts, rowid, command)
    VALUES ('delete', old.id, old.command);
END;

CREATE TRIGGER history_au AFTER UPDATE OF command ON history BEGIN
    INSERT INTO history_fts(history_fts, rowid, command)
    VALUES ('delete', old.id, old.command);
    INSERT INTO history_fts(rowid, command) VALUES (new.id, new.command);
END;

CREATE TABLE memory (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project_root TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('rule','fact','command','env')),
    text TEXT NOT NULL,
    source TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    approved INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_memory_project_root ON memory(project_root, updated_at DESC, id DESC);

CREATE VIRTUAL TABLE memory_fts USING fts5(
    text,
    content='memory',
    content_rowid='id',
    tokenize='unicode61'
);

CREATE TRIGGER memory_ai AFTER INSERT ON memory BEGIN
    INSERT INTO memory_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER memory_ad AFTER DELETE ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, text)
    VALUES ('delete', old.id, old.text);
END;

CREATE TRIGGER memory_au AFTER UPDATE OF text ON memory BEGIN
    INSERT INTO memory_fts(memory_fts, rowid, text)
    VALUES ('delete', old.id, old.text);
    INSERT INTO memory_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TABLE drive_index (
    path TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    hash TEXT NOT NULL
);

CREATE INDEX idx_drive_index_kind ON drive_index(kind, updated_at DESC);
"#;

/// Schema v2: per-project SHA-256 approvals for project rules files (`AGENTS.md`, `CLAUDE.md`,
/// `TWITTY.md`, `.twitty/rules.md`).
const SCHEMA_V2: &str = r#"
CREATE TABLE rules_file_approvals (
    project_root TEXT NOT NULL,
    rel_path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    approved_at INTEGER NOT NULL,
    PRIMARY KEY (project_root, rel_path)
);

CREATE INDEX idx_rules_approvals_project ON rules_file_approvals(project_root);
"#;

/// Schema v3: composite index accelerating per-project + kind memory queries.
const SCHEMA_V3: &str = r#"
CREATE INDEX IF NOT EXISTS idx_memory_project_kind
    ON memory(project_root, kind, updated_at DESC, id DESC);
"#;

/// Ordered list of schema migrations shipped with this crate.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: SCHEMA_V1,
    },
    Migration {
        version: 2,
        sql: SCHEMA_V2,
    },
    Migration {
        version: 3,
        sql: SCHEMA_V3,
    },
];

/// Read SQLite's `PRAGMA user_version`.
pub fn read_user_version(conn: &Connection) -> Result<u32, StoreError> {
    let version: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    Ok(version)
}

/// Apply all pending migrations from `migrations` in strictly ascending version order.
///
/// - Each migration executes in its own transaction together with `PRAGMA user_version = N`.
/// - Re-running when already at the latest version is a no-op.
/// - If the database has a version newer than `migrations.last()`, returns
///   [`StoreError::NewerSchema`] without modifying the database.
pub fn apply_migrations(
    conn: &mut Connection,
    migrations: &[Migration],
) -> Result<u32, StoreError> {
    let latest = migrations.last().map_or(0, |m| m.version);
    let current = read_user_version(conn)?;
    if current > latest {
        return Err(StoreError::NewerSchema {
            found: current,
            supported: latest,
        });
    }
    if current == latest {
        return Ok(current);
    }

    let mut applied = current;
    for migration in migrations {
        if migration.version <= applied {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(migration.sql)?;
        tx.pragma_update(None, "user_version", migration.version)?;
        tx.commit()?;
        applied = migration.version;
    }
    Ok(applied)
}
