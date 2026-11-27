//! App hooks for the Twitty Agent side panel (`docs/design/AGENT.md` §1):
//! opening/closing (`agent.panel`, `Ctrl+Shift+.`), mutual exclusion with Code review,
//! draggable divider resizing, lazy engine spawning over `AgentHandle` channels, running
//! approved commands in a visible pane (focused idle pane or a new "Agent" split), capturing
//! shell-integration block output/exit/duration on finish, Ctrl+C step cancellation, "Needs
//! input" status syncing on pane headers & sidebar cards, and forwarding `RememberFact` to
//! `twitty-store`.

use std::path::PathBuf;
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Instant;

use alacritty_terminal::term::TermMode;
use twitty_ai::context::{AssembleInput, SelectedBlock, DEFAULT_BUDGET_TOKENS};
use twitty_ai::engine::model::{
    AgentCommand, AgentEvent, AgentHandle, ContextBundle, SessionId, StepId,
};
use twitty_ai::engine::EngineConfig;
use twitty_core::redact::redact;
use twitty_store::SecretStore;
use winit::event::{KeyEvent, MouseButton};
use winit::keyboard::PhysicalKey;
use winit::window::CursorIcon;

use super::event::AppEvent;
use super::App;
use crate::blocks::Block;
use crate::composer::encode_submit;
use crate::input::route::shortcut_is_idle;
use crate::input::{key_shortcut_with, InputAction};
use crate::layout::{PaneId, SplitDir};
use crate::pty::{Pty, SpawnOptions};
use crate::ui::agent_panel::{
    width_from_divider_drag, AgentEngineSpawner, AgentPanelAction, AgentPanelHit, AgentPanelLayout,
    AgentPanelState, AgentPanelView,
};

/// The real engine (`twitty_ai::engine`), with API keys read from the system keyring.
struct BuiltInEngine;

impl AgentEngineSpawner for BuiltInEngine {
    fn spawn(&self, config: EngineConfig) -> Result<AgentHandle, String> {
        let secrets = Arc::new(twitty_store::SystemSecretStore::new());
        Ok(twitty_ai::engine::spawn(config, secrets))
    }
}

/// Maximum bytes of command output tail sent back to the agent engine (redacted before sending).
const MAX_COMMAND_OUTPUT_TAIL_BYTES: usize = 8 * 1024;

/// Tracks a model-proposed command currently running in a visible terminal pane.
struct RunningAgentCommand {
    session: SessionId,
    step: StepId,
    pane_id: PaneId,
    started_at: Instant,
    cancelled: bool,
}

/// Window-wide Agent panel host owned by `App`.
pub(super) struct AgentPanelHost {
    pub state: AgentPanelState,
    spawner: Box<dyn AgentEngineSpawner>,
    cmd_tx: Option<mpsc::Sender<AgentCommand>>,
    engine_starting: bool,
    engine_generation: u64,
    pending_cmds: Vec<AgentCommand>,
    running_cmd: Option<RunningAgentCommand>,
    waiting_status_pane: Option<PaneId>,
    dragging_divider: bool,
    context_generation: u64,
}

impl Default for AgentPanelHost {
    fn default() -> Self {
        Self::with_spawner(Box::new(BuiltInEngine))
    }
}

impl AgentPanelHost {
    pub fn with_spawner(spawner: Box<dyn AgentEngineSpawner>) -> Self {
        Self {
            state: AgentPanelState::new(),
            spawner,
            cmd_tx: None,
            engine_starting: false,
            engine_generation: 0,
            pending_cmds: Vec::new(),
            running_cmd: None,
            waiting_status_pane: None,
            dragging_divider: false,
            context_generation: 0,
        }
    }
}

impl App {
    /// `agent.panel` (`Ctrl+Shift+.`): toggle the docked Agent side panel.
    /// Review and Agent never show at once: opening one closes the other.
    pub(super) fn toggle_agent_panel(&mut self) {
        if self.agent_panel.state.is_open() {
            self.close_agent_panel();
            return;
        }
        if self.review.is_open() {
            self.close_review();
        }
        self.reset_pointer_gesture();
        if let Some(cwd) = self.focused_id().and_then(|id| self.pane_cwd(id)) {
            self.agent_panel.state.set_project_context(cwd);
        }
        self.agent_panel.state.language = self.window.ui_language();
        self.agent_panel.state.open();
        self.window
            .set_agent_panel_open(true, self.agent_panel.state.custom_width);
        self.apply_layout();
        self.refresh_agent_context();
        self.sync_agent_needs_input();
        self.cursor_blink.reset(Instant::now());
        self.window.request_redraw();
    }

    /// Open Twitty's built-in Agent panel (closing Review if open) and pre-fill the composer with
    /// `draft` for the user to review/edit before submitting.
    pub(super) fn open_agent_panel_with_draft(&mut self, draft: String) {
        if !self.agent_panel.state.is_open() {
            self.toggle_agent_panel();
        }
        self.agent_panel.state.composer.set_text(draft);
        self.agent_panel.state.focus = crate::ui::agent_panel::AgentFocus::Composer;
        self.agent_panel.state.inspector_open = false;
        self.agent_panel.state.edit_mode = None;
        self.cursor_blink.reset(Instant::now());
        self.window.request_redraw();
    }

    pub(super) fn close_agent_panel(&mut self) {
        if !self.agent_panel.state.is_open() {
            return;
        }
        self.agent_panel.state.close();
        self.agent_panel.dragging_divider = false;
        self.window
            .set_agent_panel_open(false, self.agent_panel.state.custom_width);
        self.sync_agent_needs_input();
        self.apply_layout();
        self.refresh_pointer_hover();
    }

    pub(super) fn agent_panel_view(&self) -> Option<AgentPanelView> {
        if !self.agent_panel.state.is_open() {
            return None;
        }
        let rect = self.window.agent_panel_rect(&self.ws)?;
        Some(self.agent_panel.state.view(
            rect,
            self.window.ui_language(),
            self.cursor_blink.visible(),
        ))
    }

    fn agent_panel_layout(&self) -> Option<AgentPanelLayout> {
        let view = self.agent_panel_view()?;
        let metrics = self.window.metrics()?;
        Some(AgentPanelLayout::build(&view, metrics))
    }

    /// True while the Agent panel owns keyboard input.
    pub(super) fn agent_panel_has_keys(&self) -> bool {
        self.agent_panel.state.is_focused() && self.window.agent_panel_rect(&self.ws).is_some()
    }

    /// Start probing `$PATH` and system keyring off the UI thread to build [`EngineConfig`].
    fn start_agent_engine_init(&mut self) {
        if self.agent_panel.engine_starting {
            return;
        }
        self.agent_panel.engine_generation = self.agent_panel.engine_generation.wrapping_add(1);
        let generation = self.agent_panel.engine_generation;
        self.agent_panel.engine_starting = true;
        let app_config = self.config.clone();
        let proxy = self.proxy.clone();
        if let Err(err) = thread::Builder::new()
            .name("twitty-agent-init".into())
            .spawn(move || {
                let detected = crate::ui::settings_ai::detect_cli_agents_on_path();
                let keyring = twitty_store::SystemSecretStore::new();
                let config = crate::ui::settings_ai::engine_config(
                    &app_config,
                    &detected,
                    |account| matches!(keyring.get(account), Ok(Some(v)) if !v.is_empty()),
                );
                let _ = proxy.send_event(AppEvent::AgentEngineConfigReady {
                    generation,
                    config: Box::new(config),
                });
            })
        {
            tracing::warn!("twitty agent init thread failed to start: {err}");
            self.agent_panel.engine_starting = false;
            self.agent_panel.pending_cmds.clear();
        }
    }

    /// Receive the off-thread [`EngineConfig`], spawn the engine, and flush any queued commands.
    pub(super) fn on_agent_engine_config_ready(&mut self, generation: u64, config: EngineConfig) {
        if generation != self.agent_panel.engine_generation {
            return;
        }
        self.agent_panel.engine_starting = false;
        let pending = std::mem::take(&mut self.agent_panel.pending_cmds);
        match self.agent_panel.spawner.spawn(config) {
            Ok(handle) => {
                let proxy = self.proxy.clone();
                let rx = handle.events;
                if let Err(err) = thread::Builder::new()
                    .name("twitty-agent-events".into())
                    .spawn(move || {
                        while let Ok(ev) = rx.recv() {
                            if proxy.send_event(AppEvent::Agent(ev)).is_err() {
                                break;
                            }
                        }
                    })
                {
                    tracing::warn!("twitty agent event forwarder failed to start: {err}");
                    return;
                }
                let tx = handle.commands;
                for cmd in pending {
                    if tx.send(cmd).is_err() {
                        return;
                    }
                }
                self.agent_panel.cmd_tx = Some(tx);
            }
            Err(err) => {
                tracing::warn!("twitty agent engine failed to start: {err}");
            }
        }
    }

    /// Shut down the background agent engine cleanly when the application exits.
    pub(super) fn shutdown_agent_panel(&mut self) {
        self.agent_panel.running_cmd = None;
        self.agent_panel.cmd_tx = None;
        self.agent_panel.engine_starting = false;
        self.agent_panel.engine_generation = self.agent_panel.engine_generation.wrapping_add(1);
        self.agent_panel.pending_cmds.clear();
    }

    /// Settings → AI changed (provider, keys, caps, allow-lists): the next request starts a
    /// fresh engine with the new configuration. A run in progress keeps its engine.
    pub(super) fn restart_agent_engine_when_idle(&mut self) {
        if !self.agent_panel.state.is_running() {
            self.agent_panel.cmd_tx = None;
            self.agent_panel.engine_starting = false;
            self.agent_panel.engine_generation = self.agent_panel.engine_generation.wrapping_add(1);
            self.agent_panel.pending_cmds.clear();
        }
    }

    fn send_agent_command(&mut self, cmd: AgentCommand) {
        if let Some(tx) = &self.agent_panel.cmd_tx {
            match tx.send(cmd) {
                Ok(()) => return,
                Err(mpsc::SendError(unsent)) => {
                    self.agent_panel.cmd_tx = None;
                    self.agent_panel.pending_cmds.push(unsent);
                    self.start_agent_engine_init();
                    return;
                }
            }
        }
        self.agent_panel.pending_cmds.push(cmd);
        self.start_agent_engine_init();
    }

    /// Receive an off-thread assembled [`ContextBundle`] for the Agent panel inspector.
    pub(super) fn on_agent_context_assembled(&mut self, generation: u64, bundle: ContextBundle) {
        if generation != self.agent_panel.context_generation {
            return;
        }
        self.agent_panel.state.set_context_bundle(bundle);
        self.window.request_redraw();
    }

    /// Collect the focused local pane's selected block (or up to 2 recent finished blocks)
    /// for `twitty_ai::context::assemble`.
    fn collect_agent_selected_blocks(&self) -> Vec<SelectedBlock> {
        let Some(id) = self.focused_id() else {
            return Vec::new();
        };
        let Some(pane) = self.ws.pane(id) else {
            return Vec::new();
        };
        if pane.remote.is_some() {
            return Vec::new();
        }
        let ids: Vec<u64> = if let Some(sel) = pane.blocks.selected() {
            vec![sel.id()]
        } else {
            pane.blocks
                .blocks()
                .rev()
                .filter(|b| b.finished.is_some())
                .take(2)
                .map(|b| b.id())
                .collect()
        };
        ids.into_iter()
            .filter_map(|bid| pane.blocks.block_text_by_id(bid, &pane.terminal))
            .map(|bt| SelectedBlock {
                command: bt.command,
                exit_code: bt.exit,
                output: bt.output,
            })
            .collect()
    }

    /// Re-assemble the panel's [`ContextBundle`] off the UI thread using `twitty_ai::context::assemble`,
    /// honoring `config.memory_exclude` and the user's inspector `context_overrides`.
    fn refresh_agent_context(&mut self) {
        self.agent_panel.context_generation = self.agent_panel.context_generation.wrapping_add(1);
        let generation = self.agent_panel.context_generation;
        let cwd = (!self.agent_panel.state.cwd.as_os_str().is_empty())
            .then(|| self.agent_panel.state.cwd.clone());
        let request = self.agent_panel.state.composer.text().to_owned();
        let excludes = self.config.memory_exclude.clone();
        let selected_blocks = self.collect_agent_selected_blocks();
        let overrides = self.agent_panel.state.context_overrides.clone();
        let proxy = self.proxy.clone();
        let _ = thread::Builder::new()
            .name("twitty-agent-context".into())
            .spawn(move || {
                let store = twitty_store::Store::open_default().ok();
                let bundle = twitty_ai::context::assemble(&AssembleInput {
                    cwd: cwd.as_deref(),
                    project_root: None,
                    request: &request,
                    budget_tokens: DEFAULT_BUDGET_TOKENS,
                    memory_exclude: &excludes,
                    store: store.as_ref(),
                    selected_blocks: &selected_blocks,
                    included_overrides: &overrides,
                });
                let _ = proxy.send_event(AppEvent::AgentContext(generation, bundle));
            });
    }

    /// Assemble a fresh [`ContextBundle`] for `AgentCommand::Start` off the UI thread using
    /// `twitty_ai::context::assemble` (including the prompt's FTS5 query and inspector toggles),
    /// update the inspector snapshot, and deliver `Start` to the engine.
    fn send_agent_start_with_assembled_context(
        &mut self,
        session: SessionId,
        request: String,
        cwd: PathBuf,
    ) {
        if self.agent_panel.cmd_tx.is_none() {
            self.start_agent_engine_init();
        }
        self.agent_panel.context_generation = self.agent_panel.context_generation.wrapping_add(1);
        let generation = self.agent_panel.context_generation;
        let cwd_opt = (!cwd.as_os_str().is_empty()).then(|| cwd.clone());
        let excludes = self.config.memory_exclude.clone();
        let selected_blocks = self.collect_agent_selected_blocks();
        let overrides = self.agent_panel.state.context_overrides.clone();
        let proxy = self.proxy.clone();
        if let Err(err) = thread::Builder::new()
            .name("twitty-agent-start-context".into())
            .spawn(move || {
                let store = twitty_store::Store::open_default().ok();
                let context = twitty_ai::context::assemble(&AssembleInput {
                    cwd: cwd_opt.as_deref(),
                    project_root: None,
                    request: &request,
                    budget_tokens: DEFAULT_BUDGET_TOKENS,
                    memory_exclude: &excludes,
                    store: store.as_ref(),
                    selected_blocks: &selected_blocks,
                    included_overrides: &overrides,
                });
                let _ = proxy.send_event(AppEvent::AgentStartReady {
                    generation,
                    session,
                    request,
                    cwd,
                    context,
                });
            })
        {
            tracing::warn!("twitty agent start-context thread failed to start: {err}");
        }
    }

    pub(super) fn on_agent_start_ready(
        &mut self,
        generation: u64,
        session: SessionId,
        request: String,
        cwd: PathBuf,
        context: ContextBundle,
    ) {
        if generation == self.agent_panel.context_generation {
            self.agent_panel.state.set_context_bundle(context.clone());
        }
        self.send_agent_command(AgentCommand::Start {
            session,
            request,
            context,
            cwd,
        });
    }

    /// Receive an [`AgentEvent`] forwarded from the engine thread.
    pub(super) fn on_agent_event(&mut self, event: AgentEvent) {
        self.agent_panel.state.language = self.window.ui_language();
        let action = self.agent_panel.state.apply_event(event);
        self.run_agent_panel_action(action);
        self.sync_agent_needs_input();
        self.window.request_redraw();
    }

    /// Handle a key event while the Agent panel has keyboard focus.
    /// `None` lets Twitty global shortcuts continue; `Some(close)` consumes the key.
    pub(super) fn handle_agent_panel_key(&mut self, event: &KeyEvent) -> Option<bool> {
        if !self.agent_panel_has_keys() {
            return None;
        }
        match key_shortcut_with(event, self.modifiers, &self.keybindings) {
            Some(InputAction::Paste) if self.agent_panel.state.accepts_text_input() => {
                if event.state.is_pressed() {
                    if let Some(text) = self.clipboard.text() {
                        let action = self.agent_panel.state.paste_text(&text);
                        self.run_agent_panel_action(action);
                    }
                }
                return Some(false);
            }
            Some(InputAction::ToggleAgentPanel) => {
                if event.state.is_pressed() {
                    self.close_agent_panel();
                }
                return Some(false);
            }
            Some(InputAction::Bytes(_)) | None => {}
            Some(action) => {
                let blocks = self
                    .focused_id()
                    .and_then(|id| self.ws.pane(id))
                    .is_some_and(|p| p.blocks.navigable(&p.terminal));
                let tabs = self.ws.tabs().len();
                if shortcut_is_idle(Some(&action), self.ws.single_pane(), tabs, blocks) {
                    return Some(false);
                }
                if matches!(
                    action,
                    InputAction::Focus(_)
                        | InputAction::SelectTab(_)
                        | InputAction::NextTab
                        | InputAction::PrevTab
                        | InputAction::NewTab
                        | InputAction::ReopenClosedTab
                        | InputAction::SplitRight
                        | InputAction::SplitDown
                        | InputAction::ClosePane
                        | InputAction::NextAttention
                ) {
                    self.agent_panel.state.set_focused(false);
                }
                return None;
            }
        }
        if !event.state.is_pressed() {
            return Some(false);
        }
        let physical = match event.physical_key {
            PhysicalKey::Code(code) => Some(code),
            _ => None,
        };
        self.agent_panel.state.language = self.window.ui_language();
        let action = self.agent_panel.state.on_key(
            &event.logical_key,
            physical,
            event.text.as_deref(),
            self.modifiers,
        );
        self.run_agent_panel_action(action);
        self.cursor_blink.reset(Instant::now());
        Some(false)
    }

    pub(super) fn agent_panel_ime_preedit(
        &mut self,
        preedit: Option<(String, Option<(usize, usize)>)>,
    ) -> bool {
        if !self.agent_panel_has_keys() {
            return false;
        }
        if self.agent_panel.state.on_ime_preedit(preedit) {
            self.window.request_redraw();
            true
        } else {
            false
        }
    }

    pub(super) fn agent_panel_ime_commit(&mut self, text: &str) -> bool {
        if !self.agent_panel_has_keys() || !self.agent_panel.state.accepts_text_input() {
            return false;
        }
        let action = self.agent_panel.state.on_ime_commit(text);
        self.run_agent_panel_action(action);
        true
    }

    pub(super) fn agent_panel_mouse_move(&mut self, x: f64, y: f64) -> Option<CursorIcon> {
        if !self.agent_panel.state.is_open() {
            return None;
        }
        if self.agent_panel.dragging_divider {
            if let (Some(tab_content), Some(metrics)) = (
                self.window.agent_panel_tab_content_rect(&self.ws),
                self.window.metrics(),
            ) {
                let width = width_from_divider_drag(
                    tab_content,
                    x,
                    metrics.scale,
                    self.window.ui_language(),
                );
                self.agent_panel.state.custom_width = Some(width);
                self.window.set_agent_panel_open(true, Some(width));
                self.apply_layout();
            }
            return Some(CursorIcon::ColResize);
        }
        let layout = self.agent_panel_layout()?;
        let hit = layout.hit(x, y);
        if self.agent_panel.state.hover != hit {
            self.agent_panel.state.hover = hit;
            self.window.request_redraw();
        }
        match hit {
            AgentPanelHit::Outside => None,
            AgentPanelHit::Divider => Some(CursorIcon::ColResize),
            AgentPanelHit::Composer => Some(CursorIcon::Text),
            AgentPanelHit::Close
            | AgentPanelHit::Stop
            | AgentPanelHit::JumpToLatest
            | AgentPanelHit::ExampleChip(_)
            | AgentPanelHit::ContextStrip
            | AgentPanelHit::Send
            | AgentPanelHit::InspectorClose
            | AgentPanelHit::InspectorToggle(_)
            | AgentPanelHit::Card {
                button: Some(_), ..
            } => Some(CursorIcon::Pointer),
            _ => Some(CursorIcon::Default),
        }
    }

    pub(super) fn agent_panel_mouse_release(&mut self, button: MouseButton) -> bool {
        if button == MouseButton::Left && self.agent_panel.dragging_divider {
            self.agent_panel.dragging_divider = false;
            return true;
        }
        false
    }

    pub(super) fn agent_panel_mouse_press(&mut self, button: MouseButton) -> bool {
        if !self.agent_panel.state.is_open() {
            return false;
        }
        let Some(layout) = self.agent_panel_layout() else {
            return false;
        };
        self.agent_panel.state.sync_scroll(layout.max_scroll);
        let (x, y) = self.pointer.pos;
        let hit = layout.hit(x, y);
        if hit == AgentPanelHit::Outside {
            if self.agent_panel.state.is_focused() {
                self.agent_panel.state.set_focused(false);
                self.window.request_redraw();
            }
            return false;
        }
        if button == MouseButton::Left {
            self.agent_panel.state.language = self.window.ui_language();
            let action = self.agent_panel.state.on_click(hit);
            self.run_agent_panel_action(action);
        } else if !self.agent_panel.state.is_focused() {
            self.agent_panel.state.set_focused(true);
            self.window.request_redraw();
        }
        true
    }

    pub(super) fn agent_panel_wheel(&mut self, lines: f64) -> bool {
        let Some(layout) = self.agent_panel_layout() else {
            return false;
        };
        let (x, y) = self.pointer.pos;
        if !layout.panel.contains(x, y) {
            return false;
        }
        self.agent_panel.state.sync_scroll(layout.max_scroll);
        let lh = self
            .window
            .metrics()
            .map_or(20.0, |m| m.line_height / m.scale.max(1.0));
        if self.agent_panel.state.on_wheel(lines, lh) {
            self.window.request_redraw();
        }
        true
    }

    fn run_agent_panel_action(&mut self, action: AgentPanelAction) {
        match action {
            AgentPanelAction::None => return,
            AgentPanelAction::Redraw => {}
            AgentPanelAction::Unfocus => {
                self.agent_panel.state.set_focused(false);
            }
            AgentPanelAction::Close => {
                self.close_agent_panel();
                return;
            }
            AgentPanelAction::StartDividerDrag => {
                self.agent_panel.dragging_divider = true;
                return;
            }
            AgentPanelAction::SendCommand(AgentCommand::Start {
                session,
                request,
                cwd,
                ..
            }) => {
                self.send_agent_start_with_assembled_context(session, request, cwd);
                self.sync_agent_needs_input();
            }
            AgentPanelAction::SendCommand(cmd) => {
                self.send_agent_command(cmd);
                self.sync_agent_needs_input();
            }
            AgentPanelAction::RefreshContext => {
                self.refresh_agent_context();
            }
            AgentPanelAction::CopyText(text) => {
                self.clipboard.set_text(text);
            }
            AgentPanelAction::OpenBlock { pane, block_id } => {
                self.open_agent_command_block(pane, block_id);
            }
            AgentPanelAction::RunInPane {
                session,
                step,
                command,
                cwd,
            } => {
                self.run_agent_command_in_pane(session, step, &command, cwd);
            }
            AgentPanelAction::RememberFact {
                session: _,
                project_root,
                text,
            } => {
                let root = project_root.or_else(|| {
                    (!self.agent_panel.state.cwd.as_os_str().is_empty())
                        .then(|| self.agent_panel.state.cwd.clone())
                });
                if self.remember_fact(root.as_deref(), &text) {
                    self.refresh_agent_context();
                }
            }
        }
        self.window.request_redraw();
    }

    /// Run an approved command in the focused idle pane, or open a new split titled "Agent"
    /// (AGENT.md §1.3).
    fn run_agent_command_in_pane(
        &mut self,
        session: SessionId,
        step: StepId,
        command: &str,
        cwd: PathBuf,
    ) {
        let idle_focused = self
            .focused_id()
            .filter(|&id| self.pane_is_idle_at_prompt(id));
        let pane_id = match idle_focused {
            Some(id) => id,
            None => {
                let options = SpawnOptions {
                    cwd: (!cwd.as_os_str().is_empty()).then_some(cwd.clone()),
                    ..SpawnOptions::default()
                };
                let split = self
                    .split_focused_with(SplitDir::Horizontal, options.clone())
                    .or_else(|_| self.split_focused_with(SplitDir::Vertical, options.clone()));
                match split {
                    Ok(new_id) => {
                        if let Some(pane) = self.ws.pane_mut(new_id) {
                            pane.title = Some("Agent".to_owned());
                        }
                        new_id
                    }
                    Err(_) => match self.new_tab_with(options) {
                        Some(new_id) => {
                            if let Some(pane) = self.ws.pane_mut(new_id) {
                                pane.title = Some("Agent".to_owned());
                            }
                            new_id
                        }
                        None => {
                            self.send_agent_command(AgentCommand::CommandFinished {
                                session,
                                step,
                                exit: Some(1),
                                duration_ms: 0,
                                output: "[could not open a pane for agent command]".to_owned(),
                            });
                            return;
                        }
                    },
                }
            }
        };

        self.agent_panel.running_cmd = Some(RunningAgentCommand {
            session,
            step,
            pane_id,
            started_at: Instant::now(),
            cancelled: false,
        });

        let bracketed = self.mode_of(pane_id).contains(TermMode::BRACKETED_PASTE);
        let bytes = encode_submit(command, bracketed);
        if !bytes.is_empty() {
            self.send_input(pane_id, &bytes);
            self.cursor_blink.reset(Instant::now());
        }
    }

    fn pane_is_idle_at_prompt(&self, id: PaneId) -> bool {
        let Some(pane) = self.ws.pane(id) else {
            return false;
        };
        let alt_screen = pane.mode().contains(TermMode::ALT_SCREEN);
        let running =
            pane.meta.running || pane.pty.as_ref().is_some_and(Pty::has_foreground_program);
        !alt_screen && !running
    }

    /// Called from `on_shell_event` when `MarkKind::CommandEnd` arrives on `pane_id`.
    pub(super) fn agent_panel_command_ended(
        &mut self,
        pane_id: PaneId,
        finished_block: Option<&Block>,
    ) {
        let matches_running = self
            .agent_panel
            .running_cmd
            .as_ref()
            .is_some_and(|r| r.pane_id == pane_id);
        if !matches_running {
            return;
        }
        let Some(running) = self.agent_panel.running_cmd.take() else {
            return;
        };
        let exit = finished_block.and_then(|b| b.exit).or_else(|| {
            if running.cancelled {
                Some(130)
            } else {
                self.ws.pane(pane_id).and_then(|p| p.meta.last_exit)
            }
        });
        let duration_ms = finished_block
            .and_then(Block::duration)
            .unwrap_or_else(|| running.started_at.elapsed())
            .as_millis() as u64;
        let raw_output = finished_block
            .and_then(|block| {
                self.ws
                    .pane(pane_id)
                    .and_then(|p| p.blocks.block_text_by_id(block.id(), &p.terminal))
                    .map(|bt| bt.output)
            })
            .unwrap_or_default();
        let output = bounded_redacted_tail(&raw_output, MAX_COMMAND_OUTPUT_TAIL_BYTES);
        if let Some(block) = finished_block {
            self.agent_panel.state.attach_block_link(
                running.session,
                running.step,
                pane_id,
                block.id(),
            );
        }
        self.send_agent_command(AgentCommand::CommandFinished {
            session: running.session,
            step: running.step,
            exit,
            duration_ms,
            output,
        });
    }

    /// Ctrl+C sent to the pane currently executing an agent command cancels the step (AGENT.md §1.3).
    pub(super) fn agent_panel_on_pane_input(&mut self, pane_id: PaneId, bytes: &[u8]) {
        if bytes != [0x03] {
            return;
        }
        let Some(running) = self.agent_panel.running_cmd.as_mut() else {
            return;
        };
        if running.pane_id == pane_id && !running.cancelled {
            running.cancelled = true;
            let session = running.session;
            let step = running.step;
            let duration_ms = running.started_at.elapsed().as_millis() as u64;
            self.agent_panel.running_cmd = None;
            self.send_agent_command(AgentCommand::CommandFinished {
                session,
                step,
                exit: Some(130),
                duration_ms,
                output: "[cancelled by user via Ctrl+C]".to_owned(),
            });
        }
    }

    /// Notify the agent engine if the pane executing an agent command is closed before
    /// `OSC 133 D` arrives, so the engine never hangs waiting for `CommandFinished`.
    pub(super) fn agent_panel_on_pane_closed(&mut self, pane_id: PaneId) {
        if self.agent_panel.waiting_status_pane == Some(pane_id) {
            self.agent_panel.waiting_status_pane = None;
        }
        let matches_running = self
            .agent_panel
            .running_cmd
            .as_ref()
            .is_some_and(|r| r.pane_id == pane_id);
        if !matches_running {
            return;
        }
        if let Some(running) = self.agent_panel.running_cmd.take() {
            let duration_ms = running.started_at.elapsed().as_millis() as u64;
            self.send_agent_command(AgentCommand::CommandFinished {
                session: running.session,
                step: running.step,
                exit: None,
                duration_ms,
                output: "[pane closed before command finished]".to_owned(),
            });
        }
    }

    fn open_agent_command_block(&mut self, pane_id: PaneId, block_id: u64) {
        let _ = self.focus_pane_by_id(pane_id);
        if let Some(pane) = self.ws.pane_mut(pane_id) {
            pane.blocks.refresh(&pane.terminal);
            if let Some(line) = pane.blocks.block_by_id(block_id).map(|b| b.prompt.line) {
                pane.terminal.scroll_to_line(line);
            }
        }
        self.agent_panel.state.set_focused(false);
        self.window.request_redraw();
    }

    /// Keep pane header and sidebar card "Needs input" status in sync with waiting agent cards.
    pub(super) fn sync_agent_needs_input(&mut self) {
        let waiting = self.agent_panel.state.is_open() && self.agent_panel.state.needs_input();
        if waiting {
            let target = self
                .agent_panel
                .running_cmd
                .as_ref()
                .map(|r| r.pane_id)
                .or_else(|| self.focused_id());
            if self.agent_panel.waiting_status_pane != target {
                if let Some(old) = self.agent_panel.waiting_status_pane.take() {
                    if let Some(pane) = self.ws.pane_mut(old) {
                        pane.meta.clear_status(Some("agent_panel"));
                    }
                }
            }
            if let Some(id) = target {
                if let Some(pane) = self.ws.pane_mut(id) {
                    pane.meta.set_status(
                        "agent_panel",
                        "needs_input",
                        Some("Agent needs approval"),
                        Some("warning"),
                        Some("warn"),
                    );
                }
                self.agent_panel.waiting_status_pane = Some(id);
            }
        } else if let Some(old) = self.agent_panel.waiting_status_pane.take() {
            if let Some(pane) = self.ws.pane_mut(old) {
                pane.meta.clear_status(Some("agent_panel"));
            }
        }
    }
}

/// Keep the trailing `max_bytes` UTF-8 suffix of `raw` and pass it through `redact()`.
fn bounded_redacted_tail(raw: &str, max_bytes: usize) -> String {
    let slice = if raw.len() <= max_bytes {
        raw
    } else {
        let mut start = raw.len() - max_bytes;
        while start < raw.len() && !raw.is_char_boundary(start) {
            start += 1;
        }
        &raw[start..]
    };
    redact(slice).into_owned()
}
