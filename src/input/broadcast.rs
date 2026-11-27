//! Broadcast input state: toggle sending keystrokes and pastes to every pane of a tab.
//!
//! Off by default. Scoped per stable tab id (`Tab::id`). Turned off by toggling again or pressing
//! plain `Escape` twice within [`DOUBLE_ESC_WINDOW`]. Window-free so routing and double-Esc
//! behaviour can be tested directly.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::layout::PaneId;
use crate::ui::chrome::UiLanguage;

/// Maximum interval between two plain Escape presses that disables broadcast mode.
pub const DOUBLE_ESC_WINDOW: Duration = Duration::from_millis(500);

/// Status text shown when pane headers are hidden and broadcast is active.
pub fn broadcast_status_text() -> &'static str {
    crate::ui::chrome::tr(
        "Broadcast: typing goes to all panes (Esc Esc to stop)",
        "البث: الكتابة بتروح لكل اللوحات (Esc Esc للإيقاف)",
    )
}

/// Format a pane header title with a compact bilingual broadcast badge at the front so it stays
/// visible even in narrow split panes and preserves direction for the active UI language.
pub fn format_broadcast_header_title(title: &str, language: UiLanguage) -> String {
    match language {
        UiLanguage::Arabic => format!("[بث · ALL] {title}"),
        UiLanguage::English => format!("[ALL · بث] {title}"),
    }
}

/// Per-tab broadcast toggle and `Esc-Esc` detector.
#[derive(Clone, Debug, Default)]
pub struct BroadcastState {
    tabs: HashSet<u64>,
    last_escape: Option<(u64, Instant)>,
}

impl BroadcastState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether broadcast input is currently enabled for `tab_id`. Always `false` by default.
    pub fn is_active(&self, tab_id: u64) -> bool {
        self.tabs.contains(&tab_id)
    }

    /// Toggle broadcast input on `tab_id`. Returns the new state (`true` = active).
    pub fn toggle(&mut self, tab_id: u64) -> bool {
        self.last_escape = None;
        if self.tabs.remove(&tab_id) {
            false
        } else {
            self.tabs.insert(tab_id);
            true
        }
    }

    /// Explicitly disable broadcast input on `tab_id`. Returns `true` if it was active.
    pub fn disable(&mut self, tab_id: u64) -> bool {
        self.clear_escape_for(tab_id);
        self.tabs.remove(&tab_id)
    }

    /// Forget closed tabs so memory stays bounded.
    pub fn forget_tab(&mut self, tab_id: u64) {
        self.clear_escape_for(tab_id);
        self.tabs.remove(&tab_id);
    }

    /// Cancel any pending first-Escape timestamp (called on non-Escape keys, paste, IME commit,
    /// focus/tab switch).
    pub fn cancel_escape(&mut self) {
        self.last_escape = None;
    }

    fn clear_escape_for(&mut self, tab_id: u64) {
        if self.last_escape.is_some_and(|(id, _)| id == tab_id) {
            self.last_escape = None;
        }
    }

    /// Handle a plain (unmodified, non-repeat) `Escape` press on `tab_id` at `now`.
    ///
    /// Returns `true` when this is the second Escape within [`DOUBLE_ESC_WINDOW`] while broadcast
    /// is active — turning broadcast off and signaling the caller to consume the key.
    pub fn on_plain_escape(&mut self, tab_id: u64, now: Instant) -> bool {
        if !self.is_active(tab_id) {
            self.last_escape = None;
            return false;
        }
        if let Some((prev_tab, prev_at)) = self.last_escape {
            if prev_tab == tab_id && now.saturating_duration_since(prev_at) <= DOUBLE_ESC_WINDOW {
                self.tabs.remove(&tab_id);
                self.last_escape = None;
                return true;
            }
        }
        self.last_escape = Some((tab_id, now));
        false
    }

    /// Which panes receive user input aimed at `focused` in `tab_id` (whose panes are `tab_panes`).
    ///
    /// When broadcast is off, returns `[focused]`. When broadcast is on, returns every pane in
    /// `tab_panes` (falling back to `[focused]` if `tab_panes` is empty).
    pub fn target_panes(&self, tab_id: u64, focused: PaneId, tab_panes: &[PaneId]) -> Vec<PaneId> {
        if self.is_active(tab_id) && !tab_panes.is_empty() {
            tab_panes.to_vec()
        } else {
            vec![focused]
        }
    }
}
