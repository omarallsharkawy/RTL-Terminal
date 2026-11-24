//! "Ask an agent" (Ctrl+Shift+A): a split running a coding-agent CLI with the pane's output.

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
        let cwd = meta.cwd.clone();
        let path = std::env::var_os("PATH");
        let Some(agent) = resolve_agent(self.config.agent_command.as_deref(), path.as_deref())
        else {
            let note = "\r\n\x1b[33mTwitty: no coding agent found (claude, codex, gemini, \
                        opencode, grok). Set \"agent_command\" in config.json.\x1b[0m\r\n";
            self.term(id, |t| t.process_bytes(note.as_bytes()));
            self.window.request_redraw();
            return;
        };
        let prompt = agent_prompt(&context, command.as_deref(), exit, cwd.as_deref());
        let options = SpawnOptions {
            cwd,
            command: agent_argv(&agent, &prompt),
            env: Vec::new(),
        };
        // Prefer a side-by-side split; when the sidebar or existing splits leave the pane too
        // narrow for MIN_SPLIT_COLS, fall back to a stacked split, then a new tab.
        let spawned = if self.ws.can_split_focused(SplitDir::Horizontal) {
            self.split_focused_with(SplitDir::Horizontal, options)
        } else if self.ws.can_split_focused(SplitDir::Vertical) {
            self.split_focused_with(SplitDir::Vertical, options)
        } else {
            self.new_tab_with(options)
        };
        if spawned.is_none() {
            let note = "\r\n\x1b[33mTwitty: failed to start the coding agent.\x1b[0m\r\n";
            self.term(id, |t| t.process_bytes(note.as_bytes()));
            self.window.request_redraw();
        }
    }
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
