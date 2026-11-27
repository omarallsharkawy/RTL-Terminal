//! Window-free `Workspace`-backed [`ControlHost`] harness for control protocol tests.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::SystemTime;

use serde_json::{json, Value};

use super::{
    dispatch_if_active, dispatch_request, resolve_twitty_pane, ControlHost, MAX_TITLE_CHARS,
};
use crate::control::protocol::{parse_wire_line, Request, Response};
use crate::layout::{PaneId, Rect, SplitDir};
use crate::pty::SpawnOptions;
use crate::session::pane::{Notification, Pane};
use crate::session::workspace::{GridGeometry, PaneFactory, SplitFailure, Workspace};
use crate::shell_integration::clean_text;
use crate::sidebar::{entry_title, pick_tab_agent_status, AgentStatus, Entry, Notice};
use crate::terminal::Terminal;
use crate::ui::notifications::NotificationHistory;

// ---------------------------------------------------------------------------
// HeadlessHost: window-free Workspace-backed harness for tests
// ---------------------------------------------------------------------------

struct HeadlessGrid;

impl GridGeometry for HeadlessGrid {
    fn content_rect(&self, tab_count: usize) -> Rect {
        let bar = if tab_count > 1 { 20.0 } else { 0.0 };
        Rect::new(0.0, bar, 1000.0, 600.0 - bar)
    }

    fn gap(&self) -> f32 {
        1.0
    }

    fn grid_size(&self, rect: Rect) -> (usize, usize) {
        ((rect.w / 10.0) as usize, (rect.h / 20.0) as usize)
    }
}

struct HeadlessFactory {
    options: SpawnOptions,
    tab_id: u64,
    spawned: Vec<(PaneId, u64, SpawnOptions)>,
}

impl PaneFactory for HeadlessFactory {
    fn set_tab_id(&mut self, tab_id: u64) {
        self.tab_id = tab_id;
    }

    fn spawn(
        &mut self,
        id: PaneId,
        cols: usize,
        rows: usize,
        cwd: Option<PathBuf>,
    ) -> Option<Pane> {
        let mut opts = self.options.clone();
        opts.cwd = opts.cwd.or(cwd);
        let terminal = Terminal::new(cols, rows, |_| {});
        let mut pane = Pane::new(id, terminal, None, (cols, rows));
        pane.meta.cwd = opts.cwd.clone();
        self.spawned.push((id, self.tab_id, opts));
        Some(pane)
    }
}

/// Window-free `Workspace` + [`NotificationHistory`] harness implementing control dispatch so
/// every cmux v2, v1, and Twitty `ctl` method can be tested without a GUI window or PTY.
pub struct HeadlessHost {
    ws: Workspace,
    notifications: NotificationHistory,
    sidebar_visible: bool,
    window_focused: bool,
    written_inputs: Vec<(u64, Vec<u8>)>,
    desktop_notifications: Vec<(u64, Option<String>, String)>,
    spawned: Vec<(u64, u64, SpawnOptions)>,
}

impl Default for HeadlessHost {
    fn default() -> Self {
        Self::new()
    }
}

impl HeadlessHost {
    /// Start a headless host with one initial tab and pane.
    pub fn new() -> Self {
        let mut host = Self {
            ws: Workspace::new(),
            notifications: NotificationHistory::default(),
            sidebar_visible: true,
            window_focused: false,
            written_inputs: Vec::new(),
            desktop_notifications: Vec::new(),
            spawned: Vec::new(),
        };
        host.new_tab_with(SpawnOptions::default());
        host
    }

    /// Dispatch a parsed [`Request`], returning `(Response, should_exit)`.
    pub fn dispatch(&mut self, req: Request) -> (Response, bool) {
        dispatch_request(self, req)
    }

    /// Parse and dispatch one raw wire line (JSON or minimal v1 text).
    pub fn dispatch_line(&mut self, line: &str) -> (Response, bool) {
        match parse_wire_line(line) {
            Ok(req) => dispatch_request(self, req),
            Err(resp) => (resp, false),
        }
    }

    /// Feed terminal bytes into pane `pane_id` (simulating program output).
    pub fn feed_pane_output(&mut self, pane_id: u64, bytes: &[u8]) {
        if let Some(pane) = self.ws.pane_mut(PaneId(pane_id)) {
            pane.terminal.process_bytes(bytes);
        }
    }

    /// Set whether the simulated window has focus (controls unseen attention / desktop notify).
    pub fn set_window_focused(&mut self, focused: bool) {
        self.window_focused = focused;
    }

    /// All input bytes sent to panes via `send_text` / `send_key`.
    pub fn written_inputs(&self) -> &[(u64, Vec<u8>)] {
        &self.written_inputs
    }

    /// Desktop notifications that would have been sent via `notify-send`.
    pub fn desktop_notifications(&self) -> &[(u64, Option<String>, String)] {
        &self.desktop_notifications
    }

    /// Recorded `(pane_id, tab_id, SpawnOptions)` for every pane created on this host.
    pub fn spawned(&self) -> &[(u64, u64, SpawnOptions)] {
        &self.spawned
    }

    pub fn tab_count(&self) -> usize {
        self.ws.tabs().len()
    }

    pub fn pane_count(&self) -> usize {
        self.ws.panes().count()
    }

    pub fn active_tab(&self) -> usize {
        self.ws.active()
    }

    pub fn focused_pane(&self) -> Option<u64> {
        self.ws.focused_id().map(|p| p.0)
    }

    pub fn dispatch_if_active(
        &mut self,
        req: Request,
        cancelled: &AtomicBool,
    ) -> Option<(Response, bool)> {
        dispatch_if_active(self, req, cancelled)
    }

    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    /// Read the per-pane [`AgentStatus`] stored on `pane_id`.
    pub fn pane_agent_status(&self, pane_id: u64) -> Option<AgentStatus> {
        self.ws.pane(PaneId(pane_id))?.meta.agent_status.clone()
    }

    /// Build sidebar [`Entry`] values exactly as the window renderer does.
    pub fn sidebar_entries(&self) -> Vec<Entry> {
        self.ws
            .tabs()
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let focused = self.ws.pane(tab.focused);
                let title = match tab.title.as_deref().filter(|t| !t.is_empty()) {
                    Some(custom) => clean_text(custom, MAX_TITLE_CHARS),
                    None => entry_title(focused.and_then(|p| p.title.as_deref()), None, index),
                };
                let mut notifications: Vec<_> = tab
                    .root
                    .leaves()
                    .into_iter()
                    .filter_map(|id| self.ws.pane(id)?.meta.attention.as_ref())
                    .collect();
                notifications.sort_by_key(|n| n.at);
                let focused_status = focused.and_then(|p| p.meta.agent_status.as_ref());
                let other_statuses: Vec<_> = tab
                    .root
                    .leaves()
                    .into_iter()
                    .filter(|&id| id != tab.focused)
                    .filter_map(|id| self.ws.pane(id)?.meta.agent_status.as_ref())
                    .collect();
                let agent_status = pick_tab_agent_status(focused_status, other_statuses);
                Entry::new(
                    &title,
                    focused.and_then(|p| p.meta.cwd.as_deref()),
                    focused.is_some_and(|p| p.meta.running),
                    focused.and_then(|p| p.meta.last_exit),
                    notifications
                        .into_iter()
                        .map(|n| Notice::new(n.title.as_deref(), &n.body)),
                )
                .with_agent_status(agent_status)
            })
            .collect()
    }
}

impl ControlHost for HeadlessHost {
    fn ws(&self) -> &Workspace {
        &self.ws
    }

    fn ws_mut(&mut self) -> &mut Workspace {
        &mut self.ws
    }

    fn notification_history(&self) -> &NotificationHistory {
        &self.notifications
    }

    fn notification_history_mut(&mut self) -> &mut NotificationHistory {
        &mut self.notifications
    }

    fn pane_cwd(&self, id: PaneId) -> Option<PathBuf> {
        self.ws.pane(id)?.meta.cwd.clone()
    }

    fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    fn set_sidebar_visible(&mut self, visible: bool) {
        self.sidebar_visible = visible;
    }

    fn toggle_sidebar(&mut self) -> bool {
        self.sidebar_visible = !self.sidebar_visible;
        self.sidebar_visible
    }

    fn new_tab_with(&mut self, options: SpawnOptions) -> Option<PaneId> {
        let mut factory = HeadlessFactory {
            options,
            tab_id: 0,
            spawned: Vec::new(),
        };
        let id = self.ws.new_tab(&HeadlessGrid, &mut factory)?;
        self.ws.apply_layout(&HeadlessGrid);
        for (pid, tid, opts) in factory.spawned {
            self.spawned.push((pid.0, tid, opts));
        }
        Some(id)
    }

    fn split_focused_with(
        &mut self,
        dir: SplitDir,
        options: SpawnOptions,
    ) -> Result<PaneId, SplitFailure> {
        if !self.ws.can_split_focused(&HeadlessGrid, dir) {
            return Err(self.ws.split_failure_reason(&HeadlessGrid, dir));
        }
        let mut factory = HeadlessFactory {
            options,
            tab_id: 0,
            spawned: Vec::new(),
        };
        let Some((_, new_id)) = self.ws.split_focused(&HeadlessGrid, &mut factory, dir) else {
            return Err(self.ws.split_failure_reason(&HeadlessGrid, dir));
        };
        self.ws.apply_layout(&HeadlessGrid);
        for (pid, tid, opts) in factory.spawned {
            self.spawned.push((pid.0, tid, opts));
        }
        Ok(new_id)
    }

    fn focus_pane_by_id(&mut self, id: PaneId) -> Result<usize, String> {
        let Some((tab, _, _)) = self.ws.focus_any_pane(id) else {
            return Err(format!("unknown pane: {}", id.0));
        };
        if self.window_focused {
            if let Some(pane) = self.ws.pane_mut(id) {
                pane.meta.attention = None;
            }
            self.notifications.mark_seen(id);
        }
        Ok(tab)
    }

    fn set_active_tab(&mut self, index: usize) {
        if self.ws.set_active_tab(index) && self.window_focused {
            if let Some(id) = self.ws.focused_id() {
                if let Some(pane) = self.ws.pane_mut(id) {
                    pane.meta.attention = None;
                }
                self.notifications.mark_seen(id);
            }
        }
    }

    fn close_pane(&mut self, id: PaneId) -> bool {
        let Some(closed) = self.ws.close_pane(id) else {
            return false;
        };
        if closed.focused.is_none() {
            return true;
        }
        self.ws.apply_layout(&HeadlessGrid);
        false
    }

    fn close_tab(&mut self, index: usize) -> bool {
        let Some((_, closed)) = self.ws.close_tab(index) else {
            return false;
        };
        if closed.focused.is_none() {
            return true;
        }
        self.ws.apply_layout(&HeadlessGrid);
        false
    }

    fn send_input(&mut self, id: PaneId, bytes: &[u8]) {
        self.written_inputs.push((id.0, bytes.to_vec()));
    }

    fn notify(&mut self, id: PaneId, title: Option<String>, body: String, send_desktop: bool) {
        let Some(tab_idx) = self.ws.tabs().iter().position(|t| t.root.contains(id)) else {
            return;
        };
        let tab_title = self.ws.tab_title(tab_idx);
        let seen = self.window_focused && self.ws.focused_id() == Some(id);
        self.notifications.push(
            id,
            title.as_deref(),
            &body,
            &tab_title,
            SystemTime::now(),
            seen,
        );
        let Some(notice) = crate::notify::unseen_notice(
            self.window_focused,
            self.ws.focused_id(),
            id,
            title.as_deref(),
            &body,
        ) else {
            return;
        };
        if let Some(pane) = self.ws.pane_mut(id) {
            pane.meta.attention = Some(Notification {
                title: notice.title.clone(),
                body: notice.body.clone(),
                at: SystemTime::now(),
            });
        }
        if send_desktop {
            self.desktop_notifications
                .push((id.0, notice.title, notice.body));
        }
    }

    fn request_redraw(&mut self) {}

    fn refresh_title(&mut self) {}

    fn command_list(&self) -> Value {
        Value::Array(crate::workflows::WorkflowSet::load(&[], None).control_list_json())
    }

    fn command_run(&mut self, name: &str, pane: Option<u64>) -> Result<(Value, bool), String> {
        let target = resolve_twitty_pane(self, pane)?;
        if let Some(action) = crate::commands::action(name) {
            match action {
                crate::input::InputAction::SplitRight => {
                    self.focus_pane_by_id(target)?;
                    self.split_focused_with(SplitDir::Horizontal, SpawnOptions::default())
                        .map_err(|err| err.message)?;
                }
                crate::input::InputAction::SplitDown => {
                    self.focus_pane_by_id(target)?;
                    self.split_focused_with(SplitDir::Vertical, SpawnOptions::default())
                        .map_err(|err| err.message)?;
                }
                _ => {}
            }
            Ok((json!({ "command": name, "pane": target.0 }), false))
        } else {
            Err(format!("unknown command: {name}"))
        }
    }
}
