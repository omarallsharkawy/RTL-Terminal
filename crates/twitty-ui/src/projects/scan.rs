//! Bounded off-thread discovery of git repositories one or two levels under configured project
//! roots, with symlink-loop protection.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::sidebar::git::branch as git_branch;

/// Default directories scanned for git repositories when `project_roots` is unset in `config.json`.
pub const DEFAULT_PROJECT_ROOTS: &[&str] = &["~/Projects", "~/src", "~/code"];

/// Default limits for repository scanning.
pub const MAX_ROOTS: usize = 16;
pub const MAX_ENTRIES_PER_DIR: usize = 256;
pub const MAX_DIRS_VISITED: usize = 1024;
pub const MAX_REPOS: usize = 200;
pub const SCAN_BUDGET: Duration = Duration::from_secs(2);

/// Bounds enforced during a project-root scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanLimits {
    pub max_roots: usize,
    pub max_entries_per_dir: usize,
    pub max_dirs_visited: usize,
    pub max_repos: usize,
    pub budget: Duration,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_roots: MAX_ROOTS,
            max_entries_per_dir: MAX_ENTRIES_PER_DIR,
            max_dirs_visited: MAX_DIRS_VISITED,
            max_repos: MAX_REPOS,
            budget: SCAN_BUDGET,
        }
    }
}

/// One discovered git repository and its current branch (when readable from `.git/HEAD`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedRepo {
    pub path: PathBuf,
    pub branch: Option<String>,
}

/// Default `project_roots` config value (`["~/Projects", "~/src", "~/code"]`).
pub fn default_project_roots() -> Vec<String> {
    DEFAULT_PROJECT_ROOTS
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}

/// Expand a configured project root string (`~` / `~/...`) against `home`.
/// Returns `None` for empty or non-absolute paths.
pub fn expand_root(raw: &str, home: Option<&Path>) -> Option<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = if trimmed == "~" {
        home.filter(|h| h.is_absolute())?.to_path_buf()
    } else if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        home.filter(|h| h.is_absolute())?.join(rest)
    } else {
        PathBuf::from(trimmed)
    };
    path.is_absolute().then_some(path)
}

/// Expand configured root strings and keep only existing directories (bounded to `MAX_ROOTS`).
pub fn resolve_existing_roots(roots: &[String], home: Option<&Path>) -> Vec<PathBuf> {
    roots
        .iter()
        .filter_map(|raw| expand_root(raw, home))
        .filter(|path| path.is_dir())
        .take(MAX_ROOTS)
        .collect()
}

/// Scan `roots` (1 or 2 levels deep) for git repositories using default limits.
pub fn scan_roots(roots: &[PathBuf]) -> Vec<ScannedRepo> {
    scan_roots_with_limits(roots, ScanLimits::default())
}

/// Scan `roots` (1 or 2 levels deep) for git repositories under `limits`, breaking symlink loops
/// via canonical-directory tracking.
pub fn scan_roots_with_limits(roots: &[PathBuf], limits: ScanLimits) -> Vec<ScannedRepo> {
    let mut scanner = Scanner::new(limits);
    for root in roots.iter().take(limits.max_roots) {
        if scanner.exhausted() {
            break;
        }
        scanner.scan_root(root);
    }
    scanner.repos.sort_by(|a, b| a.path.cmp(&b.path));
    scanner.repos
}

struct Scanner {
    limits: ScanLimits,
    start: Instant,
    visited: HashSet<PathBuf>,
    dirs_visited: usize,
    repos: Vec<ScannedRepo>,
}

impl Scanner {
    fn new(limits: ScanLimits) -> Self {
        Self {
            limits,
            start: Instant::now(),
            visited: HashSet::new(),
            dirs_visited: 0,
            repos: Vec::new(),
        }
    }

    fn exhausted(&self) -> bool {
        self.repos.len() >= self.limits.max_repos
            || self.dirs_visited >= self.limits.max_dirs_visited
            || self.start.elapsed() >= self.limits.budget
    }

    fn scan_root(&mut self, root: &Path) {
        if !root.is_absolute() {
            return;
        }
        let Ok(canon_root) = fs::canonicalize(root) else {
            return;
        };
        if !canon_root.is_dir() || !self.visited.insert(canon_root) {
            return;
        }
        self.scan_dir(root, 1);
    }

    fn scan_dir(&mut self, dir: &Path, depth: u8) {
        let Ok(read_dir) = fs::read_dir(dir) else {
            return;
        };
        let mut children: Vec<PathBuf> = Vec::new();
        for entry in read_dir.take(self.limits.max_entries_per_dir) {
            let Ok(entry) = entry else { continue };
            let name = entry.file_name();
            if name.to_str().is_some_and(|s| s.starts_with('.')) {
                continue;
            }
            children.push(entry.path());
        }
        children.sort();

        for child in children {
            if self.exhausted() {
                break;
            }
            let Ok(canon) = fs::canonicalize(&child) else {
                continue;
            };
            if !canon.is_dir() {
                continue;
            }
            // Canonical path tracking prevents symlink loops (self, ancestor, or mutual cycles)
            // and duplicate discovery across overlapping roots.
            if !self.visited.insert(canon.clone()) {
                continue;
            }
            self.dirs_visited += 1;

            if is_git_repo(&canon) {
                let branch = git_branch(&canon);
                self.repos.push(ScannedRepo {
                    path: child,
                    branch,
                });
            } else if depth < 2 {
                self.scan_dir(&child, depth + 1);
            }
        }
    }
}

fn is_git_repo(dir: &Path) -> bool {
    dir.join(".git").symlink_metadata().is_ok()
}
