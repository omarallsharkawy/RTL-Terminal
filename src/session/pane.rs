//! One shell session: its terminal, PTY and the UI state that belongs to it alone.

use alacritty_terminal::index::Side;
use alacritty_terminal::term::TermMode;

use crate::blocks::BlockList;
use crate::input::mouse::{ClickCounter, StepAccumulator};
use crate::layout::PaneId;
use crate::pty::Pty;
use crate::remote::RemotePane;
use crate::shell_integration::{clean_text, MAX_NOTIFY_CHARS};
use crate::sidebar::{AgentState, AgentStatus};
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

/// Maximum status key/value entries stored per pane.
pub const MAX_STATUS_ENTRIES: usize = 16;

/// One status key/value entry reported via `workspace.set_status` or hooks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEntry {
    pub key: String,
    pub value: String,
    pub label: Option<String>,
    pub icon: Option<String>,
    pub color: Option<String>,
}

/// What the shell and the programs in a pane reported through shell signals
/// (`shell_integration`) or control hooks. The sidebar, notifications, blocks and the control
/// socket read it.
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
    /// Per-pane agent status (working / needs input / done) and optional last message.
    pub agent_status: Option<AgentStatus>,
    agent_input_seen: bool,
    /// Bounded key/value status entries reported for this pane.
    pub status_entries: Vec<StatusEntry>,
}

impl PaneMeta {
    /// Insert or update a status key on this pane and refresh `agent_status`.
    pub fn set_status(
        &mut self,
        key: &str,
        value: &str,
        label: Option<&str>,
        icon: Option<&str>,
        color: Option<&str>,
    ) {
        let key = clean_text(key, 64);
        let value = clean_text(value, 64);
        if key.is_empty() {
            return;
        }
        let entry = StatusEntry {
            key: key.clone(),
            value,
            label: label
                .map(|s| clean_text(s, MAX_NOTIFY_CHARS))
                .filter(|s| !s.is_empty()),
            icon: icon.map(|s| clean_text(s, 64)).filter(|s| !s.is_empty()),
            color: color.map(|s| clean_text(s, 64)).filter(|s| !s.is_empty()),
        };
        if let Some(existing) = self.status_entries.iter_mut().find(|e| e.key == key) {
            *existing = entry;
        } else {
            if self.status_entries.len() >= MAX_STATUS_ENTRIES {
                self.status_entries.remove(0);
            }
            self.status_entries.push(entry);
        }
        self.refresh_agent_status();
    }

    /// Clear one status key (or all keys when `key` is `None`) and refresh `agent_status`.
    pub fn clear_status(&mut self, key: Option<&str>) {
        match key {
            Some(k) => self.status_entries.retain(|e| e.key != k),
            None => self.status_entries.clear(),
        }
        self.refresh_agent_status();
    }

    fn waiting_for_input(&self) -> bool {
        self.agent_status
            .as_ref()
            .is_some_and(|status| status.state == AgentState::NeedsInput)
    }

    pub fn needs_attention(&self) -> bool {
        self.attention.is_some() || (self.waiting_for_input() && !self.agent_input_seen)
    }

    pub fn mark_agent_input_seen(&mut self) {
        self.agent_input_seen = self.waiting_for_input();
    }

    fn refresh_agent_status(&mut self) {
        let was_waiting = self.waiting_for_input();
        let mut best: Option<AgentStatus> = None;
        for entry in &self.status_entries {
            let Some(state) = AgentState::from_status(&entry.key, &entry.value) else {
                continue;
            };
            let text = entry.label.as_deref();
            let candidate = AgentStatus::new(state, text);
            match &best {
                None => best = Some(candidate),
                Some(cur) if candidate.state.priority() >= cur.state.priority() => {
                    best = Some(candidate);
                }
                _ => {}
            }
        }
        self.agent_status = best;
        if self.waiting_for_input() != was_waiting {
            self.agent_input_seen = false;
        }
    }
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
