//! Window-free command history recording from finished shell-integration blocks.
//!
//! When a command block finishes, [`record_finished_block`] validates `config.record_history` and
//! `HISTCONTROL=ignorespace` (leading space), extracts the command, exit code, duration, and cwd,
//! and hands them to [`twitty_store::StoreHandle`] so Git root resolution, redaction, and SQLite
//! writes all run off the UI thread.

use std::path::Path;

use twitty_store::{now_epoch_ms, StoreHandle};

use crate::blocks::Block;

/// Decide whether `command` should be persisted to local history.
///
/// Honors `config.record_history` and skips blank commands as well as commands beginning with a
/// space (matching `HISTCONTROL=ignorespace`).
pub fn should_record_command(record_history: bool, command: &str) -> bool {
    if !record_history {
        return false;
    }
    if command.starts_with(' ') {
        return false;
    }
    !command.trim().is_empty()
}

/// Best-effort basename of the user's configured login shell (e.g. `"zsh"`, `"bash"`, `"fish"`).
pub fn detect_shell_name() -> Option<String> {
    let raw = if cfg!(windows) {
        std::env::var_os("COMSPEC")
    } else {
        std::env::var_os("SHELL")
    }?;
    let name = Path::new(&raw).file_name()?.to_string_lossy();
    let cleaned = crate::shell_integration::clean_text(&name, 64);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Record a finished command [`Block`] in `store` off the UI thread.
///
/// Returns `true` if the command passed the recording policy and was enqueued; `false` if skipped.
pub fn record_finished_block(
    store: &StoreHandle,
    record_history: bool,
    block: &Block,
    cwd: Option<&Path>,
    is_local: bool,
    shell: Option<&str>,
) -> bool {
    if block.is_running() {
        return false;
    }
    let Some(command) = block.command.as_deref() else {
        return false;
    };
    if !should_record_command(record_history, command) {
        return false;
    }
    let duration_ms = block
        .duration()
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64);
    let finished_at = now_epoch_ms();
    let started_at = finished_at.saturating_sub(duration_ms.unwrap_or(0).max(0));
    store.record_finished_block_async(
        command.trim_end().to_owned(),
        block.exit,
        duration_ms,
        cwd.map(Path::to_path_buf).unwrap_or_default(),
        is_local,
        shell.map(str::to_owned),
        started_at,
        Some(finished_at),
    );
    true
}
