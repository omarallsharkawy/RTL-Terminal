//! Bounded, process-free git HEAD discovery. Failed lookups are cached too.

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(2);
const MAX_FILE: u64 = 4096;

fn small_file(path: &Path) -> Option<String> {
    let is_small_file = |m: fs::Metadata| m.is_file() && m.len() <= MAX_FILE;
    if !is_small_file(path.metadata().ok()?) {
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A repository can replace a regular file with a FIFO between stat and open.
        options.custom_flags(libc::O_NONBLOCK);
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

    /// Keep memory bounded to the directories currently shown, including negative lookups.
    pub fn retain(&mut self, directories: &HashSet<PathBuf>) {
        self.directories
            .retain(|path, _| directories.contains(path));
    }
}
