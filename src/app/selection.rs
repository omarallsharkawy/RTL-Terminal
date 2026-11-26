//! Mouse selection and the clipboard: select, copy on release with a brief highlight, paste.

use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;
use std::time::Instant;

use super::App;
use crate::input::encode_paste;
use crate::layout::PaneId;

impl App {
    pub(super) fn clear_selection(&mut self, id: PaneId) {
        let had = self
            .term(id, |term| {
                let had = term.has_selection();
                if had {
                    term.clear_selection();
                }
                had
            })
            .unwrap_or(false);
        if let Some(pane) = self.ws.pane_mut(id) {
            pane.ui.selection.end();
            pane.ui.selection.cancel_clear();
        }
        if had {
            self.window.request_redraw();
        }
    }

    /// Copy the selection to the clipboard. Returns false when nothing is selected.
    pub(super) fn copy_selection(&mut self, id: PaneId) -> bool {
        let text = self.term(id, |t| t.selection_text()).flatten();
        let Some(text) = text else { return false };
        self.clipboard.set_text(text);
        true
    }

    pub(super) fn start_selection(&mut self, id: PaneId) {
        let now = Instant::now();
        let side = self.ws.pane(id).map_or(Side::Left, |p| p.ui.mouse_side);
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        let (col, row) = (pane.ui.mouse_col, pane.ui.mouse_row);
        let clicks = pane.ui.clicks.click(now, col, row);
        pane.ui.selection.begin();
        pane.terminal.start_selection(col, row, side, clicks);
        self.window.request_redraw();
    }

    /// Mouse released: copy what was selected, flash it briefly, then clear the highlight once
    /// its deadline elapses in `App::about_to_wait`.
    pub(super) fn finish_selection(&mut self, id: PaneId) {
        if let Some(pane) = self.ws.pane_mut(id) {
            pane.ui.selection.end();
        }
        if !self.copy_selection(id) {
            self.clear_selection(id);
            return;
        }
        if let Some(pane) = self.ws.pane_mut(id) {
            pane.ui.selection.schedule_clear(Instant::now());
        }
    }

    /// Clear any copied-selection highlights whose deadline has passed at `now`, returning the
    /// earliest remaining deadline for `ControlFlow::WaitUntil`.
    pub(super) fn poll_selection_clears(&mut self, now: Instant) -> Option<Instant> {
        let due: Vec<PaneId> = self
            .ws
            .panes()
            .filter(|p| {
                p.ui.selection
                    .next_clear_deadline()
                    .is_some_and(|at| now >= at)
            })
            .map(|p| p.id)
            .collect();
        for id in due {
            let clear = self
                .ws
                .pane_mut(id)
                .is_some_and(|p| p.ui.selection.take_due_clear(now));
            if clear {
                self.clear_selection(id);
            }
        }
        self.ws
            .panes()
            .filter_map(|p| p.ui.selection.next_clear_deadline())
            .min()
    }

    pub(super) fn paste_clipboard(&mut self, id: PaneId) {
        let Some(text) = self.clipboard.text() else {
            return;
        };
        self.broadcast.cancel_escape();
        for target in self.broadcast_targets(id) {
            let bracketed = self.mode_of(target).contains(TermMode::BRACKETED_PASTE);
            self.clear_selection(target);
            self.clear_block_selection(target);
            self.write_to(target, &encode_paste(&text, bracketed));
            self.scroll_to_bottom(target);
        }
        self.cursor_blink.reset(Instant::now());
    }
}
