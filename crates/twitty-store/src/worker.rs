//! Cloneable, async-free worker handle running [`Store`] on a dedicated background thread.
//!
//! The UI thread never blocks on SQLite: opening the database, running migrations, resolving git
//! project roots, and persisting history all happen on the `twitty-store` thread. If opening or
//! writing fails, the error is logged once and the app continues normally without a database.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use crate::commands::{FrequentCommand, DEFAULT_FREQUENT_COMMAND_LIMIT, DEFAULT_MIN_COMMAND_RUNS};
use crate::error::StoreError;
use crate::paths::{default_db_path, resolve_project_root};
use crate::project::{is_path_excluded, is_project_excluded};
use crate::rules::{RulesApproval, RulesFileInfo};
use crate::store::{
    DriveIndexEntry, ForgetReport, HistoryEntry, MemoryEntry, MemoryKind, MemoryUpdate, NewCommand,
    NewMemory, ProjectMemorySnapshot, Store,
};

/// Bounded channel capacity so a command flood cannot grow memory unboundedly if disk I/O stalls.
const CHANNEL_CAPACITY: usize = 512;

enum StoreTarget {
    Path(PathBuf),
    InMemory,
}

enum StoreOp {
    RecordCommand {
        entry: NewCommand,
        reply: Option<mpsc::Sender<Result<i64, StoreError>>>,
    },
    RecordFinishedBlock {
        command: String,
        exit_code: Option<i32>,
        duration_ms: Option<i64>,
        cwd: PathBuf,
        resolve_git_root: bool,
        excludes: Vec<String>,
        shell: Option<String>,
        started_at: i64,
        finished_at: Option<i64>,
    },
    FinishCommand {
        id: i64,
        exit_code: Option<i32>,
        duration_ms: Option<i64>,
        reply: Option<mpsc::Sender<Result<bool, StoreError>>>,
    },
    SearchHistory {
        query: String,
        project_root: Option<String>,
        limit: usize,
        reply: mpsc::Sender<Result<Vec<HistoryEntry>, StoreError>>,
    },
    RecentHistory {
        project_root: Option<String>,
        limit: usize,
        reply: mpsc::Sender<Result<Vec<HistoryEntry>, StoreError>>,
    },
    CreateMemory {
        entry: NewMemory,
        excludes: Vec<String>,
        reply: Option<mpsc::Sender<Result<i64, StoreError>>>,
    },
    GetMemory {
        id: i64,
        reply: mpsc::Sender<Result<Option<MemoryEntry>, StoreError>>,
    },
    ListMemory {
        project_root: String,
        approved_only: bool,
        limit: usize,
        reply: mpsc::Sender<Result<Vec<MemoryEntry>, StoreError>>,
    },
    ListMemoryByKind {
        project_root: String,
        kind: Option<MemoryKind>,
        approved_only: bool,
        limit: usize,
        reply: mpsc::Sender<Result<Vec<MemoryEntry>, StoreError>>,
    },
    UpdateMemory {
        id: i64,
        update: MemoryUpdate,
        reply: mpsc::Sender<Result<bool, StoreError>>,
    },
    SetMemoryApproved {
        id: i64,
        approved: bool,
        reply: mpsc::Sender<Result<bool, StoreError>>,
    },
    DeleteMemory {
        id: i64,
        reply: mpsc::Sender<Result<bool, StoreError>>,
    },
    SearchMemory {
        project_root: Option<String>,
        query: String,
        limit: usize,
        reply: mpsc::Sender<Result<Vec<MemoryEntry>, StoreError>>,
    },
    SearchApprovedMemory {
        project_root: String,
        query: String,
        kind: Option<MemoryKind>,
        limit: usize,
        reply: mpsc::Sender<Result<Vec<MemoryEntry>, StoreError>>,
    },
    ForgetProject {
        project_root: String,
        reply: mpsc::Sender<Result<ForgetReport, StoreError>>,
    },
    ApproveRulesFile {
        project_root: String,
        rel_path: String,
        sha256: String,
        reply: mpsc::Sender<Result<(), StoreError>>,
    },
    RevokeRulesFile {
        project_root: String,
        rel_path: String,
        reply: mpsc::Sender<Result<bool, StoreError>>,
    },
    ListRulesApprovals {
        project_root: String,
        reply: mpsc::Sender<Result<Vec<RulesApproval>, StoreError>>,
    },
    InspectRulesFiles {
        project_root: PathBuf,
        reply: mpsc::Sender<Result<Vec<RulesFileInfo>, StoreError>>,
    },
    RefreshEnvNotes {
        project_root: PathBuf,
        excludes: Vec<String>,
        reply: Option<mpsc::Sender<Result<Vec<MemoryEntry>, StoreError>>>,
    },
    RefreshProjectMemory {
        project_root: PathBuf,
        excludes: Vec<String>,
    },
    FrequentCommands {
        project_root: String,
        min_runs: usize,
        limit: usize,
        reply: mpsc::Sender<Result<Vec<FrequentCommand>, StoreError>>,
    },
    SyncFrequentCommands {
        project_root: String,
        min_runs: usize,
        limit: usize,
        reply: Option<mpsc::Sender<Result<Vec<MemoryEntry>, StoreError>>>,
    },
    LoadProjectSnapshot {
        project_root: PathBuf,
        excludes: Vec<String>,
        reply: mpsc::Sender<Result<ProjectMemorySnapshot, StoreError>>,
    },
    UpsertDriveIndex {
        entry: DriveIndexEntry,
        reply: mpsc::Sender<Result<(), StoreError>>,
    },
    RemoveDriveIndex {
        path: String,
        reply: mpsc::Sender<Result<bool, StoreError>>,
    },
    ListDriveIndex {
        kind: Option<String>,
        reply: mpsc::Sender<Result<Vec<DriveIndexEntry>, StoreError>>,
    },
    Flush {
        reply: mpsc::Sender<()>,
    },
}

/// Cloneable handle to the background SQLite store thread.
#[derive(Clone, Debug)]
pub struct StoreHandle {
    tx: Option<mpsc::SyncSender<StoreOp>>,
    send_warned: Arc<AtomicBool>,
}

impl StoreHandle {
    /// Spawn a background worker opening the default database path. Never blocks the caller.
    pub fn open_default() -> Self {
        match default_db_path() {
            Some(path) => Self::open(path),
            None => {
                tracing::warn!(
                    "twitty-store: could not resolve data directory; running without DB"
                );
                Self::disabled()
            }
        }
    }

    /// Spawn a background worker opening the database at `path`. Never blocks the caller.
    pub fn open(path: PathBuf) -> Self {
        Self::spawn(StoreTarget::Path(path))
    }

    /// Spawn a background worker backed by an in-memory SQLite database (for tests).
    pub fn open_in_memory() -> Self {
        Self::spawn(StoreTarget::InMemory)
    }

    /// A no-op handle with no background thread.
    pub fn disabled() -> Self {
        Self {
            tx: None,
            send_warned: Arc::new(AtomicBool::new(false)),
        }
    }

    fn spawn(target: StoreTarget) -> Self {
        let (tx, rx) = mpsc::sync_channel::<StoreOp>(CHANNEL_CAPACITY);
        let spawned = thread::Builder::new()
            .name("twitty-store".into())
            .spawn(move || worker_loop(target, rx));
        match spawned {
            Ok(_) => Self {
                tx: Some(tx),
                send_warned: Arc::new(AtomicBool::new(false)),
            },
            Err(err) => {
                tracing::warn!(
                    "twitty-store: failed to spawn worker thread ({err}); running without DB"
                );
                Self::disabled()
            }
        }
    }

    fn send_fire_and_forget(&self, op: StoreOp) {
        let Some(ref tx) = self.tx else {
            return;
        };
        if let Err(err) = tx.try_send(op) {
            if !self.send_warned.swap(true, Ordering::Relaxed) {
                tracing::warn!("twitty-store: dropping background write ({err})");
            }
        }
    }

    fn request<T>(
        &self,
        build: impl FnOnce(mpsc::Sender<Result<T, StoreError>>) -> StoreOp,
    ) -> Result<T, StoreError> {
        let Some(ref tx) = self.tx else {
            return Err(StoreError::Unavailable);
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        tx.send(build(reply_tx))
            .map_err(|_| StoreError::Unavailable)?;
        reply_rx.recv().map_err(|_| StoreError::Unavailable)?
    }

    // ---- Non-blocking UI-thread writes ----

    /// Enqueue a finished command block for persistence off the UI thread.
    ///
    /// When `resolve_git_root` is true, the worker resolves the nearest `.git` ancestor of `cwd`
    /// off the UI thread (falling back to `cwd`). Never blocks the caller.
    #[allow(clippy::too_many_arguments)]
    pub fn record_finished_block_async(
        &self,
        command: String,
        exit_code: Option<i32>,
        duration_ms: Option<i64>,
        cwd: PathBuf,
        resolve_git_root: bool,
        shell: Option<String>,
        started_at: i64,
        finished_at: Option<i64>,
    ) {
        self.record_finished_block_with_excludes_async(
            command,
            exit_code,
            duration_ms,
            cwd,
            resolve_git_root,
            Vec::new(),
            shell,
            started_at,
            finished_at,
        );
    }

    /// Like [`Self::record_finished_block_async`], skipping persistence when `cwd` or its
    /// resolved project root matches `excludes` (`config.memory_exclude`). Never blocks.
    #[allow(clippy::too_many_arguments)]
    pub fn record_finished_block_with_excludes_async(
        &self,
        command: String,
        exit_code: Option<i32>,
        duration_ms: Option<i64>,
        cwd: PathBuf,
        resolve_git_root: bool,
        excludes: Vec<String>,
        shell: Option<String>,
        started_at: i64,
        finished_at: Option<i64>,
    ) {
        self.send_fire_and_forget(StoreOp::RecordFinishedBlock {
            command,
            exit_code,
            duration_ms,
            cwd,
            resolve_git_root,
            excludes,
            shell,
            started_at,
            finished_at,
        });
    }

    /// Enqueue `record_command` without waiting for a reply (never blocks).
    pub fn record_command_async(&self, entry: NewCommand) {
        self.send_fire_and_forget(StoreOp::RecordCommand { entry, reply: None });
    }

    /// Enqueue `finish_command` without waiting for a reply (never blocks).
    pub fn finish_command_async(&self, id: i64, exit_code: Option<i32>, duration_ms: Option<i64>) {
        self.send_fire_and_forget(StoreOp::FinishCommand {
            id,
            exit_code,
            duration_ms,
            reply: None,
        });
    }

    /// Enqueue `create_memory` without waiting for a reply (never blocks).
    pub fn create_memory_async(&self, entry: NewMemory) {
        self.send_fire_and_forget(StoreOp::CreateMemory {
            entry,
            excludes: Vec::new(),
            reply: None,
        });
    }

    /// Enqueue `create_memory` off the UI thread, skipping when `entry.project_root` matches
    /// `excludes`.
    pub fn create_memory_if_not_excluded_async(&self, entry: NewMemory, excludes: Vec<String>) {
        self.send_fire_and_forget(StoreOp::CreateMemory {
            entry,
            excludes,
            reply: None,
        });
    }

    /// Refresh environment notes for `project_root` off the UI thread, skipping excluded folders.
    pub fn refresh_env_notes_async(&self, project_root: PathBuf, excludes: Vec<String>) {
        self.send_fire_and_forget(StoreOp::RefreshEnvNotes {
            project_root,
            excludes,
            reply: None,
        });
    }

    /// Refresh environment notes and frequent commands for `project_root` off the UI thread.
    pub fn refresh_project_memory_async(&self, project_root: PathBuf, excludes: Vec<String>) {
        self.send_fire_and_forget(StoreOp::RefreshProjectMemory {
            project_root,
            excludes,
        });
    }

    // ---- Request/reply methods (for background callers and tests) ----

    pub fn record_command(&self, entry: NewCommand) -> Result<i64, StoreError> {
        self.request(|reply| StoreOp::RecordCommand {
            entry,
            reply: Some(reply),
        })
    }

    pub fn finish_command(
        &self,
        id: i64,
        exit_code: Option<i32>,
        duration_ms: Option<i64>,
    ) -> Result<bool, StoreError> {
        self.request(|reply| StoreOp::FinishCommand {
            id,
            exit_code,
            duration_ms,
            reply: Some(reply),
        })
    }

    pub fn search_history(
        &self,
        query: impl Into<String>,
        project_root: Option<String>,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>, StoreError> {
        let query = query.into();
        self.request(|reply| StoreOp::SearchHistory {
            query,
            project_root,
            limit,
            reply,
        })
    }

    pub fn recent_history(
        &self,
        project_root: Option<String>,
        limit: usize,
    ) -> Result<Vec<HistoryEntry>, StoreError> {
        self.request(|reply| StoreOp::RecentHistory {
            project_root,
            limit,
            reply,
        })
    }

    pub fn create_memory(&self, entry: NewMemory) -> Result<i64, StoreError> {
        self.request(|reply| StoreOp::CreateMemory {
            entry,
            excludes: Vec::new(),
            reply: Some(reply),
        })
    }

    pub fn get_memory(&self, id: i64) -> Result<Option<MemoryEntry>, StoreError> {
        self.request(|reply| StoreOp::GetMemory { id, reply })
    }

    pub fn list_memory(
        &self,
        project_root: impl Into<String>,
        approved_only: bool,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let project_root = project_root.into();
        self.request(|reply| StoreOp::ListMemory {
            project_root,
            approved_only,
            limit,
            reply,
        })
    }

    pub fn list_memory_by_kind(
        &self,
        project_root: impl Into<String>,
        kind: Option<MemoryKind>,
        approved_only: bool,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let project_root = project_root.into();
        self.request(|reply| StoreOp::ListMemoryByKind {
            project_root,
            kind,
            approved_only,
            limit,
            reply,
        })
    }

    pub fn update_memory(&self, id: i64, update: MemoryUpdate) -> Result<bool, StoreError> {
        self.request(|reply| StoreOp::UpdateMemory { id, update, reply })
    }

    pub fn set_memory_approved(&self, id: i64, approved: bool) -> Result<bool, StoreError> {
        self.request(|reply| StoreOp::SetMemoryApproved {
            id,
            approved,
            reply,
        })
    }

    pub fn delete_memory(&self, id: i64) -> Result<bool, StoreError> {
        self.request(|reply| StoreOp::DeleteMemory { id, reply })
    }

    pub fn search_memory(
        &self,
        project_root: Option<String>,
        query: impl Into<String>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let query = query.into();
        self.request(|reply| StoreOp::SearchMemory {
            project_root,
            query,
            limit,
            reply,
        })
    }

    pub fn search_approved_memory(
        &self,
        project_root: impl Into<String>,
        query: impl Into<String>,
        kind: Option<MemoryKind>,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let project_root = project_root.into();
        let query = query.into();
        self.request(|reply| StoreOp::SearchApprovedMemory {
            project_root,
            query,
            kind,
            limit,
            reply,
        })
    }

    pub fn forget_project(
        &self,
        project_root: impl Into<String>,
    ) -> Result<ForgetReport, StoreError> {
        let project_root = project_root.into();
        self.request(|reply| StoreOp::ForgetProject {
            project_root,
            reply,
        })
    }

    pub fn approve_rules_file(
        &self,
        project_root: impl Into<String>,
        rel_path: impl Into<String>,
        sha256: impl Into<String>,
    ) -> Result<(), StoreError> {
        let project_root = project_root.into();
        let rel_path = rel_path.into();
        let sha256 = sha256.into();
        self.request(|reply| StoreOp::ApproveRulesFile {
            project_root,
            rel_path,
            sha256,
            reply,
        })
    }

    pub fn revoke_rules_file(
        &self,
        project_root: impl Into<String>,
        rel_path: impl Into<String>,
    ) -> Result<bool, StoreError> {
        let project_root = project_root.into();
        let rel_path = rel_path.into();
        self.request(|reply| StoreOp::RevokeRulesFile {
            project_root,
            rel_path,
            reply,
        })
    }

    pub fn list_rules_approvals(
        &self,
        project_root: impl Into<String>,
    ) -> Result<Vec<RulesApproval>, StoreError> {
        let project_root = project_root.into();
        self.request(|reply| StoreOp::ListRulesApprovals {
            project_root,
            reply,
        })
    }

    pub fn inspect_rules_files(
        &self,
        project_root: PathBuf,
    ) -> Result<Vec<RulesFileInfo>, StoreError> {
        self.request(|reply| StoreOp::InspectRulesFiles {
            project_root,
            reply,
        })
    }

    pub fn refresh_env_notes(&self, project_root: PathBuf) -> Result<Vec<MemoryEntry>, StoreError> {
        self.request(|reply| StoreOp::RefreshEnvNotes {
            project_root,
            excludes: Vec::new(),
            reply: Some(reply),
        })
    }

    pub fn frequent_commands(
        &self,
        project_root: impl Into<String>,
        min_runs: usize,
        limit: usize,
    ) -> Result<Vec<FrequentCommand>, StoreError> {
        let project_root = project_root.into();
        self.request(|reply| StoreOp::FrequentCommands {
            project_root,
            min_runs,
            limit,
            reply,
        })
    }

    pub fn sync_frequent_commands(
        &self,
        project_root: impl Into<String>,
        min_runs: usize,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>, StoreError> {
        let project_root = project_root.into();
        self.request(|reply| StoreOp::SyncFrequentCommands {
            project_root,
            min_runs,
            limit,
            reply: Some(reply),
        })
    }

    pub fn load_project_snapshot(
        &self,
        project_root: PathBuf,
        excludes: Vec<String>,
    ) -> Result<ProjectMemorySnapshot, StoreError> {
        self.request(|reply| StoreOp::LoadProjectSnapshot {
            project_root,
            excludes,
            reply,
        })
    }

    pub fn upsert_drive_index(&self, entry: DriveIndexEntry) -> Result<(), StoreError> {
        self.request(|reply| StoreOp::UpsertDriveIndex { entry, reply })
    }

    pub fn remove_drive_index(&self, path: impl Into<String>) -> Result<bool, StoreError> {
        let path = path.into();
        self.request(|reply| StoreOp::RemoveDriveIndex { path, reply })
    }

    pub fn list_drive_index(
        &self,
        kind: Option<String>,
    ) -> Result<Vec<DriveIndexEntry>, StoreError> {
        self.request(|reply| StoreOp::ListDriveIndex { kind, reply })
    }

    /// Wait up to `timeout` for all prior queued operations to finish on the worker thread.
    pub fn flush_timeout(&self, timeout: Duration) -> bool {
        let Some(ref tx) = self.tx else {
            return true;
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        if tx.try_send(StoreOp::Flush { reply: reply_tx }).is_err() {
            return false;
        }
        reply_rx.recv_timeout(timeout).is_ok()
    }

    /// Wait for all prior queued operations to complete (useful in tests).
    pub fn flush(&self) {
        let Some(ref tx) = self.tx else {
            return;
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        if tx.send(StoreOp::Flush { reply: reply_tx }).is_ok() {
            let _ = reply_rx.recv();
        }
    }
}

fn worker_loop(target: StoreTarget, rx: mpsc::Receiver<StoreOp>) {
    let mut logged_error = false;
    let mut log_once = |msg: &str, err: &StoreError| {
        if !logged_error {
            logged_error = true;
            tracing::warn!("twitty-store: {msg}: {err}");
        }
    };

    let store = match target {
        StoreTarget::Path(path) => match Store::open(&path) {
            Ok(s) => {
                // Keep long-lived command history bounded and query planner stats fresh on startup.
                const DEFAULT_MAX_HISTORY_ROWS: usize = 50_000;
                if let Err(err) = s.prune_history(DEFAULT_MAX_HISTORY_ROWS) {
                    log_once("prune_history failed", &err);
                }
                let _ = s.optimize();
                Some(s)
            }
            Err(err) => {
                log_once("could not open database; continuing without storage", &err);
                None
            }
        },
        StoreTarget::InMemory => match Store::open_in_memory() {
            Ok(s) => Some(s),
            Err(err) => {
                log_once("could not open in-memory database", &err);
                None
            }
        },
    };

    while let Ok(op) = rx.recv() {
        match op {
            StoreOp::Flush { reply } => {
                let _ = reply.send(());
            }
            StoreOp::RecordCommand { entry, reply } => {
                let res = match store.as_ref() {
                    Some(s) => s.record_command(&entry),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("record_command failed", err);
                }
                if let Some(reply) = reply {
                    let _ = reply.send(res);
                }
            }
            StoreOp::RecordFinishedBlock {
                command,
                exit_code,
                duration_ms,
                cwd,
                resolve_git_root,
                excludes,
                shell,
                started_at,
                finished_at,
            } => {
                let Some(s) = store.as_ref() else {
                    continue;
                };
                let resolved_root = if cwd.as_os_str().is_empty() {
                    None
                } else if resolve_git_root {
                    Some(resolve_project_root(&cwd))
                } else {
                    Some(cwd.clone())
                };
                if let Some(ref root) = resolved_root {
                    if is_project_excluded(&cwd, root, &excludes) {
                        continue;
                    }
                }
                let project_root = resolved_root
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned());
                let entry = NewCommand {
                    command,
                    exit_code,
                    duration_ms,
                    cwd: cwd.to_string_lossy().into_owned(),
                    project_root,
                    shell,
                    started_at,
                    finished_at,
                };
                if let Err(ref err) = s.record_command(&entry) {
                    log_once("record_finished_block failed", err);
                }
            }
            StoreOp::FinishCommand {
                id,
                exit_code,
                duration_ms,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.finish_command(id, exit_code, duration_ms),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("finish_command failed", err);
                }
                if let Some(reply) = reply {
                    let _ = reply.send(res);
                }
            }
            StoreOp::SearchHistory {
                query,
                project_root,
                limit,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.search_history(&query, project_root.as_deref(), limit),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("search_history failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::RecentHistory {
                project_root,
                limit,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.recent_history(project_root.as_deref(), limit),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("recent_history failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::CreateMemory {
                entry,
                excludes,
                reply,
            } => {
                if !excludes.is_empty()
                    && is_path_excluded(std::path::Path::new(&entry.project_root), &excludes)
                {
                    if let Some(reply) = reply {
                        let _ = reply.send(Err(StoreError::InvalidInput("project is excluded")));
                    }
                    continue;
                }
                let res = match store.as_ref() {
                    Some(s) => s.create_memory(&entry),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("create_memory failed", err);
                }
                if let Some(reply) = reply {
                    let _ = reply.send(res);
                }
            }
            StoreOp::GetMemory { id, reply } => {
                let res = match store.as_ref() {
                    Some(s) => s.get_memory(id),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("get_memory failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::ListMemory {
                project_root,
                approved_only,
                limit,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.list_memory(&project_root, approved_only, limit),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("list_memory failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::ListMemoryByKind {
                project_root,
                kind,
                approved_only,
                limit,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.list_memory_by_kind(&project_root, kind, approved_only, limit),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("list_memory_by_kind failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::UpdateMemory { id, update, reply } => {
                let res = match store.as_ref() {
                    Some(s) => s.update_memory(id, &update),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("update_memory failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::SetMemoryApproved {
                id,
                approved,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.set_memory_approved(id, approved),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("set_memory_approved failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::DeleteMemory { id, reply } => {
                let res = match store.as_ref() {
                    Some(s) => s.delete_memory(id),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("delete_memory failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::SearchMemory {
                project_root,
                query,
                limit,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.search_memory(project_root.as_deref(), &query, limit),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("search_memory failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::SearchApprovedMemory {
                project_root,
                query,
                kind,
                limit,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.search_approved_memory(&project_root, &query, kind, limit),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("search_approved_memory failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::ForgetProject {
                project_root,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.forget_project(&project_root),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("forget_project failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::ApproveRulesFile {
                project_root,
                rel_path,
                sha256,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.approve_rules_file(&project_root, &rel_path, &sha256),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("approve_rules_file failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::RevokeRulesFile {
                project_root,
                rel_path,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.revoke_rules_file(&project_root, &rel_path),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("revoke_rules_file failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::ListRulesApprovals {
                project_root,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.list_rules_approvals(&project_root),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("list_rules_approvals failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::InspectRulesFiles {
                project_root,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.inspect_rules_files(&project_root),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("inspect_rules_files failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::RefreshEnvNotes {
                project_root,
                excludes,
                reply,
            } => {
                let resolved_root = crate::paths::resolve_project_root(&project_root);
                if !excludes.is_empty()
                    && crate::project::is_project_excluded(&project_root, &resolved_root, &excludes)
                {
                    if let Some(reply) = reply {
                        let _ = reply.send(Ok(Vec::new()));
                    }
                    continue;
                }
                let res = match store.as_ref() {
                    Some(s) => s.refresh_env_notes(&resolved_root),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("refresh_env_notes failed", err);
                }
                if let Some(reply) = reply {
                    let _ = reply.send(res);
                }
            }
            StoreOp::RefreshProjectMemory {
                project_root,
                excludes,
            } => {
                if !excludes.is_empty() && is_path_excluded(&project_root, &excludes) {
                    continue;
                }
                if let Some(s) = store.as_ref() {
                    if let Err(ref err) = s.refresh_env_notes(&project_root) {
                        log_once("refresh_env_notes failed", err);
                    }
                    let key = project_root.to_string_lossy();
                    if let Err(ref err) = s.sync_frequent_commands(
                        &key,
                        DEFAULT_MIN_COMMAND_RUNS,
                        DEFAULT_FREQUENT_COMMAND_LIMIT,
                    ) {
                        log_once("sync_frequent_commands failed", err);
                    }
                }
            }
            StoreOp::FrequentCommands {
                project_root,
                min_runs,
                limit,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.frequent_commands(&project_root, min_runs, limit),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("frequent_commands failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::SyncFrequentCommands {
                project_root,
                min_runs,
                limit,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.sync_frequent_commands(&project_root, min_runs, limit),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("sync_frequent_commands failed", err);
                }
                if let Some(reply) = reply {
                    let _ = reply.send(res);
                }
            }
            StoreOp::LoadProjectSnapshot {
                project_root,
                excludes,
                reply,
            } => {
                let res = match store.as_ref() {
                    Some(s) => s.load_project_snapshot(&project_root, &excludes),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("load_project_snapshot failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::UpsertDriveIndex { entry, reply } => {
                let res = match store.as_ref() {
                    Some(s) => s.upsert_drive_index(&entry),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("upsert_drive_index failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::RemoveDriveIndex { path, reply } => {
                let res = match store.as_ref() {
                    Some(s) => s.remove_drive_index(&path),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("remove_drive_index failed", err);
                }
                let _ = reply.send(res);
            }
            StoreOp::ListDriveIndex { kind, reply } => {
                let res = match store.as_ref() {
                    Some(s) => s.list_drive_index(kind.as_deref()),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("list_drive_index failed", err);
                }
                let _ = reply.send(res);
            }
        }
    }
}
