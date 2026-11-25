//! Tabs and panes as the window sees them: opening, splitting, focusing and closing, and
//! keeping every pane's grid in step with the window.

use alacritty_terminal::term::TermMode;
use std::time::Instant;

use super::{App, AppEvent};
use crate::config::TwittyConfig;
use crate::layout::{FocusDir, PaneId, Rect, SplitDir};
use crate::pty::SpawnOptions;
use crate::remote::Target;
use crate::session::spawn::ShellFactory;
use winit::event_loop::EventLoopProxy;

fn shell_factory<'a>(
    proxy: &'a EventLoopProxy<AppEvent>,
    config: &TwittyConfig,
    socket: Option<String>,
    options: SpawnOptions,
) -> ShellFactory<'a> {
    let mut shells = ShellFactory::new(proxy, config.cursor_blink)
        .with_shell_integration(config.shell_integration)
        .with_control(socket, config.cmux_compat, config.claude_hooks);
    shells.options = options;
    shells
}

impl App {
    pub(super) fn start_session(&mut self) {
        if let Some(ssh) = self.startup_ssh.take() {
            self.new_ssh_tab(ssh);
            return;
        }
        if let Some(session) = self.startup_session.take() {
            let was_visible = self.window.sidebar_visible();
            self.window.set_sidebar_visible(session.sidebar_visible);
            let socket = self.control_socket_path();
            let mut shells =
                shell_factory(&self.proxy, &self.config, socket, SpawnOptions::default());
            self.ws = session.rebuild(&self.window, &mut shells);
            if let Some(id) = self.focused_id() {
                self.focus_changed(None, id);
                return;
            }
            self.window.set_sidebar_visible(was_visible);
            crate::config::warn("twitty: no shells from the saved session could be started".into());
        }
        self.new_tab();
    }

    pub(super) fn pane_rect(&self, id: PaneId) -> Option<Rect> {
        self.ws
            .pane_rect(self.window.grid()?, id)
            .map(|rect| self.window.pane_layout(rect).grid)
    }

    pub(super) fn pane_at(&self, x: f64, y: f64) -> Option<PaneId> {
        self.ws
            .pane_at(self.window.grid()?, x, y)
            .filter(|&id| self.pane_rect(id).is_some_and(|rect| rect.contains(x, y)))
    }

    /// Give every pane of every tab the grid its rectangle allows.
    pub(super) fn apply_layout(&mut self) {
        let Some(r) = self.window.grid() else {
            return;
        };
        self.ws.apply_layout(r);
        self.refresh_pointer_hover();
        self.window.request_redraw();
    }

    pub(super) fn new_tab(&mut self) {
        self.new_tab_with(SpawnOptions::default());
    }

    pub(super) fn new_tab_with(&mut self, options: SpawnOptions) -> Option<PaneId> {
        self.new_tab_with_process(options, None)
    }

    pub(super) fn new_ssh_tab(&mut self, target: Target) -> Option<PaneId> {
        self.new_tab_with_process(SpawnOptions::default(), Some(target))
    }

    fn new_tab_with_process(
        &mut self,
        options: SpawnOptions,
        remote: Option<Target>,
    ) -> Option<PaneId> {
        let r = self.window.grid()?;
        let previous = self.focused_id();
        let socket = self.control_socket_path();
        let mut shells = shell_factory(&self.proxy, &self.config, socket, options);
        shells.remote = remote;
        let id = self.ws.new_tab(r, &mut shells)?;
        self.reset_pointer_gesture();
        // A second tab shows the tab bar, which shrinks every pane.
        self.apply_layout();
        self.focus_changed(previous, id);
        Some(id)
    }

    pub(super) fn split_focused(&mut self, dir: SplitDir) {
        self.split_focused_with(dir, SpawnOptions::default());
    }

    /// Split the focused pane, starting the new one as `options` says. Returns the new pane.
    pub(super) fn split_focused_with(
        &mut self,
        dir: SplitDir,
        options: SpawnOptions,
    ) -> Option<PaneId> {
        let r = self.window.grid()?;
        let socket = self.control_socket_path();
        let mut shells = shell_factory(&self.proxy, &self.config, socket, options);
        let (old, new) = self.ws.split_focused(r, &mut shells, dir)?;
        self.apply_layout();
        self.focus_changed(Some(old), new);
        Some(new)
    }

    /// Keyboard focus moved from `old` to `new`: close any open palette, tell the programs,
    /// reveal the active tab in the sidebar, restart the cursor blink and show the new pane's title.
    fn focus_changed(&mut self, old: Option<PaneId>, new: PaneId) {
        if old == Some(new) {
            return;
        }
        self.close_palette();
        if self.window_focused {
            if let Some(pane) = self.ws.pane_mut(new) {
                pane.meta.attention = None;
                pane.meta.mark_agent_input_seen();
            }
        }
        if self.window.is_focused() {
            if let Some(old) = old {
                self.report_focus(old, false);
            }
            self.report_focus(new, true);
        }
        self.window.reveal_active_tab();
        self.cursor_blink.reset(Instant::now());
        self.refresh_window_title();
        self.window.request_redraw();
    }

    /// Tell the program in pane `id` it gained or lost focus, if it asked (mode 1004).
    pub(super) fn report_focus(&self, id: PaneId, focused: bool) {
        if self.mode_of(id).contains(TermMode::FOCUS_IN_OUT) {
            self.write_to(id, if focused { b"\x1b[I" } else { b"\x1b[O" });
        }
    }

    pub(super) fn focus_pane(&mut self, id: PaneId) {
        if let Some(old) = self.ws.focus_pane(id) {
            self.focus_changed(Some(old), id);
        }
    }

    /// Focus pane `id` across tabs in one atomic step (no intermediate focus events on the
    /// target tab's previously focused pane). Returns the tab index holding `id`.
    pub(super) fn focus_pane_by_id(&mut self, id: PaneId) -> Result<usize, String> {
        let Some((tab_index, old, tab_changed)) = self.ws.focus_any_pane(id) else {
            return Err(format!("unknown pane: {}", id.0));
        };
        if tab_changed {
            self.reset_pointer_gesture();
        }
        self.focus_changed(old, id);
        if old == Some(id) {
            self.window.reveal_active_tab();
            self.window.request_redraw();
        }
        Ok(tab_index)
    }

    pub(super) fn set_active_tab(&mut self, index: usize) {
        let old = self.focused_id();
        if !self.ws.set_active_tab(index) {
            return;
        }
        self.reset_pointer_gesture();
        if let Some(new) = self.focused_id() {
            self.focus_changed(old, new);
        }
    }

    pub(super) fn cycle_tab(&mut self, step: isize) {
        if let Some(index) = self.ws.cycled_tab(step) {
            self.set_active_tab(index);
        }
    }

    /// Focus the pane with the oldest unseen notification, switching tabs when needed.
    pub(super) fn focus_next_attention(&mut self) {
        let Some((_, id)) = self.ws.oldest_attention() else {
            return;
        };
        let _ = self.focus_pane_by_id(id);
        if self.window_focused {
            if let Some(pane) = self.ws.pane_mut(id) {
                pane.meta.attention = None;
            }
        }
        self.window.request_redraw();
    }

    pub(super) fn focus_direction(&mut self, dir: FocusDir) {
        let Some(r) = self.window.grid() else {
            return;
        };
        if let Some(to) = self.ws.neighbor(r, dir) {
            self.focus_pane(to);
        }
    }

    /// Close one pane. Returns true when it was the last one and the window should close.
    pub(super) fn close_pane(&mut self, id: PaneId) -> bool {
        let Some(closed) = self.ws.close_pane(id) else {
            return false;
        };
        self.window.forget_pane(id);
        self.pointer.forget(id);
        let Some(now) = closed.focused else {
            self.close_palette();
            self.session.last_pane_closed();
            return true;
        };
        self.apply_layout();
        self.focus_changed(closed.previous, now);
        self.refresh_window_title();
        false
    }

    /// Close all panes in tab `index`. Returns true when it was the last tab and the window
    /// should close.
    pub(super) fn close_tab(&mut self, index: usize) -> bool {
        let Some((removed, closed)) = self.ws.close_tab(index) else {
            return false;
        };
        for id in removed {
            self.window.forget_pane(id);
            self.pointer.forget(id);
        }
        let Some(now) = closed.focused else {
            self.close_palette();
            self.session.last_pane_closed();
            return true;
        };
        self.apply_layout();
        self.focus_changed(closed.previous, now);
        self.refresh_window_title();
        false
    }
}
