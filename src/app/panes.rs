//! Tabs and panes as the window sees them: opening, splitting, focusing and closing, and
//! keeping every pane's grid in step with the window.

use alacritty_terminal::term::TermMode;
use std::time::Instant;

use super::App;
use crate::layout::{FocusDir, PaneId, Rect, SplitDir};
use crate::session::spawn::ShellFactory;

impl App {
    pub(super) fn pane_rect(&self, id: PaneId) -> Option<Rect> {
        self.ws.pane_rect(self.window.grid()?, id)
    }

    pub(super) fn pane_at(&self, x: f64, y: f64) -> Option<PaneId> {
        self.ws.pane_at(self.window.grid()?, x, y)
    }

    /// Give every pane of every tab the grid its rectangle allows.
    pub(super) fn apply_layout(&mut self) {
        let Some(r) = self.window.grid() else {
            return;
        };
        self.ws.apply_layout(r);
        self.window.request_redraw();
    }

    pub(super) fn new_tab(&mut self) {
        let Some(r) = self.window.grid() else {
            return;
        };
        let previous = self.focused_id();
        let mut shells = ShellFactory::new(&self.proxy, self.config.cursor_blink);
        let Some(id) = self.ws.new_tab(r, &mut shells) else {
            return;
        };
        self.release_held_buttons();
        self.pointer.cancel();
        // A second tab shows the tab bar, which shrinks every pane.
        self.apply_layout();
        self.focus_changed(previous, id);
    }

    pub(super) fn split_focused(&mut self, dir: SplitDir) {
        let Some(r) = self.window.grid() else {
            return;
        };
        let mut shells = ShellFactory::new(&self.proxy, self.config.cursor_blink);
        let Some((old, new)) = self.ws.split_focused(r, &mut shells, dir) else {
            return;
        };
        self.apply_layout();
        self.focus_changed(Some(old), new);
    }

    /// Keyboard focus moved from `old` to `new`: tell the programs, restart the cursor blink and
    /// show the new pane's title.
    fn focus_changed(&mut self, old: Option<PaneId>, new: PaneId) {
        if old == Some(new) {
            return;
        }
        if self.window.is_focused() {
            if let Some(old) = old {
                self.report_focus(old, false);
            }
            self.report_focus(new, true);
        }
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

    pub(super) fn set_active_tab(&mut self, index: usize) {
        let old = self.focused_id();
        if !self.ws.set_active_tab(index) {
            return;
        }
        self.release_held_buttons();
        self.pointer.cancel();
        if let Some(new) = self.focused_id() {
            self.focus_changed(old, new);
        }
    }

    pub(super) fn cycle_tab(&mut self, step: isize) {
        if let Some(index) = self.ws.cycled_tab(step) {
            self.set_active_tab(index);
        }
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
            return true;
        };
        self.apply_layout();
        self.focus_changed(closed.previous, now);
        self.refresh_window_title();
        false
    }
}
