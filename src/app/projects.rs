//! Project switcher integration: MRU persistence, off-thread git repository scanning, and
//! opening a project in a tab (switching if already open) or a split.

use std::path::{Path, PathBuf};

use super::{App, AppEvent};
use crate::layout::SplitDir;
use crate::projects::mru::ProjectMru;
use crate::projects::scan::{resolve_existing_roots, scan_roots};
use crate::projects::{
    find_existing_tab_for_project, home_dir, merge_projects, ProjectItem, TabProjectCwds,
};
use crate::pty::SpawnOptions;

pub(super) struct ProjectState {
    mru: ProjectMru,
    mru_dirty: bool,
    mru_save_seq: std::sync::Arc<std::sync::atomic::AtomicU64>,
    mru_save_lock: std::sync::Arc<std::sync::Mutex<()>>,
    generation: u64,
    scanning: bool,
    items: Vec<ProjectItem>,
}

impl ProjectState {
    pub(super) fn load() -> Self {
        Self {
            mru: ProjectMru::load_default(),
            mru_dirty: false,
            mru_save_seq: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            mru_save_lock: std::sync::Arc::new(std::sync::Mutex::new(())),
            generation: 0,
            scanning: false,
            items: Vec::new(),
        }
    }

    pub(super) fn items(&self) -> &[ProjectItem] {
        &self.items
    }

    pub(super) fn is_scanning(&self) -> bool {
        self.scanning
    }
}

impl App {
    /// Record a local pane working directory in the in-memory MRU list.
    pub(super) fn record_project_cwd(&mut self, path: &Path) {
        if self.projects.mru.record(path) {
            self.projects.mru_dirty = true;
        }
    }

    /// Collect all current local pane working directories into the MRU list.
    fn collect_live_cwds_into_mru(&mut self) {
        let mut cwds: Vec<PathBuf> = Vec::new();
        for tab in self.ws.tabs() {
            for id in tab.root.leaves() {
                if let Some(cwd) = self.pane_cwd(id) {
                    cwds.push(cwd);
                }
            }
        }
        // Record focused pane last so it stays at the top of the MRU.
        let focused = self.focused_id().and_then(|id| self.pane_cwd(id));
        for cwd in cwds {
            self.record_project_cwd(&cwd);
        }
        if let Some(cwd) = focused {
            self.record_project_cwd(&cwd);
        }
    }

    /// Start an off-thread scan of `project_roots` and branch resolution for MRU entries.
    pub(super) fn start_project_scan(&mut self) {
        self.collect_live_cwds_into_mru();
        let home = home_dir();
        let mru_entries = self.projects.mru.entries().to_vec();
        if self.projects.items.is_empty() {
            self.projects.items = merge_projects(&mru_entries, Vec::new(), home.as_deref(), false);
        }
        self.projects.generation = self.projects.generation.wrapping_add(1);
        let generation = self.projects.generation;
        self.projects.scanning = true;

        let roots_cfg = self.config.project_roots.clone();
        let proxy = self.proxy.clone();
        let spawn = std::thread::Builder::new()
            .name("twitty-projects".into())
            .spawn(move || {
                let roots = resolve_existing_roots(&roots_cfg, home.as_deref());
                let scanned = scan_roots(&roots);
                let items = merge_projects(&mru_entries, scanned, home.as_deref(), true);
                let _ = proxy.send_event(AppEvent::Projects(generation, items));
            });
        if let Err(err) = spawn {
            tracing::warn!("project scanner thread failed to start: {err}");
            self.projects.scanning = false;
        }
    }

    /// Apply off-thread project scan results if they match the latest request.
    pub(super) fn on_projects_loaded(&mut self, generation: u64, items: Vec<ProjectItem>) {
        if generation != self.projects.generation {
            return;
        }
        self.projects.items = items;
        self.projects.scanning = false;
        if self.projects_palette_open() {
            self.sync_palette();
        }
    }

    /// Open `path` from the project switcher: `Enter` switches to an existing tab in that
    /// directory or opens a new tab; `Alt+Enter` (`split == true`) opens a split pane there.
    pub(super) fn open_project(&mut self, path: PathBuf, split: bool) {
        self.record_project_cwd(&path);
        self.save_mru_async();

        if split {
            let opts = SpawnOptions {
                cwd: Some(path),
                ..SpawnOptions::default()
            };
            let focused = self.focused_id();
            let res = if self.can_split_focused(SplitDir::Horizontal) {
                self.split_focused_with(SplitDir::Horizontal, opts)
            } else if self.can_split_focused(SplitDir::Vertical) {
                self.split_focused_with(SplitDir::Vertical, opts)
            } else {
                self.split_focused_with(SplitDir::Horizontal, opts)
            };
            if let Err(err) = res {
                if let Some(id) = focused {
                    self.show_pane_warning(id, &err.message);
                }
            }
            return;
        }

        let tabs: Vec<TabProjectCwds> = self
            .ws
            .tabs()
            .iter()
            .map(|tab| {
                let panes = tab
                    .root
                    .leaves()
                    .into_iter()
                    .map(|id| (id, self.pane_cwd(id)))
                    .collect();
                TabProjectCwds {
                    focused: tab.focused,
                    panes,
                }
            })
            .collect();
        if let Some((_, pane_id)) = find_existing_tab_for_project(&tabs, &path) {
            let _ = self.focus_pane_by_id(pane_id);
            return;
        }
        let _ = self.new_tab_with(SpawnOptions {
            cwd: Some(path),
            ..SpawnOptions::default()
        });
    }

    fn save_mru_async(&mut self) {
        if !self.projects.mru_dirty {
            return;
        }
        self.projects.mru_dirty = false;
        let snapshot = self.projects.mru.clone();
        let seq = self
            .projects
            .mru_save_seq
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            + 1;
        let seq_ref = std::sync::Arc::clone(&self.projects.mru_save_seq);
        let lock = std::sync::Arc::clone(&self.projects.mru_save_lock);
        if let Err(err) = std::thread::Builder::new()
            .name("twitty-mru-save".into())
            .spawn(move || {
                let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                if seq_ref.load(std::sync::atomic::Ordering::Acquire) != seq {
                    return;
                }
                if let Err(err) = snapshot.save_default() {
                    tracing::warn!("twitty: could not save recent projects: {err}");
                }
            })
        {
            tracing::warn!("twitty: mru save thread failed to start: {err}");
            self.projects.mru_dirty = true;
        }
    }

    /// Persist recent pane working directories when the window exits.
    pub(super) fn finish_projects(&mut self) {
        self.collect_live_cwds_into_mru();
        if self.projects.mru_dirty {
            self.projects.mru_dirty = false;
            self.projects
                .mru_save_seq
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let _guard = self
                .projects
                .mru_save_lock
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Err(err) = self.projects.mru.save_default() {
                crate::config::warn(format!("twitty: could not save recent projects ({err})"));
            }
        }
    }
}
