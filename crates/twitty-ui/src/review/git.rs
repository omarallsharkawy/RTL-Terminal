//! Reading a repository's uncommitted changes with git, on a worker thread.
//!
//! git only ever gets fixed arguments plus the repository path (`-C <dir>`): nothing a program
//! prints or a user types reaches its command line. Its output is read up to a bound and the
//! process is killed past it or after a timeout, so a huge repository never stalls the panel.
//! Settings that could run other programs or change the output format (external diff and
//! textconv drivers, fsmonitor, quoted paths, prefixes) are turned off on the command line.

use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::Duration;

use super::diff::{parse, Diff, Limits, Untracked, MAX_DIFF_BYTES};
use crate::process::{read_bounded, run_bounded};

/// Longest a git command may run.
const TIMEOUT: Duration = Duration::from_secs(20);
/// Largest untracked file read; larger ones are one-line entries.
pub const MAX_UNTRACKED_FILE_BYTES: u64 = 512 * 1024;
/// Most bytes of `git ls-files --others` output read.
const MAX_LIST_BYTES: usize = 1024 * 1024;
/// The id of git's empty tree, for diffs in a repository without commits.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// The changes of one repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The repository's top directory.
    pub root: PathBuf,
    /// Its directory name, cleaned for display.
    pub name: String,
    /// The checked-out branch (or short commit), when known.
    pub branch: Option<String>,
    pub diff: Diff,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// The directory is not inside a git work tree.
    NotRepo,
    /// git could not be started.
    NoGit,
    /// git failed; the cleaned reason.
    Failed(String),
}

/// One finished git command.
struct Output {
    stdout: Vec<u8>,
    /// Output went past the bound (git was stopped).
    truncated: bool,
    status: Option<ExitStatus>,
}

impl Output {
    fn success(&self) -> bool {
        self.status.is_some_and(|s| s.success())
    }
}

/// Run `git -C dir <fixed config> args…`, reading at most `limit` bytes of output.
fn git(dir: &Path, args: &[&str], limit: usize) -> Result<Output, LoadError> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.quotepath=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "diff.noprefix=false",
            "-c",
            "diff.mnemonicprefix=false",
            "-c",
            "color.ui=never",
        ])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    let out = run_bounded(
        cmd,
        "twitty-review-git",
        TIMEOUT,
        Duration::from_millis(15),
        limit,
    )
    .map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => LoadError::NoGit,
        _ => LoadError::Failed(format!("git: {e}")),
    })?;
    Ok(Output {
        stdout: out.stdout,
        truncated: out.truncated,
        status: out.status,
    })
}

fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Load the uncommitted changes (against `HEAD`, staged or not) and the untracked files of the
/// repository containing `cwd`. Blocking: call it on a worker thread.
pub fn load(cwd: &Path) -> Result<Snapshot, LoadError> {
    let top = git(cwd, &["rev-parse", "--show-toplevel"], 64 * 1024)?;
    if !top.success() || top.truncated {
        return Err(LoadError::NotRepo);
    }
    let line = top.stdout.split(|&b| b == b'\n').next().unwrap_or(&[]);
    if line.is_empty() {
        return Err(LoadError::NotRepo);
    }
    let root = path_from_bytes(line);
    let has_head = git(&root, &["rev-parse", "--verify", "--quiet", "HEAD"], 1024)?.success();
    let base = if has_head { "HEAD" } else { EMPTY_TREE };
    let out = git(
        &root,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-U3",
            "-M",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            base,
            "--",
        ],
        MAX_DIFF_BYTES,
    )?;
    if !out.truncated && !out.success() {
        let code = out.status.and_then(|s| s.code());
        return Err(LoadError::Failed(match code {
            Some(code) => format!("git diff exited with {code}"),
            None => "git diff was stopped".into(),
        }));
    }
    let limits = Limits::default();
    let mut diff = parse(&out.stdout, out.truncated, &limits);
    if !out.truncated {
        let budget = MAX_DIFF_BYTES - out.stdout.len();
        add_untracked(&root, &mut diff, budget, &limits)?;
    }
    let name = root
        .file_name()
        .map(|n| crate::shell_integration::clean_text(&n.to_string_lossy(), 256))
        .unwrap_or_default();
    let branch = crate::sidebar::git::branch(&root);
    Ok(Snapshot {
        root,
        name,
        branch,
        diff,
    })
}

/// Add the untracked (not ignored) files as added files, reading at most `budget` bytes.
fn add_untracked(
    root: &Path,
    diff: &mut Diff,
    mut budget: usize,
    limits: &Limits,
) -> Result<(), LoadError> {
    let list = git(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z", "--"],
        MAX_LIST_BYTES,
    )?;
    if !list.success() && !list.truncated {
        return Ok(());
    }
    let mut names: Vec<&[u8]> = list.stdout.split(|&b| b == 0).collect();
    // The last name of a cut list may be partial.
    if list.truncated {
        names.pop();
    }
    for name in names.into_iter().filter(|n| !n.is_empty()) {
        let rel = path_from_bytes(name);
        if !is_plain_relative(&rel) {
            continue;
        }
        let shown = String::from_utf8_lossy(name);
        if diff.files.len() >= limits.max_files {
            diff.omitted_files += 1;
            continue;
        }
        let content = read_untracked(&root.join(&rel), budget as u64);
        if let Untracked::Text(bytes) = &content {
            budget -= bytes.len().min(budget);
        }
        diff.push_untracked(&shown, content, limits);
    }
    if list.truncated {
        diff.truncated = true;
    }
    Ok(())
}

/// Only `a/b/c` style paths: never absolute, never `..`.
fn is_plain_relative(path: &Path) -> bool {
    path.components().all(|c| matches!(c, Component::Normal(_)))
}

/// Read an untracked file: never following a symlink or opening a FIFO, at most `budget` (and
/// `MAX_UNTRACKED_FILE_BYTES`) bytes.
pub fn read_untracked(path: &Path, budget: u64) -> Untracked {
    let Ok(meta) = path.symlink_metadata() else {
        return Untracked::Unreadable;
    };
    if !meta.is_file() {
        return Untracked::Special;
    }
    let limit = budget.min(MAX_UNTRACKED_FILE_BYTES);
    if meta.len() > limit {
        return Untracked::TooBig(meta.len());
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let Ok(file) = options.open(path) else {
        return Untracked::Unreadable;
    };
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Untracked::Special;
    }
    let (bytes, more) = read_bounded(file, limit as usize);
    if more {
        return Untracked::TooBig(meta.len().max(limit + 1));
    }
    Untracked::Text(bytes)
}
