//! Synchronous typed SQLite store for command history, project memory, and Drive index.
//!
//! All text written through [`Store`] passes through [`twitty_core::redact::redact`] internally so
//! callers cannot bypass secret redaction. No raw SQL is exposed outside this crate.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use twitty_core::redact::redact;

use crate::commands::{
    compute_frequent_commands, FrequentCommand, AUTO_COMMAND_SOURCE,
    DEFAULT_FREQUENT_COMMAND_LIMIT, DEFAULT_MIN_COMMAND_RUNS,
};
use crate::env::{detect_project_env, AUTO_ENV_SOURCE};
use crate::error::StoreError;
use crate::fts::{escape_fts5_or_query, escape_fts5_query};
use crate::migrations::{
    apply_migrations, read_user_version, Migration, CURRENT_SCHEMA_VERSION, MIGRATIONS,
};
use crate::paths::{default_db_path, prepare_db_path, tighten_sidecar_permissions};
use crate::project::is_path_excluded;
use crate::rules::{inspect_rules_files_with_approvals, RulesApproval, RulesFileInfo};

/// Maximum rows returned by any single query.
const MAX_QUERY_LIMIT: usize = 1000;

/// Current wall-clock timestamp in Unix epoch milliseconds.
pub fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(i64::MAX as u128) as i64)
}

/// Input for recording a command execution in `history`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewCommand {
    pub command: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub cwd: String,
    pub project_root: Option<String>,
    pub shell: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

/// One row from the `history` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEntry {
    pub id: i64,
    pub command: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub cwd: String,
    pub project_root: Option<String>,
    pub shell: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

/// Classification of a project memory record.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemoryKind {
    Rule,
    Fact,
    Command,
    Env,
}

impl MemoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Fact => "fact",
            Self::Command => "command",
            Self::Env => "env",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "rule" => Some(Self::Rule),
            "fact" => Some(Self::Fact),
            "command" => Some(Self::Command),
            "env" => Some(Self::Env),
            _ => None,
        }
    }
}

/// Input for inserting a record into `memory`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewMemory {
    pub project_root: String,
    pub kind: MemoryKind,
    pub text: String,
    pub source: String,
    pub approved: bool,
}

/// Partial update for an existing `memory` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryUpdate {
    pub kind: Option<MemoryKind>,
    pub text: Option<String>,
    pub source: Option<String>,
    pub approved: Option<bool>,
}

/// One row from the `memory` table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemoryEntry {
    pub id: i64,
    pub project_root: String,
    pub kind: MemoryKind,
    pub text: String,
    pub source: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub approved: bool,
}

/// One row in `drive_index`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriveIndexEntry {
    pub path: String,
    pub kind: String,
    pub title: String,
    pub updated_at: i64,
    pub hash: String,
}

/// Counts removed by [`Store::forget_project`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForgetReport {
    pub memories_deleted: usize,
    pub rules_approvals_deleted: usize,
    pub history_deleted: usize,
}

/// Off-thread snapshot of everything shown in the Project Memory screen for one project.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectMemorySnapshot {
    pub project_root: String,
    pub excluded: bool,
    pub rules_files: Vec<RulesFileInfo>,
    pub entries: Vec<MemoryEntry>,
    pub frequent_commands: Vec<FrequentCommand>,
}

/// Local SQLite storage engine.
pub struct Store {
    conn: Connection,
    read_only: bool,
}

impl Store {
    /// Open (creating if absent) and migrate the database at `path`.
    ///
    /// If `path` already exists with a `PRAGMA user_version` newer than [`CURRENT_SCHEMA_VERSION`],
    /// it is inspected read-only and [`StoreError::NewerSchema`] is returned without modifying or
    /// downgrading the file.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::open_with_migrations(path, MIGRATIONS)
    }

    /// Open and migrate the default per-user database (`$XDG_DATA_HOME/twitty/twitty.db` or
    /// platform equivalent).
    pub fn open_default() -> Result<Self, StoreError> {
        let path = default_db_path().ok_or(StoreError::NoDataDir)?;
        Self::open(&path)
    }

    /// Open an in-memory SQLite database with the latest schema applied (for tests).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::open_in_memory_with_migrations(MIGRATIONS)
    }

    /// Open an existing database strictly read-only without running migrations.
    pub fn open_read_only(path: &Path) -> Result<Self, StoreError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)?;
        conn.pragma_update(None, "busy_timeout", 5000i64)?;
        Ok(Self {
            conn,
            read_only: true,
        })
    }

    /// Open and migrate `path` using a custom migration list (exposed for migration testing).
    pub fn open_with_migrations(path: &Path, migrations: &[Migration]) -> Result<Self, StoreError> {
        let latest = migrations.last().map_or(0, |m| m.version);

        // If the file already exists, check its schema version read-only first so a newer-than-
        // known database is never opened read-write, never creates WAL files, and never downgrades.
        if path.exists() {
            let ro_flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            let ro = Connection::open_with_flags(path, ro_flags)?;
            let found = read_user_version(&ro)?;
            drop(ro);
            if found > latest {
                return Err(StoreError::NewerSchema {
                    found,
                    supported: latest,
                });
            }
        }

        prepare_db_path(path)?;

        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut conn = Connection::open_with_flags(path, flags)?;
        configure_connection(&conn, true)?;
        apply_migrations(&mut conn, migrations)?;
        tighten_sidecar_permissions(path);

        Ok(Self {
            conn,
            read_only: false,
        })
    }

    /// Open an in-memory database with a custom migration list (for migration tests).
    pub fn open_in_memory_with_migrations(migrations: &[Migration]) -> Result<Self, StoreError> {
        let mut conn = Connection::open_in_memory()?;
        configure_connection(&conn, false)?;
        apply_migrations(&mut conn, migrations)?;
        Ok(Self {
            conn,
            read_only: false,
        })
    }

    /// Current `PRAGMA user_version` of this database.
    pub fn schema_version(&self) -> Result<u32, StoreError> {
        read_user_version(&self.conn)
    }

    /// True if this store was opened read-only.
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Current SQLite journal mode (e.g. `"wal"` for file-backed stores).
    pub fn journal_mode(&self) -> Result<String, StoreError> {
        let mode: String = self
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
        Ok(mode)
    }

    /// Whether `PRAGMA foreign_keys` is enabled.
    pub fn foreign_keys_enabled(&self) -> Result<bool, StoreError> {
        let on: i64 = self
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        Ok(on != 0)
    }

    // ---- Command history ----

    /// Insert a command into `history` after redacting all text fields. Returns the row id.
    pub fn record_command(&self, entry: &NewCommand) -> Result<i64, StoreError> {
        let command = redact(&entry.command);
        let cwd = redact(&entry.cwd);
        let project_root = entry.project_root.as_deref().map(redact);
        let shell = entry.shell.as_deref().map(redact);

        self.conn.execute(
            "INSERT INTO history
                (command, exit_code, duration_ms, cwd, project_root, shell, started_at, finished_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                command.as_ref(),
                entry.exit_code,
                entry.duration_ms,
                cwd.as_ref(),
                project_root.as_deref(),
                shell.as_deref(),
                entry.started_at,
                entry.finished_at,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Update an existing command's `exit_code`, `duration_ms`, and `finished_at` timestamp.
    /// Returns `true` if a row was updated.
    pub fn finish_command(
        &self,
        id: i64,
        exit_code: Option<i32>,
        duration_ms: Option<i64>,
    ) -> Result<bool, StoreError> {
        let finished_at = now_epoch_ms();
        let changed = self.conn.execute(
            "UPDATE history
                SET exit_code = ?1, duration_ms = ?2, finished_at = ?3
              WHERE id = ?4",
            params![exit_code, duration_ms, finished_at, id],
        )?;
        Ok(changed > 0)
    }

    /// Full-text search over `history` via FTS5. Optionally scoped to `project_root`.
    pub fn search_history(
        &self,
        query: &str,
        project_root: Option<&str>,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>, StoreError> {
        let limit = limit.min(MAX_QUERY_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(fts_query) = escape_fts5_query(query) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        if let Some(project) = project_root {
            let mut stmt = self.conn.prepare(
                "SELECT h.id, h.command, h.exit_code, h.duration_ms, h.cwd,
                        h.project_root, h.shell, h.started_at, h.finished_at
                   FROM history_fts f
                   JOIN history h ON h.id = f.rowid
                  WHERE history_fts MATCH ?1
                    AND h.project_root = ?2
                  ORDER BY bm25(history_fts), h.started_at DESC, h.id DESC
                  LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![fts_query, project, limit as i64], map_history)?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT h.id, h.command, h.exit_code, h.duration_ms, h.cwd,
                        h.project_root, h.shell, h.started_at, h.finished_at
                   FROM history_fts f
                   JOIN history h ON h.id = f.rowid
                  WHERE history_fts MATCH ?1
                  ORDER BY bm25(history_fts), h.started_at DESC, h.id DESC
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![fts_query, limit as i64], map_history)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    /// Most recent commands, newest first, optionally filtered by `project_root`.
    pub fn recent_history(
        &self,
        project_root: Option<&str>,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>, StoreError> {
        let limit = limit.min(MAX_QUERY_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        if let Some(project) = project_root {
            let mut stmt = self.conn.prepare(
                "SELECT id, command, exit_code, duration_ms, cwd,
                        project_root, shell, started_at, finished_at
                   FROM history
                  WHERE project_root = ?1
                  ORDER BY started_at DESC, id DESC
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![project, limit as i64], map_history)?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id, command, exit_code, duration_ms, cwd,
                        project_root, shell, started_at, finished_at
                   FROM history
                  ORDER BY started_at DESC, id DESC
                  LIMIT ?1",
            )?;
            let rows = stmt.query_map(params![limit as i64], map_history)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    // ---- Project memory ----

    /// Create a memory record (redacted before writing). Returns its row id.
    pub fn create_memory(&self, entry: &NewMemory) -> Result<i64, StoreError> {
        let project_root = redact(&entry.project_root);
        let text = redact(&entry.text);
        let source = redact(&entry.source);
        let now = now_epoch_ms();
        self.conn.execute(
            "INSERT INTO memory
                (project_root, kind, text, source, created_at, updated_at, approved)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                project_root.as_ref(),
                entry.kind.as_str(),
                text.as_ref(),
                source.as_ref(),
                now,
                now,
                i32::from(entry.approved),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Fetch a single memory record by `id`.
    pub fn get_memory(&self, id: i64) -> Result<Option<MemoryEntry>, StoreError> {
        self.conn
            .query_row(
                "SELECT id, project_root, kind, text, source, created_at, updated_at, approved
                   FROM memory
                  WHERE id = ?1",
                params![id],
                map_memory,
            )
            .optional()
            .map_err(StoreError::from)
    }

    /// List memories for `project_root`, newest-updated first.
    pub fn list_memory(
        &self,
        project_root: &str,
        approved_only: bool,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let limit = limit.min(MAX_QUERY_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        if approved_only {
            let mut stmt = self.conn.prepare(
                "SELECT id, project_root, kind, text, source, created_at, updated_at, approved
                   FROM memory
                  WHERE project_root = ?1 AND approved = 1
                  ORDER BY updated_at DESC, id DESC
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![project_root, limit as i64], map_memory)?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT id, project_root, kind, text, source, created_at, updated_at, approved
                   FROM memory
                  WHERE project_root = ?1
                  ORDER BY updated_at DESC, id DESC
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![project_root, limit as i64], map_memory)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    /// Update fields of a memory record (redacting any updated text/source).
    /// Returns `true` if the row existed.
    pub fn update_memory(&self, id: i64, update: &MemoryUpdate) -> Result<bool, StoreError> {
        let Some(existing) = self.get_memory(id)? else {
            return Ok(false);
        };
        let kind = update.kind.unwrap_or(existing.kind);
        let text = match update.text.as_deref() {
            Some(t) => redact(t).into_owned(),
            None => existing.text,
        };
        let source = match update.source.as_deref() {
            Some(s) => redact(s).into_owned(),
            None => existing.source,
        };
        let approved = update.approved.unwrap_or(existing.approved);
        let now = now_epoch_ms();
        let changed = self.conn.execute(
            "UPDATE memory
                SET kind = ?1, text = ?2, source = ?3, updated_at = ?4, approved = ?5
              WHERE id = ?6",
            params![kind.as_str(), text, source, now, i32::from(approved), id],
        )?;
        Ok(changed > 0)
    }

    /// Mark a memory record as approved or unapproved. Returns `true` if the row existed.
    pub fn set_memory_approved(&self, id: i64, approved: bool) -> Result<bool, StoreError> {
        let now = now_epoch_ms();
        let changed = self.conn.execute(
            "UPDATE memory SET approved = ?1, updated_at = ?2 WHERE id = ?3",
            params![i32::from(approved), now, id],
        )?;
        Ok(changed > 0)
    }

    /// Delete a memory record by `id`. Returns `true` if a row was removed.
    pub fn delete_memory(&self, id: i64) -> Result<bool, StoreError> {
        let changed = self
            .conn
            .execute("DELETE FROM memory WHERE id = ?1", params![id])?;
        Ok(changed > 0)
    }

    /// Full-text search over `memory` via FTS5, scoped to `project_root` when provided.
    pub fn search_memory(
        &self,
        project_root: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let limit = limit.min(MAX_QUERY_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(fts_query) = escape_fts5_query(query) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        if let Some(project) = project_root {
            let mut stmt = self.conn.prepare(
                "SELECT m.id, m.project_root, m.kind, m.text, m.source,
                        m.created_at, m.updated_at, m.approved
                   FROM memory_fts f
                   JOIN memory m ON m.id = f.rowid
                  WHERE memory_fts MATCH ?1
                    AND m.project_root = ?2
                  ORDER BY bm25(memory_fts), m.updated_at DESC, m.id DESC
                  LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![fts_query, project, limit as i64], map_memory)?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT m.id, m.project_root, m.kind, m.text, m.source,
                        m.created_at, m.updated_at, m.approved
                   FROM memory_fts f
                   JOIN memory m ON m.id = f.rowid
                  WHERE memory_fts MATCH ?1
                  ORDER BY bm25(memory_fts), m.updated_at DESC, m.id DESC
                  LIMIT ?2",
            )?;
            let rows = stmt.query_map(params![fts_query, limit as i64], map_memory)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    /// List memories for `project_root`, optionally filtered by `kind` and approval status,
    /// newest-updated first.
    pub fn list_memory_by_kind(
        &self,
        project_root: &str,
        kind: Option<MemoryKind>,
        approved_only: bool,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let limit = limit.min(MAX_QUERY_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        match (kind, approved_only) {
            (Some(k), true) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, project_root, kind, text, source, created_at, updated_at, approved
                       FROM memory
                      WHERE project_root = ?1 AND kind = ?2 AND approved = 1
                      ORDER BY updated_at DESC, id DESC
                      LIMIT ?3",
                )?;
                let rows =
                    stmt.query_map(params![project_root, k.as_str(), limit as i64], map_memory)?;
                for row in rows {
                    out.push(row?);
                }
            }
            (Some(k), false) => {
                let mut stmt = self.conn.prepare(
                    "SELECT id, project_root, kind, text, source, created_at, updated_at, approved
                       FROM memory
                      WHERE project_root = ?1 AND kind = ?2
                      ORDER BY approved ASC, updated_at DESC, id DESC
                      LIMIT ?3",
                )?;
                let rows =
                    stmt.query_map(params![project_root, k.as_str(), limit as i64], map_memory)?;
                for row in rows {
                    out.push(row?);
                }
            }
            (None, approved_only) => {
                return self.list_memory(project_root, approved_only, limit);
            }
        }
        Ok(out)
    }

    /// Full-text search over approved `memory` rows for `project_root`, optionally filtered by
    /// `kind`, ordered deterministically by BM25 score then `updated_at DESC, id DESC`.
    pub fn search_approved_memory(
        &self,
        project_root: &str,
        query: &str,
        kind: Option<MemoryKind>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let limit = limit.min(MAX_QUERY_LIMIT);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let Some(fts_query) = escape_fts5_or_query(query) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        if let Some(k) = kind {
            let mut stmt = self.conn.prepare(
                "SELECT m.id, m.project_root, m.kind, m.text, m.source,
                        m.created_at, m.updated_at, m.approved
                   FROM memory_fts f
                   JOIN memory m ON m.id = f.rowid
                  WHERE memory_fts MATCH ?1
                    AND m.project_root = ?2
                    AND m.approved = 1
                    AND m.kind = ?3
                  ORDER BY bm25(memory_fts), m.updated_at DESC, m.id DESC
                  LIMIT ?4",
            )?;
            let rows = stmt.query_map(
                params![fts_query, project_root, k.as_str(), limit as i64],
                map_memory,
            )?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT m.id, m.project_root, m.kind, m.text, m.source,
                        m.created_at, m.updated_at, m.approved
                   FROM memory_fts f
                   JOIN memory m ON m.id = f.rowid
                  WHERE memory_fts MATCH ?1
                    AND m.project_root = ?2
                    AND m.approved = 1
                  ORDER BY bm25(memory_fts), m.updated_at DESC, m.id DESC
                  LIMIT ?3",
            )?;
            let rows =
                stmt.query_map(params![fts_query, project_root, limit as i64], map_memory)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    /// Delete all stored memories, rules-file approvals, and project-scoped history for
    /// `project_root` in one transaction ("Forget this project").
    pub fn forget_project(&self, project_root: &str) -> Result<ForgetReport, StoreError> {
        self.with_tx(|| {
            let memories_deleted = self.conn.execute(
                "DELETE FROM memory WHERE project_root = ?1",
                params![project_root],
            )?;
            let rules_approvals_deleted = self.conn.execute(
                "DELETE FROM rules_file_approvals WHERE project_root = ?1",
                params![project_root],
            )?;
            let history_deleted = self.conn.execute(
                "DELETE FROM history WHERE project_root = ?1",
                params![project_root],
            )?;
            Ok(ForgetReport {
                memories_deleted,
                rules_approvals_deleted,
                history_deleted,
            })
        })
    }

    // ---- Project rules-file approvals ----

    /// Record or update the user's approval of `rel_path` under `project_root` for `sha256`.
    pub fn approve_rules_file(
        &self,
        project_root: &str,
        rel_path: &str,
        sha256: &str,
    ) -> Result<(), StoreError> {
        if project_root.is_empty() || rel_path.is_empty() || sha256.is_empty() {
            return Err(StoreError::InvalidInput(
                "rules approval requires non-empty project_root, rel_path, and sha256",
            ));
        }
        let project_root = redact(project_root);
        let rel_path = redact(rel_path);
        let sha256 = redact(sha256);
        let now = now_epoch_ms();
        self.conn.execute(
            "INSERT INTO rules_file_approvals (project_root, rel_path, sha256, approved_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(project_root, rel_path) DO UPDATE SET
                sha256 = excluded.sha256,
                approved_at = excluded.approved_at",
            params![
                project_root.as_ref(),
                rel_path.as_ref(),
                sha256.as_ref(),
                now
            ],
        )?;
        Ok(())
    }

    /// Revoke a previously stored rules-file approval. Returns `true` if a row was removed.
    pub fn revoke_rules_file(
        &self,
        project_root: &str,
        rel_path: &str,
    ) -> Result<bool, StoreError> {
        let changed = self.conn.execute(
            "DELETE FROM rules_file_approvals WHERE project_root = ?1 AND rel_path = ?2",
            params![project_root, rel_path],
        )?;
        Ok(changed > 0)
    }

    /// List all stored rules-file approvals for `project_root`.
    pub fn list_rules_approvals(
        &self,
        project_root: &str,
    ) -> Result<Vec<RulesApproval>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT project_root, rel_path, sha256, approved_at
               FROM rules_file_approvals
              WHERE project_root = ?1
              ORDER BY rel_path ASC",
        )?;
        let rows = stmt.query_map(params![project_root], |row| {
            Ok(RulesApproval {
                project_root: row.get(0)?,
                rel_path: row.get(1)?,
                sha256: row.get(2)?,
                approved_at: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Inspect candidate rules files in `project_root` against stored SHA-256 approvals.
    ///
    /// Only files whose current SHA-256 matches a stored approval have `approved_content`
    /// populated; changed files silently lapse to [`crate::RulesFileState::Changed`].
    pub fn inspect_rules_files(
        &self,
        project_root: &Path,
    ) -> Result<Vec<RulesFileInfo>, StoreError> {
        let key = project_root.to_string_lossy();
        let approvals = self.list_rules_approvals(&key)?;
        Ok(inspect_rules_files_with_approvals(project_root, &approvals))
    }

    // ---- Environment notes & frequent commands ----

    /// Refresh auto-detected environment notes (`kind = 'env'`, `source = 'auto:env'`) for
    /// `project_root` while preserving user-written environment notes.
    pub fn refresh_env_notes(&self, project_root: &Path) -> Result<Vec<MemoryEntry>, StoreError> {
        let key = project_root.to_string_lossy().into_owned();
        let detected = detect_project_env(project_root);
        let existing = self.list_memory_by_kind(&key, Some(MemoryKind::Env), false, 100)?;
        let existing_auto: Vec<&str> = existing
            .iter()
            .filter(|e| e.source == AUTO_ENV_SOURCE)
            .map(|e| e.text.as_str())
            .collect();
        let detected_texts: Vec<&str> = detected.iter().map(|n| n.text.as_str()).collect();
        let unchanged = existing_auto.len() == detected_texts.len()
            && detected_texts.iter().all(|t| existing_auto.contains(t));
        if !unchanged {
            let project_redacted = redact(&key);
            let now = now_epoch_ms();
            self.with_tx(|| {
                self.conn.execute(
                    "DELETE FROM memory
                      WHERE project_root = ?1 AND kind = 'env' AND source = ?2",
                    params![project_redacted.as_ref(), AUTO_ENV_SOURCE],
                )?;
                for note in &detected {
                    let text = redact(&note.text);
                    self.conn.execute(
                        "INSERT INTO memory
                            (project_root, kind, text, source, created_at, updated_at, approved)
                         VALUES (?1, 'env', ?2, ?3, ?4, ?5, 1)",
                        params![
                            project_redacted.as_ref(),
                            text.as_ref(),
                            AUTO_ENV_SOURCE,
                            now,
                            now,
                        ],
                    )?;
                }
                Ok(())
            })?;
        }
        self.list_memory_by_kind(&key, Some(MemoryKind::Env), false, 100)
    }

    /// Compute frequent commands with success rates from `history` for `project_root`.
    pub fn frequent_commands(
        &self,
        project_root: &str,
        min_runs: usize,
        limit: usize,
    ) -> Result<Vec<FrequentCommand>, StoreError> {
        let history = self.recent_history(Some(project_root), MAX_QUERY_LIMIT)?;
        Ok(compute_frequent_commands(&history, min_runs, limit))
    }

    /// Recompute frequent commands from `history` and sync `kind = 'command'`,
    /// `source = 'auto:history'` rows in `memory` (leaving user-authored command rows intact).
    pub fn sync_frequent_commands(
        &self,
        project_root: &str,
        min_runs: usize,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let frequent = self.frequent_commands(project_root, min_runs, limit)?;
        let notes: Vec<String> = frequent.iter().map(FrequentCommand::format_note).collect();
        let existing =
            self.list_memory_by_kind(project_root, Some(MemoryKind::Command), false, 100)?;
        let existing_auto: Vec<&str> = existing
            .iter()
            .filter(|e| e.source == AUTO_COMMAND_SOURCE)
            .map(|e| e.text.as_str())
            .collect();
        let unchanged = existing_auto.len() == notes.len()
            && notes.iter().all(|t| existing_auto.contains(&t.as_str()));
        if !unchanged {
            let project_redacted = redact(project_root);
            let now = now_epoch_ms();
            self.with_tx(|| {
                self.conn.execute(
                    "DELETE FROM memory
                      WHERE project_root = ?1 AND kind = 'command' AND source = ?2",
                    params![project_redacted.as_ref(), AUTO_COMMAND_SOURCE],
                )?;
                for text in &notes {
                    let text = redact(text);
                    self.conn.execute(
                        "INSERT INTO memory
                            (project_root, kind, text, source, created_at, updated_at, approved)
                         VALUES (?1, 'command', ?2, ?3, ?4, ?5, 1)",
                        params![
                            project_redacted.as_ref(),
                            text.as_ref(),
                            AUTO_COMMAND_SOURCE,
                            now,
                            now,
                        ],
                    )?;
                }
                Ok(())
            })?;
        }
        self.list_memory_by_kind(project_root, Some(MemoryKind::Command), false, 100)
    }

    /// Load a complete [`ProjectMemorySnapshot`] for the Memory screen.
    ///
    /// When `project_root` matches `excludes`, nothing is refreshed or returned from the store.
    pub fn load_project_snapshot(
        &self,
        project_root: &Path,
        excludes: &[String],
    ) -> Result<ProjectMemorySnapshot, StoreError> {
        let key = project_root.to_string_lossy().into_owned();
        if is_path_excluded(project_root, excludes) {
            return Ok(ProjectMemorySnapshot {
                project_root: key,
                excluded: true,
                rules_files: Vec::new(),
                entries: Vec::new(),
                frequent_commands: Vec::new(),
            });
        }
        let _ = self.refresh_env_notes(project_root);
        let frequent_commands = self.frequent_commands(
            &key,
            DEFAULT_MIN_COMMAND_RUNS,
            DEFAULT_FREQUENT_COMMAND_LIMIT,
        )?;
        let _ = self.sync_frequent_commands(
            &key,
            DEFAULT_MIN_COMMAND_RUNS,
            DEFAULT_FREQUENT_COMMAND_LIMIT,
        );
        let rules_files = self.inspect_rules_files(project_root)?;
        let entries = self.list_memory(&key, false, MAX_QUERY_LIMIT)?;
        Ok(ProjectMemorySnapshot {
            project_root: key,
            excluded: false,
            rules_files,
            entries,
            frequent_commands,
        })
    }

    fn with_tx<T>(&self, f: impl FnOnce() -> Result<T, StoreError>) -> Result<T, StoreError> {
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        match f() {
            Ok(val) => {
                self.conn.execute_batch("COMMIT")?;
                Ok(val)
            }
            Err(err) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    }

    // ---- Drive index ----

    /// Insert or update a `drive_index` record (all text fields redacted before writing).
    pub fn upsert_drive_index(&self, entry: &DriveIndexEntry) -> Result<(), StoreError> {
        if entry.path.is_empty() {
            return Err(StoreError::InvalidInput("drive_index path cannot be empty"));
        }
        let path = redact(&entry.path);
        let kind = redact(&entry.kind);
        let title = redact(&entry.title);
        let hash = redact(&entry.hash);
        self.conn.execute(
            "INSERT INTO drive_index (path, kind, title, updated_at, hash)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                kind = excluded.kind,
                title = excluded.title,
                updated_at = excluded.updated_at,
                hash = excluded.hash",
            params![
                path.as_ref(),
                kind.as_ref(),
                title.as_ref(),
                entry.updated_at,
                hash.as_ref(),
            ],
        )?;
        Ok(())
    }

    /// Remove a `drive_index` record by `path`. Returns `true` if a row was removed.
    pub fn remove_drive_index(&self, path: &str) -> Result<bool, StoreError> {
        let changed = self
            .conn
            .execute("DELETE FROM drive_index WHERE path = ?1", params![path])?;
        Ok(changed > 0)
    }

    /// List `drive_index` entries, optionally filtered by `kind`, newest-updated first.
    pub fn list_drive_index(&self, kind: Option<&str>) -> Result<Vec<DriveIndexEntry>, StoreError> {
        let mut out = Vec::new();
        if let Some(kind) = kind {
            let mut stmt = self.conn.prepare(
                "SELECT path, kind, title, updated_at, hash
                   FROM drive_index
                  WHERE kind = ?1
                  ORDER BY updated_at DESC, path ASC",
            )?;
            let rows = stmt.query_map(params![kind], map_drive_index)?;
            for row in rows {
                out.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT path, kind, title, updated_at, hash
                   FROM drive_index
                  ORDER BY updated_at DESC, path ASC",
            )?;
            let rows = stmt.query_map([], map_drive_index)?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    /// Expose the schema version constant for callers inspecting compatibility.
    pub const fn supported_schema_version() -> u32 {
        CURRENT_SCHEMA_VERSION
    }
}

fn configure_connection(conn: &Connection, file_backed: bool) -> Result<(), StoreError> {
    conn.pragma_update(None, "busy_timeout", 5000i64)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    if file_backed {
        // `PRAGMA journal_mode = WAL` returns a row, so query_row is required.
        let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
    }
    Ok(())
}

fn map_history(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: row.get(0)?,
        command: row.get(1)?,
        exit_code: row.get(2)?,
        duration_ms: row.get(3)?,
        cwd: row.get(4)?,
        project_root: row.get(5)?,
        shell: row.get(6)?,
        started_at: row.get(7)?,
        finished_at: row.get(8)?,
    })
}

fn map_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryEntry> {
    let kind_str: String = row.get(2)?;
    let kind = MemoryKind::parse(&kind_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            2,
            rusqlite::types::Type::Text,
            Box::new(StoreError::InvalidInput("unknown memory kind")),
        )
    })?;
    let approved: i64 = row.get(7)?;
    Ok(MemoryEntry {
        id: row.get(0)?,
        project_root: row.get(1)?,
        kind,
        text: row.get(3)?,
        source: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        approved: approved != 0,
    })
}

fn map_drive_index(row: &rusqlite::Row<'_>) -> rusqlite::Result<DriveIndexEntry> {
    Ok(DriveIndexEntry {
        path: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        updated_at: row.get(3)?,
        hash: row.get(4)?,
    })
}
