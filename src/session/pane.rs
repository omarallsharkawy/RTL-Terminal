//! One shell session: its terminal, PTY and the UI state that belongs to it alone.

use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;
use std::sync::{Arc, Mutex};

use crate::input::mouse::{ClickCounter, StepAccumulator};
use crate::layout::PaneId;
use crate::pty::Pty;
use crate::terminal::Terminal;
use crate::ui::selection::SelectionState;

pub struct Pane {
    pub id: PaneId,
    pub terminal: Arc<Mutex<Terminal>>,
    pub pty: Option<Pty>,
    /// Grid size last sent to the terminal and the PTY.
    pub size: (usize, usize),
    /// Window title set by the program (OSC 0/2).
    pub title: Option<String>,
    pub ui: PaneUi,
}

/// Pointer, selection and search state of one pane.
#[derive(Debug)]
pub struct PaneUi {
    pub mouse_col: usize,
    pub mouse_row: usize,
    /// Button whose press was reported to the program (xterm codes 0-2), for drag and release
    /// reporting.
    pub mouse_button_down: Option<u8>,
    /// Side of the cell under the pointer (see `Renderer::pixel_to_visual_cell`).
    pub mouse_side: Side,
    pub selection: SelectionState,
    /// The current left press opened a link, so its release is swallowed.
    pub link_press: bool,
    pub clicks: ClickCounter,
    /// Fractional wheel/trackpad lines not yet applied.
    pub scroll_steps: StepAccumulator,
    /// Query being typed while scrollback search is open (keys go here, not to the PTY).
    pub search_query: Option<String>,
}

impl Default for PaneUi {
    fn default() -> Self {
        Self {
            mouse_col: 0,
            mouse_row: 0,
            mouse_button_down: None,
            mouse_side: Side::Left,
            selection: SelectionState::default(),
            link_press: false,
            clicks: ClickCounter::default(),
            scroll_steps: StepAccumulator::default(),
            search_query: None,
        }
    }
}

impl Pane {
    pub fn new(
        id: PaneId,
        terminal: Arc<Mutex<Terminal>>,
        pty: Option<Pty>,
        size: (usize, usize),
    ) -> Self {
        Self {
            id,
            terminal,
            pty,
            size,
            title: None,
            ui: PaneUi::default(),
        }
    }

    pub fn mode(&self) -> TermMode {
        self.terminal
            .lock()
            .map(|t| t.mode())
            .unwrap_or(TermMode::NONE)
    }

    pub fn write(&self, bytes: &[u8]) {
        if let Some(ref pty) = self.pty {
            let _ = pty.write(bytes);
        }
    }

    /// Resize the terminal and the PTY; nothing happens when the grid is unchanged.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if self.size == (cols, rows) {
            return;
        }
        self.size = (cols, rows);
        if let Ok(mut term) = self.terminal.lock() {
            term.resize(cols, rows);
        }
        if let Some(ref pty) = self.pty {
            let _ = pty.resize(cols as u16, rows as u16);
        }
    }
}
