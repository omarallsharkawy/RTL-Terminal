//! "Ask an agent" (Ctrl+Shift+A): a split running a coding-agent CLI with the pane's output.

use std::path::PathBuf;

use super::App;
use crate::agent::{agent_argv, agent_prompt, resolve_agent};
use crate::layout::{PaneId, SplitDir};
use crate::pty::SpawnOptions;
use crate::terminal::Terminal;

impl App {
    pub(super) fn ask_agent(&mut self, id: PaneId) {
        let Some(pane) = self.ws.pane(id) else {
            return;
        };
        // What to ask about: the selection, else the selected (or last) command block, else
        // the visible screen with the last command the shell reported.
        let selection = pane
            .terminal
            .selection_text()
            .filter(|s| !s.trim().is_empty());
        let block = selection
            .is_none()
            .then(|| pane.blocks.selected_or_last_text(&pane.terminal))
            .flatten()
            .filter(|b| !b.output.trim().is_empty() || b.command.is_some());
        let meta = &pane.meta;
        let (context, command, exit) = match (selection, block) {
            (Some(text), _) => (text, None, None),
            (None, Some(b)) => (b.output, b.command, b.exit),
            (None, None) => (
                screen_text(&pane.terminal),
                meta.last_command.clone(),
                meta.last_exit,
            ),
        };
        let cwd = if pane.remote.is_none() {
            meta.cwd.clone()
        } else {
            None
        };
        let prompt = agent_prompt(&context, command.as_deref(), exit, cwd.as_deref());
        self.launch_agent(id, cwd, &prompt);
    }

    /// Ask an agent specifically about command block `block_id` in pane `id` (from the card
    /// toolbar's Ask Agent button).
    pub(super) fn ask_agent_about_block(&mut self, id: PaneId, block_id: u64) {
        let Some(pane) = self.ws.pane(id) else {
            return;
        };
        let Some(block) = pane.blocks.block_text_by_id(block_id, &pane.terminal) else {
            return;
        };
        let cwd = if pane.remote.is_none() {
            pane.meta.cwd.clone()
        } else {
            None
        };
        let prompt = agent_prompt(
            &block.output,
            block.command.as_deref(),
            block.exit,
            cwd.as_deref(),
        );
        self.launch_agent(id, cwd, &prompt);
    }

    fn launch_agent(&mut self, id: PaneId, cwd: Option<PathBuf>, prompt: &str) {
        let note = match self.start_agent(cwd, prompt) {
            Ok(_) => return,
            Err(AgentStartError::NotFound) => {
                "\r\n\x1b[33mTwitty: no coding agent found (claude, codex, gemini, \
                 opencode, grok). Set \"agent_command\" in config.json. / مفيش وكيل برمجة متسطّب؛ اضبط agent_command في config.json\x1b[0m\r\n"
            }
            Err(AgentStartError::Failed) => {
                "\r\n\x1b[33mTwitty: failed to start the coding agent. / مقدرناش نشغّل وكيل البرمجة.\x1b[0m\r\n"
            }
        };
        self.term(id, |t| t.process_bytes(note.as_bytes()));
        self.window.request_redraw();
    }

    /// Start the configured (or first installed) coding agent with `prompt` as its first
    /// message, in a split of the focused pane (or a new tab when there is no room). Returns
    /// the agent's program name.
    pub(super) fn start_agent(
        &mut self,
        cwd: Option<PathBuf>,
        prompt: &str,
    ) -> Result<String, AgentStartError> {
        let path = std::env::var_os("PATH");
        let agent = resolve_agent(self.config.agent_command.as_deref(), path.as_deref())
            .ok_or(AgentStartError::NotFound)?;
        let name = agent
            .first()
            .and_then(|p| std::path::Path::new(p).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let options = SpawnOptions {
            cwd,
            command: agent_argv(&agent, prompt),
            env: Vec::new(),
            shell: false,
        };
        // Prefer a side-by-side split; when the sidebar or existing splits leave the pane too
        // narrow for MIN_SPLIT_COLS, fall back to a stacked split, then a new tab.
        let spawned = if self.can_split_focused(SplitDir::Horizontal) {
            self.split_focused_with(SplitDir::Horizontal, options).ok()
        } else if self.can_split_focused(SplitDir::Vertical) {
            self.split_focused_with(SplitDir::Vertical, options).ok()
        } else {
            self.new_tab_with(options)
        };
        spawned.map(|_| name).ok_or(AgentStartError::Failed)
    }
}

/// Why no agent was started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentStartError {
    /// No agent CLI is installed and none is configured.
    NotFound,
    /// The agent's pane could not be opened.
    Failed,
}

/// The visible screen as plain text in logical order.
fn screen_text(terminal: &Terminal) -> String {
    let (lines, _) = terminal.snapshot();
    let mut text = String::new();
    for line in &lines {
        let row: String = line
            .cells
            .iter()
            .filter(|c| !c.is_spacer())
            .flat_map(|c| std::iter::once(c.c).chain(c.zerowidth.iter().copied()))
            .collect();
        text.push_str(row.trim_end());
        text.push('\n');
    }
    text
}
