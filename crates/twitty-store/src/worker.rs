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

use crate::error::StoreError;
use crate::paths::{default_db_path, resolve_project_root};
use crate::store::{
    DriveIndexEntry, HistoryEntry, MemoryEntry, MemoryUpdate, NewCommand, NewMemory, Store,
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
        reply: mpsc::Sender<Result<i64, StoreError>>,
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
        self.send_fire_and_forget(StoreOp::RecordFinishedBlock {
            command,
            exit_code,
            duration_ms,
            cwd,
            resolve_git_root,
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
        self.request(|reply| StoreOp::CreateMemory { entry, reply })
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
            Ok(s) => Some(s),
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
                shell,
                started_at,
                finished_at,
            } => {
                let Some(s) = store.as_ref() else {
                    continue;
                };
                let project_root = if cwd.as_os_str().is_empty() {
                    None
                } else if resolve_git_root {
                    Some(resolve_project_root(&cwd).to_string_lossy().into_owned())
                } else {
                    Some(cwd.to_string_lossy().into_owned())
                };
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
            StoreOp::CreateMemory { entry, reply } => {
                let res = match store.as_ref() {
                    Some(s) => s.create_memory(&entry),
                    None => Err(StoreError::Unavailable),
                };
                if let Err(ref err) = res {
                    log_once("create_memory failed", err);
                }
                let _ = reply.send(res);
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
