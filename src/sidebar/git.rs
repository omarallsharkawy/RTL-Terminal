//! Bounded, process-free git HEAD discovery. Failed lookups are cached too.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const REFRESH: Duration = Duration::from_secs(2);
const MAX_FILE: u64 = 4096;

fn small_file(path: &Path) -> Option<String> {
    let is_small_file = |m: fs::Metadata| m.is_file() && m.len() <= MAX_FILE;
    if !is_small_file(path.symlink_metadata().ok()?) {
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A repository can replace a regular file with a FIFO or symlink between stat and open.
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let file = options.open(path).ok()?;
    if !is_small_file(file.metadata().ok()?) {
        return None;
    }
    let mut text = String::new();
    file.take(MAX_FILE + 1).read_to_string(&mut text).ok()?;
    (text.len() <= MAX_FILE as usize).then_some(text)
}

pub fn branch(cwd: &Path) -> Option<String> {
    // OSC 7 paths are absolute; do not accidentally search relative to Twitty's own cwd.
    if !cwd.is_absolute() {
        return None;
    }
    let cwd = cwd.canonicalize().ok()?;
    for dir in cwd.ancestors().take(64) {
        let dot_git = dir.join(".git");
        if dot_git.symlink_metadata().is_err() {
            continue;
        }
        let gitdir = if dot_git.metadata().ok()?.is_dir() {
            dot_git
        } else {
            let file = small_file(&dot_git)?;
            let target = file.trim().strip_prefix("gitdir:")?.trim();
            if target.is_empty() {
                return None;
            }
            dir.join(target)
        };
        let head = small_file(&gitdir.join("HEAD"))?;
        let head = head.trim();
        let name = if let Some(name) = head.strip_prefix("ref: refs/heads/") {
            name
        } else if matches!(head.len(), 40 | 64) && head.bytes().all(|b| b.is_ascii_hexdigit()) {
            &head[..7]
        } else {
            return None;
        };
        let name = crate::shell_integration::clean_text(name, 256);
        return (!name.is_empty()).then_some(name);
    }
    None
}

#[derive(Default)]
pub struct BranchCache {
    directories: HashMap<PathBuf, (Instant, Option<String>)>,
}

impl BranchCache {
    pub fn get(&mut self, cwd: &Path, now: Instant) -> Option<String> {
        if let Some((at, value)) = self.directories.get(cwd) {
            if now.saturating_duration_since(*at) < REFRESH {
                return value.clone();
            }
        }
        let value = branch(cwd);
        self.directories
            .insert(cwd.to_path_buf(), (now, value.clone()));
        value
    }

    /// Cheap UI-thread lookup of the last cached branch without touching the filesystem.
    pub fn cached(&self, cwd: &Path) -> Option<String> {
        self.directories
            .get(cwd)
            .and_then(|(_, value)| value.clone())
    }

    /// Apply branch lookups produced off the UI thread. Returns true when any cached branch
    /// value changed.
    pub fn update(&mut self, results: HashMap<PathBuf, Option<String>>, now: Instant) -> bool {
        let mut changed = false;
        for (dir, value) in results {
            let prev = self.directories.get(&dir).and_then(|(_, v)| v.as_deref());
            if prev != value.as_deref() {
                changed = true;
            }
            self.directories.insert(dir, (now, value));
        }
        changed
    }

    /// Keep memory bounded to the directories currently shown, including negative lookups.
    pub fn retain(&mut self, directories: &HashSet<PathBuf>) {
        self.directories
            .retain(|path, _| directories.contains(path));
    }

    pub fn clear(&mut self) {
        self.directories.clear();
    }

    pub fn len(&self) -> usize {
        self.directories.len()
    }

    pub fn is_empty(&self) -> bool {
        self.directories.is_empty()
    }
}

/// Resolves git branch names for watched sidebar directories on a background thread on a
/// periodic TTL, delivering updates only when the resolved map changes. Dropping it stops the
/// thread.
pub struct BranchWatcher {
    tx: mpsc::Sender<Vec<PathBuf>>,
    watching: Vec<PathBuf>,
}

impl BranchWatcher {
    /// Start the background thread. `deliver` returns false once nobody listens, ending it.
    pub fn spawn(
        interval: Duration,
        mut deliver: impl FnMut(HashMap<PathBuf, Option<String>>) -> bool + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        std::thread::Builder::new()
            .name("twitty-git-branch".into())
            .spawn(move || {
                let mut dirs: Vec<PathBuf> = Vec::new();
                let mut last: Option<HashMap<PathBuf, Option<String>>> = None;
                loop {
                    let next = if dirs.is_empty() {
                        rx.recv().map_err(|_| ())
                    } else {
                        match rx.recv_timeout(interval) {
                            Ok(updated) => Ok(updated),
                            Err(mpsc::RecvTimeoutError::Timeout) => Ok(dirs.clone()),
                            Err(mpsc::RecvTimeoutError::Disconnected) => Err(()),
                        }
                    };
                    let Ok(mut next) = next else { return };
                    while let Ok(newer) = rx.try_recv() {
                        next = newer;
                    }
                    dirs = next;
                    if dirs.is_empty() {
                        // Sidebar closed: clear last delivered snapshot so reopening delivers fresh
                        // values even if the branches did not change while hidden.
                        last = None;
                        continue;
                    }
                    let result: HashMap<PathBuf, Option<String>> =
                        dirs.iter().map(|d| (d.clone(), branch(d))).collect();
                    if last.as_ref() != Some(&result) {
                        if !deliver(result.clone()) {
                            return;
                        }
                        last = Some(result);
                    }
                }
            })?;
        Ok(Self {
            tx,
            watching: Vec::new(),
        })
    }

    /// Watch `directories`; an empty list pauses scanning. No-op when the set is unchanged.
    pub fn watch(&mut self, mut directories: Vec<PathBuf>) {
        directories.sort();
        directories.dedup();
        if directories != self.watching {
            self.watching = directories.clone();
            let _ = self.tx.send(directories);
        }
    }

    pub fn watching(&self) -> &[PathBuf] {
        &self.watching
    }
}
