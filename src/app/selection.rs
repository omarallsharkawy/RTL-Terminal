//! Mouse selection and the clipboard: select, copy on release with a brief highlight, paste.

use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;
use std::time::Instant;

use super::{App, AppEvent};
use crate::input::encode_paste;
use crate::layout::PaneId;
use crate::ui::selection::COPIED_HIGHLIGHT;

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

    /// Mouse released: copy what was selected, flash it briefly, then clear the highlight.
    pub(super) fn finish_selection(&mut self, id: PaneId) {
        if let Some(pane) = self.ws.pane_mut(id) {
            pane.ui.selection.end();
        }
        if !self.copy_selection(id) {
            self.clear_selection(id);
            return;
        }
        let Some(generation) = self.ws.pane(id).map(|p| p.ui.selection.generation()) else {
            return;
        };
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            std::thread::sleep(COPIED_HIGHLIGHT);
            let _ = proxy.send_event(AppEvent::ClearSelection(id, generation));
        });
    }

    pub(super) fn paste_clipboard(&mut self, id: PaneId) {
        let Some(text) = self.clipboard.text() else {
            return;
        };
        let bracketed = self.mode_of(id).contains(TermMode::BRACKETED_PASTE);
        self.write_to(id, &encode_paste(&text, bracketed));
        self.cursor_blink.reset(Instant::now());
        self.scroll_to_bottom(id);
    }
}
