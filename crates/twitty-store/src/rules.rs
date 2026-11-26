//! Project rules files (`AGENTS.md`, `CLAUDE.md`, `TWITTY.md`, `.twitty/rules.md`) and
//! SHA-256-backed per-project approval inspection.
//!
//! A rules file is NEVER read into AI context until the user explicitly approves it for that
//! project. Approvals store the file's SHA-256 digest; if the file changes on disk, approval
//! silently lapses ([`RulesFileState::Changed`]) and the file's content is withheld until the user
//! re-approves it.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::Path;

use twitty_core::redact::redact;

use crate::sha256::sha256_hex;

/// Canonical candidate rules files checked under a project root, in deterministic priority order.
pub const RULES_FILES: [&str; 4] = ["AGENTS.md", "CLAUDE.md", "TWITTY.md", ".twitty/rules.md"];

/// Maximum bytes read from a single rules file (64 KiB).
pub const MAX_RULES_FILE_BYTES: usize = 64 * 1024;

/// A stored per-project rules file approval row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RulesApproval {
    pub project_root: String,
    pub rel_path: String,
    pub sha256: String,
    pub approved_at: i64,
}

/// Approval state of an existing rules file on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulesFileState {
    /// File exists on disk and has never been approved for this project.
    Unapproved,
    /// File exists on disk and its SHA-256 matches the user's stored approval.
    Approved,
    /// File was previously approved, but its contents changed on disk (approval silently lapsed).
    Changed,
}

/// Inspected state of one rules file present in a project root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RulesFileInfo {
    /// Relative path inside the project root (one of [`RULES_FILES`]).
    pub rel_path: String,
    /// SHA-256 hex digest of the file's current bytes on disk.
    pub current_sha256: String,
    /// Previously approved SHA-256 digest, if any.
    pub approved_sha256: Option<String>,
    /// Current approval state.
    pub state: RulesFileState,
    /// Redacted file contents, populated ONLY when `state == RulesFileState::Approved`.
    pub approved_content: Option<String>,
}

/// Inspect all existing rules files under `project_root` against `approvals`.
///
/// Files that escape `project_root` via symlinks, are non-regular files, exceed
/// [`MAX_RULES_FILE_BYTES`], or cannot be read are skipped safely.
pub fn inspect_rules_files_with_approvals(
    project_root: &Path,
    approvals: &[RulesApproval],
) -> Vec<RulesFileInfo> {
    let Ok(canon_root) = fs::canonicalize(project_root) else {
        return Vec::new();
    };
    let approval_map: HashMap<&str, &RulesApproval> =
        approvals.iter().map(|a| (a.rel_path.as_str(), a)).collect();

    let mut out = Vec::new();
    for &rel_path in &RULES_FILES {
        let Some(bytes) = read_bounded_project_file(&canon_root, rel_path, MAX_RULES_FILE_BYTES)
        else {
            continue;
        };
        let current_sha256 = sha256_hex(&bytes);
        let stored = approval_map.get(rel_path).copied();
        let (state, approved_sha256, approved_content) = match stored {
            Some(app) if app.sha256 == current_sha256 => {
                let text = String::from_utf8_lossy(&bytes);
                let redacted = redact(&text).into_owned();
                (
                    RulesFileState::Approved,
                    Some(app.sha256.clone()),
                    Some(redacted),
                )
            }
            Some(app) => (RulesFileState::Changed, Some(app.sha256.clone()), None),
            None => (RulesFileState::Unapproved, None, None),
        };
        out.push(RulesFileInfo {
            rel_path: rel_path.to_owned(),
            current_sha256,
            approved_sha256,
            state,
            approved_content,
        });
    }
    out
}

/// Read a regular file at `canon_root.join(rel_path)` safely:
/// - resolves symlinks and rejects anything whose canonical path leaves `canon_root`,
/// - rejects directories/FIFOs/devices,
/// - caps reads at `max_bytes`.
pub(crate) fn read_bounded_project_file(
    canon_root: &Path,
    rel_path: &str,
    max_bytes: usize,
) -> Option<Vec<u8>> {
    let candidate = canon_root.join(rel_path);
    let canon_file = fs::canonicalize(&candidate).ok()?;
    if !canon_file.starts_with(canon_root) {
        return None;
    }
    let meta = fs::metadata(&canon_file).ok()?;
    if !meta.file_type().is_file() || meta.len() > max_bytes as u64 {
        return None;
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    let file = opts.open(&canon_file).ok()?;
    let fd_meta = file.metadata().ok()?;
    if !fd_meta.file_type().is_file() || fd_meta.len() > max_bytes as u64 {
        return None;
    }
    let mut bytes = Vec::with_capacity(fd_meta.len().min(max_bytes as u64) as usize);
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > max_bytes {
        return None;
    }
    Some(bytes)
}
