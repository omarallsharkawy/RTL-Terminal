//! Events the event loop receives from shells and terminals (through the event proxy).

use alacritty_terminal::event::{Event as TermEvent, WindowSize};
use std::time::{Duration, Instant};

use super::App;
use crate::layout::PaneId;
use crate::pty::Pty;

#[derive(Debug)]
pub enum AppEvent {
    /// The shell of a pane wrote output; it is waiting in the pane's queue.
    PtyOutput(PaneId),
    Terminal(PaneId, TermEvent),
    PtyExit(PaneId),
    /// Clear the highlight of a copied selection, unless a newer selection was started.
    ClearSelection(PaneId, u64),
}

/// Longest stretch spent parsing output before the screen is redrawn.
const OUTPUT_BUDGET: Duration = Duration::from_millis(12);

impl App {
    /// Output from the shell of pane `id`: everything queued since the last event, in one go.
    pub(super) fn on_pty_output(&mut self, id: PaneId) {
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        let term = &mut pane.terminal;
        let before = term.cursor_position();
        // Keep taking output that arrives meanwhile, but leave time for a frame: a flood never
        // freezes the screen, and an idle shell is drawn as soon as its output is parsed.
        let start = Instant::now();
        let mut any = false;
        loop {
            let data = pane.pty.as_ref().map(Pty::take_output).unwrap_or_default();
            if data.is_empty() {
                break;
            }
            any = true;
            term.process_bytes(&data);
            if start.elapsed() >= OUTPUT_BUDGET {
                break;
            }
        }
        if !any {
            return;
        }
        let moved = term.cursor_position() != before;
        // Output that moves the cursor keeps it solid while text is appearing.
        if moved && self.focused_id() == Some(id) {
            self.cursor_blink.reset(Instant::now());
        }
        if self.ws.is_visible(id) {
            self.window.request_redraw();
        }
    }

    /// The delayed clear of a copied selection.
    pub(super) fn on_clear_selection(&mut self, id: PaneId, generation: u64) {
        let current = self
            .ws
            .pane(id)
            .is_some_and(|p| p.ui.selection.may_clear(generation));
        if current {
            self.clear_selection(id);
        }
    }

    /// Something the terminal of pane `id` asks for: replies, title, bell, clipboard.
    pub(super) fn on_terminal_event(&mut self, id: PaneId, event: TermEvent) {
        match event {
            TermEvent::PtyWrite(text) => self.write_to(id, text.as_bytes()),
            TermEvent::Title(title) => {
                if let Some(pane) = self.ws.pane_mut(id) {
                    pane.title = Some(title);
                }
                self.refresh_window_title();
                self.window.request_redraw();
            }
            TermEvent::ResetTitle => {
                if let Some(pane) = self.ws.pane_mut(id) {
                    pane.title = None;
                }
                self.refresh_window_title();
                self.window.request_redraw();
            }
            TermEvent::Bell => self.window.bell(),
            TermEvent::ColorRequest(index, format) => {
                // OSC 4/10/11/12 queries (used by vim, fzf, delta to detect dark themes).
                // A color the program redefined itself (OSC 4) answers with its new value.
                let redefined = self.term(id, |t| t.program_palette_color(index)).flatten();
                let Some(rgb) = redefined.or_else(|| self.window.palette_rgb(index)) else {
                    return;
                };
                self.write_to(id, format(rgb).as_bytes());
            }
            TermEvent::TextAreaSizeRequest(format) => {
                let Some((cell_width, cell_height)) = self.window.cell_size() else {
                    return;
                };
                let Some((cols, rows)) = self.ws.pane(id).map(|p| p.size) else {
                    return;
                };
                let size = WindowSize {
                    num_lines: rows as u16,
                    num_cols: cols as u16,
                    cell_width,
                    cell_height,
                };
                self.write_to(id, format(size).as_bytes());
            }
            TermEvent::ClipboardStore(_, text) => {
                // OSC 52 copy (e.g. from tmux/vim over SSH).
                self.clipboard.set_text(text);
            }
            // OSC 52 reads are refused: any program could silently exfiltrate the clipboard.
            TermEvent::ClipboardLoad(..) => {}
            _ => {}
        }
    }
}
