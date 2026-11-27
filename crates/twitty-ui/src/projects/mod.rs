//! Project switcher: bounded recent-directory MRU and off-thread git repository discovery.
//!
//! Window-free logic; `app::projects` drives background scanning and opening tabs or splits.

pub mod mru;
pub mod scan;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::layout::PaneId;
use crate::shell_integration::clean_text;
use crate::sidebar::git::branch as git_branch;
use crate::ui::palette::PaletteItem;
use crate::ui::palette::Searchable;
use twitty_core::language::UiLanguage;

use self::scan::ScannedRepo;

/// Palette placeholder for the project switcher (`Ctrl+Shift+L`).
/// Palette placeholder while picking a project.
pub fn projects_placeholder() -> &'static str {
    crate::ui::chrome::t("Open project… (Enter: tab, Alt+Enter: split)")
}

/// Maximum characters kept in a project's display name or path.
const MAX_DISPLAY_CHARS: usize = 200;

/// One project directory listed in the project switcher palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectItem {
    pub path: PathBuf,
    pub name: String,
    pub display_path: String,
    pub branch: Option<String>,
    pub recent: bool,
}

impl ProjectItem {
    /// Build a displayable project item for `path`, abbreviating `home` as `~`.
    pub fn new(path: PathBuf, home: Option<&Path>, branch: Option<String>, recent: bool) -> Self {
        let display_path = clean_text(&abbreviate_home(&path, home), MAX_DISPLAY_CHARS);
        let raw_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&display_path);
        let name = clean_text(raw_name, MAX_DISPLAY_CHARS);
        let branch = branch
            .map(|b| clean_text(&b, 128))
            .filter(|b| !b.is_empty());
        Self {
            path,
            name,
            display_path,
            branch,
            recent,
        }
    }

    /// Format this project as a command-palette row (English/path on the left, branch/recency on
    /// the right).
    pub fn to_palette_item(&self) -> PaletteItem {
        let title_en = if self.name == self.display_path || self.display_path.is_empty() {
            self.name.clone()
        } else {
            format!("{}  {}", self.name, self.display_path)
        };
        let title_ar = match (&self.branch, self.recent) {
            (Some(branch), true) => format!("{} · {branch}", UiLanguage::Arabic.t("Recent")),
            (Some(branch), false) => branch.clone(),
            (None, true) => UiLanguage::Arabic.t("Recent").into(),
            (None, false) => String::new(),
        };
        PaletteItem {
            title_en,
            title_ar,
            shortcut: None,
            dimmed: false,
        }
    }
}

impl Searchable for ProjectItem {
    fn search_fields(&self) -> Vec<&str> {
        let mut fields = vec![self.name.as_str(), self.display_path.as_str()];
        if let Some(branch) = self.branch.as_deref() {
            fields.push(branch);
        }
        fields
    }
}

/// Replace a leading `home` prefix with `~` for compact display.
pub fn abbreviate_home(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home.filter(|h| h.is_absolute()) {
        if path == home {
            return "~".into();
        }
        if let Ok(rest) = path.strip_prefix(home) {
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

/// Return the current user's home directory, if set to an absolute path.
pub fn home_dir() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// Canonicalization key for deduplicating project paths without failing on missing dirs.
pub fn path_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Merge MRU directories (most recent first) with discovered git repositories, deduplicating by
/// canonical path. When `resolve_mru` is true (on the background worker thread), non-existent MRU
/// directories are filtered out and git branches are read for remaining MRU entries.
pub fn merge_projects(
    mru: &[PathBuf],
    scanned: Vec<ScannedRepo>,
    home: Option<&Path>,
    resolve_mru: bool,
) -> Vec<ProjectItem> {
    let mut scanned_branches: std::collections::HashMap<PathBuf, Option<String>> =
        std::collections::HashMap::with_capacity(scanned.len());
    for repo in &scanned {
        scanned_branches.insert(path_key(&repo.path), repo.branch.clone());
    }

    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out = Vec::new();

    for path in mru {
        if !path.is_absolute() {
            continue;
        }
        if resolve_mru && !path.is_dir() {
            continue;
        }
        let key = path_key(path);
        if !seen.insert(key.clone()) {
            continue;
        }
        let branch = if let Some(found) = scanned_branches.get(&key) {
            found.clone()
        } else if resolve_mru {
            git_branch(path)
        } else {
            None
        };
        out.push(ProjectItem::new(path.clone(), home, branch, true));
    }

    let mut extra: Vec<ProjectItem> = scanned
        .into_iter()
        .filter_map(|repo| {
            let key = path_key(&repo.path);
            if !seen.insert(key) {
                return None;
            }
            Some(ProjectItem::new(repo.path, home, repo.branch, false))
        })
        .collect();

    extra.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
    out.extend(extra);
    out
}

/// Snapshot of one open tab's focused pane and each pane's local working directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabProjectCwds {
    pub focused: PaneId,
    pub panes: Vec<(PaneId, Option<PathBuf>)>,
}

/// Find an existing tab that already has a local pane in `target`.
///
/// Focused panes are checked first so switching lands on the primary pane of a matching tab.
pub fn find_existing_tab_for_project(
    tabs: &[TabProjectCwds],
    target: &Path,
) -> Option<(usize, PaneId)> {
    let target_key = path_key(target);
    // First pass: focused pane of each tab.
    for (tab_index, tab) in tabs.iter().enumerate() {
        if let Some((_, Some(cwd))) = tab.panes.iter().find(|(id, _)| *id == tab.focused) {
            if path_key(cwd) == target_key {
                return Some((tab_index, tab.focused));
            }
        }
    }
    // Second pass: any pane of any tab.
    for (tab_index, tab) in tabs.iter().enumerate() {
        for (id, cwd) in &tab.panes {
            if let Some(cwd) = cwd {
                if path_key(cwd) == target_key {
                    return Some((tab_index, *id));
                }
            }
        }
    }
    None
}
