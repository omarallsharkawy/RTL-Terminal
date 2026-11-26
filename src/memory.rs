//! App-facing project memory helpers: resolving the active project root and persisting
//! user-approved facts from the agent panel (`AgentEvent::RememberFact`) off the UI thread.

use std::path::{Path, PathBuf};

use twitty_store::{is_path_excluded, resolve_project_root, MemoryKind, NewMemory, StoreHandle};

/// Maximum characters kept in a remembered fact.
const MAX_FACT_CHARS: usize = 2048;

/// Resolve the project identity for a working directory (git root if present, else `cwd`).
pub fn resolve_active_project_root(cwd: Option<&Path>) -> Option<PathBuf> {
    cwd.filter(|p| !p.as_os_str().is_empty())
        .map(resolve_project_root)
}

/// Persist a user-approved fact from the agent panel (`AgentEvent::RememberFact`) into project
/// memory off the UI thread.
///
/// Because the user already clicked Approve on the agent's `FactSuggestion` card, the fact is
/// stored with `approved = true`. Secret redaction runs inside `twitty-store` before writing.
/// Returns `true` if the write was enqueued, or `false` if `project_root` is `None` or `text` is
/// empty after trimming.
pub fn remember_fact(store: &StoreHandle, project_root: Option<&Path>, text: &str) -> bool {
    remember_fact_with_excludes(store, project_root, text, &[])
}

/// Like [`remember_fact`], also honoring `memory_exclude` patterns so nothing is recorded for
/// excluded projects.
pub fn remember_fact_with_excludes(
    store: &StoreHandle,
    project_root: Option<&Path>,
    text: &str,
    memory_exclude: &[String],
) -> bool {
    let Some(root) = project_root.filter(|p| !p.as_os_str().is_empty()) else {
        return false;
    };
    if !memory_exclude.is_empty() && is_path_excluded(root, memory_exclude) {
        return false;
    }
    let cleaned = crate::shell_integration::clean_text(text.trim(), MAX_FACT_CHARS);
    if cleaned.is_empty() {
        return false;
    }
    let entry = NewMemory {
        project_root: root.to_string_lossy().into_owned(),
        kind: MemoryKind::Fact,
        text: cleaned,
        source: "agent".to_owned(),
        approved: true,
    };
    store.create_memory_if_not_excluded_async(entry, memory_exclude.to_vec());
    true
}
