//! One shell session: its terminal, PTY and the UI state that belongs to it alone.

use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;

use crate::blocks::BlockList;
use crate::input::mouse::{ClickCounter, StepAccumulator};
use crate::layout::PaneId;
use crate::pty::Pty;
use crate::remote::RemotePane;
use crate::terminal::Terminal;
use crate::ui::selection::SelectionState;

pub(crate) struct Pane {
    pub id: PaneId,
    /// Only touched on the event-loop thread: PTY output arrives as events, so no lock.
    pub terminal: Terminal,
    pub pty: Option<Pty>,
    pub remote: Option<RemotePane>,
    /// Grid size last sent to the terminal and the PTY.
    pub size: (usize, usize),
    /// Window title set by the program (OSC 0/2).
    pub title: Option<String>,
    pub ui: PaneUi,
    /// What the shell reported about itself (OSC 7, 9, 133, 777…).
    pub meta: PaneMeta,
    /// Command blocks from OSC 133 marks.
    pub blocks: BlockList,
}

/// What the shell and the programs in a pane reported through shell signals
/// (`shell_integration`). The sidebar, notifications, blocks and the control socket read it.
#[derive(Debug, Default)]
pub(crate) struct PaneMeta {
    /// Working directory from OSC 7.
    pub cwd: Option<std::path::PathBuf>,
    /// An unseen notification (OSC 9/99/777) from this pane: the sidebar shows it until the
    /// pane is focused while the window has focus.
    pub attention: Option<Notification>,
    /// A command is running: OSC 133 C seen, D not yet.
    pub running: bool,
    /// Exit status of the last finished command (OSC 133 D).
    pub last_exit: Option<i32>,
    /// Last command line the shell reported (OSC 633 E).
    pub last_command: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Notification {
    pub title: Option<String>,
    pub body: String,
    pub at: std::time::SystemTime,
}

/// Pointer, selection and search state of one pane.
#[derive(Debug)]
pub(crate) struct PaneUi {
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
    /// Target of the link under the pointer while Ctrl is held, shown in the status line: a
    /// hyperlink's text can say anything, so the address it opens must be visible.
    pub hover_target: Option<String>,
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
            hover_target: None,
        }
    }
}

impl Pane {
    pub fn new(id: PaneId, terminal: Terminal, pty: Option<Pty>, size: (usize, usize)) -> Self {
        Self {
            id,
            terminal,
            pty,
            remote: None,
            size,
            title: None,
            ui: PaneUi::default(),
            meta: PaneMeta::default(),
            blocks: BlockList::new(),
        }
    }

    pub fn mode(&self) -> TermMode {
        self.terminal.mode()
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
        self.terminal.resize(cols, rows);
        self.blocks.refresh(&self.terminal);
        if let Some(ref pty) = self.pty {
            let _ = pty.resize(cols as u16, rows as u16);
        }
    }
}
