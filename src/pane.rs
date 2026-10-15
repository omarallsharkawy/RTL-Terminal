//! One shell session: its terminal, PTY and the UI state that belongs to it alone.

use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;
use std::sync::{Arc, Mutex};

use crate::input::mouse::{ClickCounter, StepAccumulator};
use crate::layout::PaneId;
use crate::pty::Pty;
use crate::terminal::Terminal;

pub struct Pane {
    pub id: PaneId,
    pub terminal: Arc<Mutex<Terminal>>,
    pub pty: Option<Pty>,
    /// Grid size last sent to the terminal and the PTY.
    pub size: (usize, usize),
    /// Window title set by the program (OSC 0/2).
    pub title: Option<String>,
    pub mouse_col: usize,
    pub mouse_row: usize,
    /// Button whose press was reported to the program (xterm codes 0-2), for drag and release
    /// reporting.
    pub mouse_button_down: Option<u8>,
    /// Side of the cell under the pointer (see `Renderer::pixel_to_visual_cell`).
    pub mouse_side: Side,
    /// A mouse selection is being dragged.
    pub selecting: bool,
    /// The current left press opened a link, so its release is swallowed.
    pub link_press: bool,
    /// Bumped on every new selection, so a delayed clear never removes a newer one.
    pub selection_generation: u64,
    pub clicks: ClickCounter,
    /// Fractional wheel/trackpad lines not yet applied.
    pub scroll_steps: StepAccumulator,
    /// Query being typed while scrollback search is open (keys go here, not to the PTY).
    pub search_query: Option<String>,
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
            mouse_col: 0,
            mouse_row: 0,
            mouse_button_down: None,
            mouse_side: Side::Left,
            selecting: false,
            link_press: false,
            selection_generation: 0,
            clicks: ClickCounter::default(),
            scroll_steps: StepAccumulator::default(),
            search_query: None,
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
