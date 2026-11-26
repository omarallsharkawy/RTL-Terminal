//! Bounded LIFO stack of recently closed tabs for "Reopen closed tab".

use std::collections::VecDeque;
use std::path::PathBuf;

use crate::shell_integration::clean_text;

/// Maximum number of closed tabs kept for reopening.
pub const MAX_CLOSED_TABS: usize = 20;

const MAX_TITLE_CHARS: usize = 256;

/// Snapshot of a closed tab's working directory and titles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosedTab {
    pub cwd: Option<PathBuf>,
    pub tab_title: Option<String>,
    pub pane_title: Option<String>,
}

impl ClosedTab {
    pub fn new(
        cwd: Option<PathBuf>,
        tab_title: Option<String>,
        pane_title: Option<String>,
    ) -> Self {
        Self {
            cwd: cwd.filter(|p| p.is_absolute()),
            tab_title: tab_title
                .as_deref()
                .map(|s| clean_text(s, MAX_TITLE_CHARS))
                .filter(|s| !s.is_empty()),
            pane_title: pane_title
                .as_deref()
                .map(|s| clean_text(s, MAX_TITLE_CHARS))
                .filter(|s| !s.is_empty()),
        }
    }

    /// The working directory if it still exists on disk as a directory.
    pub fn existing_cwd(&self) -> Option<PathBuf> {
        self.cwd
            .as_ref()
            .filter(|p| p.is_absolute() && p.is_dir())
            .cloned()
    }
}

/// Bounded stack of closed tabs, most recently closed on top.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClosedTabs {
    stack: VecDeque<ClosedTab>,
    capacity: usize,
}

impl ClosedTabs {
    pub fn new() -> Self {
        Self::with_capacity(MAX_CLOSED_TABS)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            stack: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    fn cap(&self) -> usize {
        if self.capacity == 0 {
            MAX_CLOSED_TABS
        } else {
            self.capacity
        }
    }

    /// Push a closed tab onto the stack, dropping the oldest if at capacity.
    pub fn push(&mut self, tab: ClosedTab) {
        let cap = self.cap();
        while self.stack.len() >= cap {
            self.stack.pop_front();
        }
        self.stack.push_back(tab);
    }

    /// Pop the most recently closed tab.
    pub fn pop(&mut self) -> Option<ClosedTab> {
        self.stack.pop_back()
    }

    pub fn len(&self) -> usize {
        self.stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}
