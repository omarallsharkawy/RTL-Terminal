//! Short notices that float over the window for a few seconds ("Copied", "Could not save
//! layout: …") instead of being printed into a terminal.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// At most this many notices show at once; older ones leave first.
pub const MAX_TOASTS: usize = 3;
const LIFETIME: Duration = Duration::from_millis(2500);
const ERROR_LIFETIME: Duration = Duration::from_secs(6);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub until: Instant,
}

#[derive(Default, Debug)]
pub struct Toasts {
    list: VecDeque<Toast>,
}

impl Toasts {
    /// Show `text` from `now`. The same text again restarts its timer instead of stacking.
    pub fn push(&mut self, kind: ToastKind, text: impl Into<String>, now: Instant) {
        let text = crate::shell_integration::clean_text(&text.into(), 300);
        if text.is_empty() {
            return;
        }
        let life = match kind {
            ToastKind::Error => ERROR_LIFETIME,
            ToastKind::Info | ToastKind::Success => LIFETIME,
        };
        self.list.retain(|t| t.text != text);
        self.list.push_back(Toast {
            kind,
            text,
            until: now + life,
        });
        while self.list.len() > MAX_TOASTS {
            self.list.pop_front();
        }
    }

    /// Drop expired notices; true when any left.
    pub fn prune(&mut self, now: Instant) -> bool {
        let before = self.list.len();
        self.list.retain(|t| t.until > now);
        self.list.len() != before
    }

    /// When the next notice expires.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.list.iter().map(|t| t.until).min()
    }

    /// Oldest first.
    pub fn visible(&self) -> Vec<Toast> {
        self.list.iter().cloned().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}
