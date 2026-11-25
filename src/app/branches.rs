//! The git branch watcher behind the sidebar: started the first time the sidebar is drawn,
//! told which directories to watch on every frame (a no-op while nothing changed), and paused
//! while the sidebar is hidden. Results come back as `AppEvent::Branches`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use super::{App, AppEvent};
use crate::sidebar::git::{BranchWatcher, REFRESH};

#[derive(Default)]
pub(super) enum BranchWatch {
    /// Not started yet.
    #[default]
    Idle,
    Running(BranchWatcher),
    /// The thread could not be started.
    Unavailable,
}

impl App {
    /// Watch the active tab directories while the sidebar is shown; pause otherwise.
    pub(super) fn sync_branch_watch(&mut self) {
        let shown = self.window.sidebar_shown(&self.ws);
        if !shown && !matches!(self.branches, BranchWatch::Running(_)) {
            return;
        }
        if matches!(self.branches, BranchWatch::Idle) {
            self.branches = self.start_branch_watch();
        }
        let BranchWatch::Running(watcher) = &mut self.branches else {
            return;
        };
        let dirs = if shown {
            self.window.sidebar_branch_dirs(&self.ws)
        } else {
            Vec::new()
        };
        watcher.watch(dirs);
    }

    fn start_branch_watch(&self) -> BranchWatch {
        let proxy = self.proxy.clone();
        match BranchWatcher::spawn(REFRESH, move |branches| {
            proxy.send_event(AppEvent::Branches(branches)).is_ok()
        }) {
            Ok(watcher) => BranchWatch::Running(watcher),
            Err(e) => {
                tracing::warn!("branch watcher not started: {e}");
                BranchWatch::Unavailable
            }
        }
    }

    /// A background branch scan finished; redraw when the sidebar changes.
    pub(super) fn on_branches(&mut self, branches: HashMap<PathBuf, Option<String>>) {
        if self.window.set_branches(branches, Instant::now()) {
            self.window.request_redraw();
        }
    }
}
