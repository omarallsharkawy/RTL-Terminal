//! Bounded Git line counts for the focused pane. Process I/O belongs to a worker thread.

use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const MAX_OUTPUT: usize = 1024 * 1024;
pub const MAX_FILES: usize = 4096;
pub const MAX_LINES: u32 = 999_999;
const TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub added: u32,
    pub removed: u32,
}

impl DiffStat {
    /// Cap the visible count, with a trailing plus when the exact total exceeds that cap.
    pub fn label(self) -> String {
        let count = |n| {
            if n > MAX_LINES {
                format!("{MAX_LINES}+")
            } else {
                format!("{n}")
            }
        };
        format!("+{} −{}", count(self.added), count(self.removed))
    }
}

/// Parse `git diff --numstat HEAD`, with newline or `-z` delimiters. With `-z`, renames
/// have an empty path followed by two NUL-terminated names, which may contain any byte
/// except NUL. Binary files contribute no lines. Reject incomplete or oversized snapshots.
pub fn parse_numstat(output: &[u8]) -> Option<DiffStat> {
    if output.len() > MAX_OUTPUT {
        return None;
    }
    let nul = output.contains(&0);
    let delimiter = if nul { 0 } else { b'\n' };
    let mut entries = output.split(|&b| b == delimiter).peekable();
    let mut stats = DiffStat::default();
    let mut files = 0;
    while let Some(entry) = entries.next() {
        if entry.is_empty() && entries.peek().is_none() {
            break;
        }
        files += 1;
        if files > MAX_FILES {
            return None;
        }
        let mut columns = entry.splitn(3, |&b| b == b'\t');
        let (added, removed, name) = (columns.next()?, columns.next()?, columns.next()?);
        if name.is_empty() && (!nul || entries.next()?.is_empty() || entries.next()?.is_empty()) {
            return None;
        }
        if nul && entries.peek().is_none() {
            return None;
        }
        if (added, removed) != (b"-", b"-") {
            stats.added = (stats.added + parse_count(added)?).min(MAX_LINES + 1);
            stats.removed = (stats.removed + parse_count(removed)?).min(MAX_LINES + 1);
        }
    }
    Some(stats)
}

fn parse_count(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(bytes.iter().fold(0, |count, digit| {
        (count * 10 + u32::from(digit - b'0')).min(MAX_LINES + 1)
    }))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkingDirectory {
    Reported(PathBuf),
    Process(u32),
}

impl WorkingDirectory {
    fn resolve(self) -> Option<PathBuf> {
        match self {
            Self::Reported(path) => Some(path),
            Self::Process(pid) if cfg!(target_os = "linux") => {
                std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
            }
            Self::Process(_) => None,
        }
    }
}

/// For a local pane only. Never relies on Git's pager, external diff, text conversion or
/// fsmonitor. Returns `None` outside a worktree and on Git failure or budget exhaustion.
pub fn scan(directory: WorkingDirectory) -> Option<DiffStat> {
    let cwd = directory.resolve()?;
    let mut git = Command::new("git");
    crate::process::apply_no_window(&mut git);
    let child = git
        .args([
            "--no-pager",
            "-c",
            "core.fsmonitor=false",
            "diff",
            "--numstat",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            "HEAD",
            "--",
        ])
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut guard = crate::process::KillOnDrop::new(child);
    let deadline = Instant::now() + TIMEOUT;
    let output = read_output(guard.0.as_mut()?, deadline)?;
    loop {
        if let Some(status) = guard.0.as_mut()?.try_wait().ok()? {
            let _ = guard.disarm();
            return status.success().then(|| parse_numstat(&output)).flatten();
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(target_os = "linux")]
fn read_output(child: &mut Child, deadline: Instant) -> Option<Vec<u8>> {
    use rustix::fs::{fcntl_setfl, OFlags};
    let mut stdout = child.stdout.take()?;
    fcntl_setfl(&stdout, OFlags::NONBLOCK).ok()?;
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        match stdout.read(&mut buffer) {
            Ok(0) => return Some(output),
            Ok(n) if output.len() + n <= MAX_OUTPUT => output.extend_from_slice(&buffer[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(POLL_INTERVAL),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            _ => return None,
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn read_output(child: &mut Child, deadline: Instant) -> Option<Vec<u8>> {
    let stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("twitty git output".into())
        .spawn(move || {
            let mut output = Vec::new();
            let result: io::Result<_> = stdout
                .take((MAX_OUTPUT + 1) as u64)
                .read_to_end(&mut output);
            let _ = tx.send(result.ok().filter(|&len| len <= MAX_OUTPUT).map(|_| output));
        })
        .ok()?;
    rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()?
}
