//! Agent panel interactive state machine, keyboard/mouse handling, and event reducer.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use twitty_ai::engine::model::{
    AgentCommand, AgentEvent, ContextBundle, Decision, SessionId, StepId, StepKind, StepStatus,
};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

use super::layout::{AgentPanelHit, AgentPanelView};
use super::session::{AgentSession, CardButton, CardEditMode, StepEntry};
use super::{is_approval_kind, EXAMPLE_CHIPS, MAX_HISTORY};
use crate::composer::editor::TextEditor;
use crate::layout::{PaneId, Rect};
use crate::ui::chrome::UiLanguage;

/// Which region of the agent panel owns keyboard input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentFocus {
    #[default]
    Composer,
    Card,
    Inspector,
}

/// What the panel asks the app to carry out after handling an event or input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentPanelAction {
    None,
    Redraw,
    Close,
    Unfocus,
    SendCommand(AgentCommand),
    CopyText(String),
    OpenBlock {
        pane: PaneId,
        block_id: u64,
    },
    RunInPane {
        session: SessionId,
        step: StepId,
        command: String,
        cwd: PathBuf,
    },
    RememberFact {
        session: SessionId,
        project_root: Option<PathBuf>,
        text: String,
    },
    /// User toggled a context inspector item; ask the app to re-assemble via `twitty_ai::context::assemble`.
    RefreshContext,
    StartDividerDrag,
}

/// Complete window-free state machine for the Agent panel.
#[derive(Clone, Debug)]
pub struct AgentPanelState {
    open: bool,
    focused: bool,
    pub focus: AgentFocus,
    /// User-dragged panel width override in logical pixels (clamped 340..=560).
    pub custom_width: Option<f32>,
    /// Project display name shown in the header.
    pub project_name: String,
    /// Working directory sent on `AgentCommand::Start`.
    pub cwd: PathBuf,
    /// Sessions keyed by id so multiple runs remain addressable (`Arc` for O(1) render snapshots).
    pub sessions: HashMap<SessionId, Arc<AgentSession>>,
    pub active_session: Option<SessionId>,
    next_session_id: u64,
    /// Selected step card in the active session.
    pub selected_step: Option<StepId>,
    /// Focused button index on the selected waiting card (`0` = primary).
    pub focused_button: usize,
    /// Active inline editor for Command or Patch edit mode.
    pub edit_mode: Option<CardEditMode>,
    /// Footer multi-line composer editor (reuses `composer::editor::TextEditor`).
    pub composer: TextEditor,
    /// Prompt history for Up/Down recall in the footer composer.
    history: Vec<String>,
    history_cursor: Option<usize>,
    history_draft: String,
    /// Context bundle provided by lane n21's assembler (displayed and toggled here).
    pub context: ContextBundle,
    /// Per-item `included` overrides chosen in the context inspector (`label -> included`),
    /// passed back to `twitty_ai::context::assemble` so unticking items frees token budget.
    pub context_overrides: Vec<(String, bool)>,
    /// Whether the context inspector sheet is open over the panel.
    pub inspector_open: bool,
    pub inspector_selected: usize,
    /// Timeline vertical scroll offset in logical pixels from top.
    pub scroll_y: f32,
    /// Last computed maximum scroll offset in logical pixels.
    pub max_scroll: f32,
    /// Auto-scroll to newest step while true.
    pub pinned_to_bottom: bool,
    /// Last pointer hover target inside the panel.
    pub hover: AgentPanelHit,
    /// Current UI language for formatting and mirroring.
    pub language: UiLanguage,
}

impl Default for AgentPanelState {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentPanelState {
    pub fn new() -> Self {
        Self {
            open: false,
            focused: false,
            focus: AgentFocus::Composer,
            custom_width: None,
            project_name: String::new(),
            cwd: PathBuf::new(),
            sessions: HashMap::new(),
            active_session: None,
            next_session_id: 1,
            selected_step: None,
            focused_button: 0,
            edit_mode: None,
            composer: TextEditor::new(),
            history: Vec::new(),
            history_cursor: None,
            history_draft: String::new(),
            context: ContextBundle::default(),
            context_overrides: Vec::new(),
            inspector_open: false,
            inspector_selected: 0,
            scroll_y: 0.0,
            max_scroll: 0.0,
            pinned_to_bottom: true,
            hover: AgentPanelHit::Outside,
            language: UiLanguage::English,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn is_focused(&self) -> bool {
        self.open && self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused && self.open;
    }

    pub fn open(&mut self) {
        self.open = true;
        self.focused = true;
        if self.needs_input() {
            self.focus = AgentFocus::Card;
        } else if !self.inspector_open {
            self.focus = AgentFocus::Composer;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.focused = false;
        self.inspector_open = false;
        self.hover = AgentPanelHit::Outside;
    }

    pub fn is_editing_card(&self) -> bool {
        self.edit_mode.is_some()
    }

    /// True when keyboard text / IME input should be routed to an editor in the panel.
    pub fn accepts_text_input(&self) -> bool {
        self.is_focused()
            && (self.edit_mode.is_some()
                || (!self.inspector_open && self.focus == AgentFocus::Composer))
    }

    pub fn active_session(&self) -> Option<&AgentSession> {
        self.active_session
            .and_then(|id| self.sessions.get(&id))
            .map(Arc::as_ref)
    }

    pub fn active_session_mut(&mut self) -> Option<&mut AgentSession> {
        let id = self.active_session?;
        self.sessions.get_mut(&id).map(Arc::make_mut)
    }

    pub fn is_running(&self) -> bool {
        self.active_session().is_some_and(|s| s.running)
    }

    /// True while any card in the active session waits for user approval.
    pub fn needs_input(&self) -> bool {
        self.active_session().is_some_and(AgentSession::needs_input)
    }

    pub fn steps(&self) -> &[StepEntry] {
        self.active_session().map_or(&[], |s| s.steps.as_slice())
    }

    pub fn show_jump_to_latest(&self) -> bool {
        !self.pinned_to_bottom && self.max_scroll > 1.0
    }

    pub fn set_context_bundle(&mut self, bundle: ContextBundle) {
        self.context = bundle;
        if self.inspector_selected >= self.context.items.len() {
            self.inspector_selected = self.context.items.len().saturating_sub(1);
        }
    }

    pub fn set_project_context(&mut self, cwd: PathBuf) {
        if self.cwd != cwd {
            self.context_overrides.clear();
        }
        self.project_name = cwd
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        self.cwd = cwd;
    }

    /// Assemble (or re-assemble) the panel's [`ContextBundle`] via `twitty_ai::context::assemble`,
    /// honoring the user's per-item inspector toggles (`self.context_overrides`).
    pub fn assemble_context(
        &mut self,
        store: Option<&twitty_store::Store>,
        request: &str,
        selected_blocks: &[twitty_ai::context::SelectedBlock],
        memory_exclude: &[String],
        budget_tokens: u32,
    ) -> ContextBundle {
        let cwd = (!self.cwd.as_os_str().is_empty()).then_some(self.cwd.as_path());
        let input = twitty_ai::context::AssembleInput {
            cwd,
            project_root: None,
            request,
            budget_tokens,
            memory_exclude,
            store,
            selected_blocks,
            included_overrides: &self.context_overrides,
        };
        let bundle = twitty_ai::context::assemble(&input);
        self.set_context_bundle(bundle.clone());
        bundle
    }

    /// Associate a terminal command block with a `CommandResult` step so "Open block" works.
    pub fn attach_block_link(
        &mut self,
        session: SessionId,
        step: StepId,
        pane: PaneId,
        block: u64,
    ) {
        if let Some(sess) = self.sessions.get_mut(&session).map(Arc::make_mut) {
            if let Some(entry) = sess.step_mut(step) {
                entry.block_link = Some((pane, block));
                return;
            }
            // If the CommandResult was appended right after the Command step, link the newest
            // CommandResult that lacks a link.
            if let Some(entry) = sess.steps.iter_mut().rev().find(|e| {
                matches!(e.step.kind, StepKind::CommandResult { .. }) && e.block_link.is_none()
            }) {
                entry.block_link = Some((pane, block));
            }
        }
    }

    /// Clear all sessions and return the panel to the empty state (Settings → AI "Clear history").
    pub fn clear_history(&mut self) {
        self.sessions.clear();
        self.active_session = None;
        self.selected_step = None;
        self.focused_button = 0;
        self.edit_mode = None;
        self.scroll_y = 0.0;
        self.max_scroll = 0.0;
        self.pinned_to_bottom = true;
        self.focus = AgentFocus::Composer;
    }

    /// Apply one [`AgentEvent`] from the engine thread and return any follow-up app action.
    pub fn apply_event(&mut self, event: AgentEvent) -> AgentPanelAction {
        match event {
            AgentEvent::Started { session, provider } => {
                let entry = Arc::make_mut(
                    self.sessions
                        .entry(session)
                        .or_insert_with(|| Arc::new(AgentSession::new(session))),
                );
                entry.provider = Some(provider);
                entry.running = true;
                entry.finish_reason = None;
                self.active_session = Some(session);
                AgentPanelAction::Redraw
            }
            AgentEvent::StepAdded { session, step } => {
                let id = step.id;
                let waiting =
                    step.status == StepStatus::WaitingApproval && is_approval_kind(&step.kind);
                let entry = Arc::make_mut(
                    self.sessions
                        .entry(session)
                        .or_insert_with(|| Arc::new(AgentSession::new(session))),
                );
                entry.push_step(StepEntry::new(step));
                self.active_session = Some(session);
                if waiting {
                    self.selected_step = Some(id);
                    self.focused_button = 0;
                    self.edit_mode = None;
                    if !self.inspector_open {
                        self.focus = AgentFocus::Card;
                    }
                } else if self.selected_step.is_none() {
                    self.selected_step = Some(id);
                }
                if self.pinned_to_bottom {
                    self.scroll_y = f32::MAX;
                }
                AgentPanelAction::Redraw
            }
            AgentEvent::StepUpdated { session, step } => {
                let id = step.id;
                let waiting =
                    step.status == StepStatus::WaitingApproval && is_approval_kind(&step.kind);
                let sess = Arc::make_mut(
                    self.sessions
                        .entry(session)
                        .or_insert_with(|| Arc::new(AgentSession::new(session))),
                );
                if let Some(existing) = sess.step_mut(id) {
                    existing.set_step(step);
                } else {
                    sess.push_step(StepEntry::new(step));
                }
                if self.edit_mode.as_ref().is_some_and(|e| e.step_id() == id) && !waiting {
                    self.edit_mode = None;
                }
                if waiting && self.active_session == Some(session) {
                    self.selected_step = Some(id);
                    self.focused_button = 0;
                    if !self.inspector_open {
                        self.focus = AgentFocus::Card;
                    }
                }
                if self.pinned_to_bottom {
                    self.scroll_y = f32::MAX;
                }
                AgentPanelAction::Redraw
            }
            AgentEvent::RunInPane {
                session,
                step,
                command,
                cwd,
            } => AgentPanelAction::RunInPane {
                session,
                step,
                command,
                cwd,
            },
            AgentEvent::RememberFact {
                session,
                project_root,
                text,
            } => AgentPanelAction::RememberFact {
                session,
                project_root,
                text,
            },
            AgentEvent::Usage { session, usage } => {
                let sess = Arc::make_mut(
                    self.sessions
                        .entry(session)
                        .or_insert_with(|| Arc::new(AgentSession::new(session))),
                );
                sess.usage = usage;
                AgentPanelAction::Redraw
            }
            AgentEvent::Finished { session, reason } => {
                if let Some(sess) = self.sessions.get_mut(&session).map(Arc::make_mut) {
                    sess.running = false;
                    sess.finish_reason = Some(reason);
                }
                if self.active_session == Some(session) && !self.needs_input() {
                    self.edit_mode = None;
                    if self.focus == AgentFocus::Card {
                        self.focus = AgentFocus::Composer;
                    }
                }
                AgentPanelAction::Redraw
            }
        }
    }

    /// Submit text from the footer composer or an example chip.
    pub fn submit_prompt(&mut self, text: String) -> AgentPanelAction {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return AgentPanelAction::None;
        }
        let prompt = trimmed.to_owned();
        if self.history.last().map(String::as_str) != Some(prompt.as_str()) {
            self.history.push(prompt.clone());
            if self.history.len() > MAX_HISTORY {
                self.history.remove(0);
            }
        }
        self.history_cursor = None;
        self.history_draft.clear();
        self.composer.clear();
        self.pinned_to_bottom = true;
        self.scroll_y = f32::MAX;

        if let Some(session) = self.active_session {
            if let Some(sess) = self.sessions.get_mut(&session).map(Arc::make_mut) {
                sess.running = true;
                sess.finish_reason = None;
            }
            AgentPanelAction::SendCommand(AgentCommand::Reply {
                session,
                text: prompt,
            })
        } else {
            let session = SessionId(self.next_session_id);
            self.next_session_id += 1;
            self.sessions
                .insert(session, Arc::new(AgentSession::new(session)));
            self.active_session = Some(session);
            AgentPanelAction::SendCommand(AgentCommand::Start {
                session,
                request: prompt,
                context: self.context.clone(),
                cwd: self.cwd.clone(),
            })
        }
    }

    /// Trigger the Stop button in the header.
    pub fn stop_run(&mut self) -> AgentPanelAction {
        let Some(session) = self.active_session else {
            return AgentPanelAction::None;
        };
        if !self.is_running() {
            return AgentPanelAction::None;
        }
        self.edit_mode = None;
        AgentPanelAction::SendCommand(AgentCommand::Stop { session })
    }

    /// Jump scroll back to the latest step and re-enable auto-scroll.
    pub fn jump_to_latest(&mut self) -> AgentPanelAction {
        self.pinned_to_bottom = true;
        self.scroll_y = self.max_scroll;
        AgentPanelAction::Redraw
    }

    /// Toggle the context inspector sheet.
    pub fn toggle_inspector(&mut self) -> AgentPanelAction {
        self.inspector_open = !self.inspector_open;
        if self.inspector_open {
            self.focus = AgentFocus::Inspector;
            self.inspector_selected = self
                .inspector_selected
                .min(self.context.items.len().saturating_sub(1));
        } else {
            self.focus = if self.needs_input() {
                AgentFocus::Card
            } else {
                AgentFocus::Composer
            };
        }
        AgentPanelAction::Redraw
    }

    /// Toggle whether `index` in `context.items` is included in the next request.
    ///
    /// Records the override in `context_overrides` and returns [`AgentPanelAction::RefreshContext`]
    /// so the app re-assembles through `twitty_ai::context::assemble` (freeing budget for lower-
    /// priority memories or longer block tails).
    pub fn toggle_context_item(&mut self, index: usize) -> AgentPanelAction {
        if let Some(item) = self.context.items.get_mut(index) {
            item.included = !item.included;
            let label = item.label.clone();
            let included = item.included;
            if let Some(entry) = self.context_overrides.iter_mut().find(|(l, _)| *l == label) {
                entry.1 = included;
            } else {
                self.context_overrides.push((label, included));
            }
            self.inspector_selected = index;
            AgentPanelAction::RefreshContext
        } else {
            AgentPanelAction::None
        }
    }

    /// Enter inline edit mode for a waiting Command or Patch step.
    pub fn begin_edit(&mut self, step_id: StepId) -> AgentPanelAction {
        let initial = self.active_session().and_then(|sess| {
            let entry = sess.step(step_id)?;
            if entry.step.status != StepStatus::WaitingApproval {
                return None;
            }
            match &entry.step.kind {
                StepKind::Command { command, .. } => Some((false, command.clone())),
                StepKind::Patch { diff, .. } => Some((true, diff.clone())),
                _ => None,
            }
        });
        let Some((is_patch, initial_text)) = initial else {
            return AgentPanelAction::None;
        };
        self.selected_step = Some(step_id);
        self.focus = AgentFocus::Card;
        self.edit_mode = Some(if is_patch {
            CardEditMode::Patch {
                step: step_id,
                editor: TextEditor::with_text(&initial_text),
            }
        } else {
            CardEditMode::Command {
                step: step_id,
                editor: TextEditor::with_text(&initial_text),
            }
        });
        AgentPanelAction::Redraw
    }

    /// Submit the active inline card edit as `Decision::ApproveEdited`.
    pub fn submit_edit(&mut self) -> AgentPanelAction {
        let Some(session) = self.active_session else {
            return AgentPanelAction::None;
        };
        let Some(edit) = self.edit_mode.take() else {
            return AgentPanelAction::None;
        };
        let step = edit.step_id();
        let text = edit.editor().text().to_owned();
        if text.trim().is_empty() {
            // Re-open edit mode rather than sending an empty edited command/patch.
            self.edit_mode = Some(edit);
            return AgentPanelAction::Redraw;
        }
        if let Some(sess) = self.sessions.get_mut(&session).map(Arc::make_mut) {
            if let Some(entry) = sess.step_mut(step) {
                entry.user_edited = true;
                match &mut entry.step.kind {
                    StepKind::Command { command, .. } => *command = text.clone(),
                    StepKind::Patch { diff, .. } => *diff = text.clone(),
                    _ => {}
                }
            }
        }
        AgentPanelAction::SendCommand(AgentCommand::Decide {
            session,
            step,
            decision: Decision::ApproveEdited(text),
        })
    }

    /// Cancel inline edit mode without deciding.
    pub fn cancel_edit(&mut self) -> AgentPanelAction {
        if self.edit_mode.take().is_some() {
            self.focused_button = 0;
            AgentPanelAction::Redraw
        } else {
            AgentPanelAction::None
        }
    }

    /// Send an approval or denial decision for `step_id`.
    pub fn decide(&mut self, step_id: StepId, decision: Decision) -> AgentPanelAction {
        let Some(session) = self.active_session else {
            return AgentPanelAction::None;
        };
        let is_waiting = self
            .active_session()
            .and_then(|s| s.step(step_id))
            .is_some_and(StepEntry::is_waiting);
        if !is_waiting {
            return AgentPanelAction::None;
        }
        if self
            .edit_mode
            .as_ref()
            .is_some_and(|e| e.step_id() == step_id)
        {
            self.edit_mode = None;
        }
        self.focused_button = 0;
        AgentPanelAction::SendCommand(AgentCommand::Decide {
            session,
            step: step_id,
            decision,
        })
    }

    /// Execute a specific button on `step_id`.
    pub fn activate_card_button(
        &mut self,
        step_id: StepId,
        button: CardButton,
    ) -> AgentPanelAction {
        self.selected_step = Some(step_id);
        match button {
            CardButton::Primary => self.decide(step_id, Decision::Approve),
            CardButton::Edit => self.begin_edit(step_id),
            CardButton::Deny => self.decide(step_id, Decision::Deny),
            CardButton::ApplyEdit => self.submit_edit(),
            CardButton::CancelEdit => self.cancel_edit(),
            CardButton::Expand => {
                if let Some(sess) = self.active_session_mut() {
                    if let Some(entry) = sess.step_mut(step_id) {
                        entry.expanded = !entry.expanded;
                        return AgentPanelAction::Redraw;
                    }
                }
                AgentPanelAction::None
            }
            CardButton::OpenBlock => {
                if let Some((pane, block_id)) = self
                    .active_session()
                    .and_then(|s| s.step(step_id))
                    .and_then(|e| e.block_link)
                {
                    AgentPanelAction::OpenBlock { pane, block_id }
                } else {
                    AgentPanelAction::None
                }
            }
            CardButton::Undo => {
                let Some(session) = self.active_session else {
                    return AgentPanelAction::None;
                };
                let available = self
                    .active_session()
                    .and_then(|s| s.step(step_id))
                    .is_some_and(|e| {
                        matches!(
                            e.step.kind,
                            StepKind::PatchApplied {
                                undo_available: true,
                                ..
                            }
                        )
                    });
                if available {
                    AgentPanelAction::SendCommand(AgentCommand::Undo {
                        session,
                        step: step_id,
                    })
                } else {
                    AgentPanelAction::None
                }
            }
            CardButton::Copy => {
                let text = self
                    .active_session()
                    .and_then(|s| s.step(step_id))
                    .and_then(|e| match &e.step.kind {
                        StepKind::Message { text } => Some(text.clone()),
                        StepKind::Command { command, .. } => Some(command.clone()),
                        _ => None,
                    });
                match text {
                    Some(text) => AgentPanelAction::CopyText(text),
                    None => AgentPanelAction::None,
                }
            }
        }
    }

    // ---- Keyboard & IME handling ----

    /// Handle one key press while the agent panel has focus.
    pub fn on_key(
        &mut self,
        logical_key: &Key,
        physical: Option<KeyCode>,
        text: Option<&str>,
        modifiers: ModifiersState,
    ) -> AgentPanelAction {
        if self.inspector_open {
            return self.on_inspector_key(logical_key);
        }
        if self.edit_mode.is_some() {
            return self.on_card_edit_key(logical_key, text, modifiers);
        }
        if self.focus == AgentFocus::Card {
            return self.on_card_key(logical_key, physical, modifiers);
        }
        self.on_composer_key(logical_key, text, modifiers)
    }

    /// Commit IME text (e.g. Arabic) into the active editor.
    pub fn on_ime_commit(&mut self, text: &str) -> AgentPanelAction {
        if text.is_empty() {
            return AgentPanelAction::None;
        }
        if let Some(edit) = &mut self.edit_mode {
            edit.editor_mut().set_preedit(None);
            edit.editor_mut().insert_text(text);
            return AgentPanelAction::Redraw;
        }
        if !self.inspector_open && self.focus == AgentFocus::Composer {
            self.composer.set_preedit(None);
            self.composer.insert_text(text);
            return AgentPanelAction::Redraw;
        }
        AgentPanelAction::None
    }

    /// Update inline IME preedit in the active editor.
    pub fn on_ime_preedit(&mut self, preedit: Option<(String, Option<(usize, usize)>)>) -> bool {
        if let Some(edit) = &mut self.edit_mode {
            edit.editor_mut().set_preedit(preedit);
            return true;
        }
        if self.is_focused() && !self.inspector_open && self.focus == AgentFocus::Composer {
            self.composer.set_preedit(preedit);
            return true;
        }
        false
    }

    /// Paste clipboard text into whichever editor is active.
    pub fn paste_text(&mut self, text: &str) -> AgentPanelAction {
        if text.is_empty() {
            return AgentPanelAction::None;
        }
        if let Some(edit) = &mut self.edit_mode {
            edit.editor_mut().insert_text(text);
            return AgentPanelAction::Redraw;
        }
        if !self.inspector_open {
            self.focus = AgentFocus::Composer;
            self.composer.insert_text(text);
            return AgentPanelAction::Redraw;
        }
        AgentPanelAction::None
    }

    fn on_inspector_key(&mut self, logical_key: &Key) -> AgentPanelAction {
        match logical_key {
            Key::Named(NamedKey::Escape) => self.toggle_inspector(),
            Key::Named(NamedKey::ArrowUp) => {
                if self.inspector_selected > 0 {
                    self.inspector_selected -= 1;
                    AgentPanelAction::Redraw
                } else {
                    AgentPanelAction::None
                }
            }
            Key::Named(NamedKey::ArrowDown) => {
                if self.inspector_selected + 1 < self.context.items.len() {
                    self.inspector_selected += 1;
                    AgentPanelAction::Redraw
                } else {
                    AgentPanelAction::None
                }
            }
            Key::Named(NamedKey::Space | NamedKey::Enter) => {
                self.toggle_context_item(self.inspector_selected)
            }
            _ => AgentPanelAction::None,
        }
    }

    fn on_card_edit_key(
        &mut self,
        logical_key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
    ) -> AgentPanelAction {
        let is_patch = self.edit_mode.as_ref().is_some_and(CardEditMode::is_patch);
        match logical_key {
            Key::Named(NamedKey::Escape) => return self.cancel_edit(),
            Key::Named(NamedKey::Enter) => {
                if is_patch {
                    if modifiers.control_key() {
                        return self.submit_edit();
                    }
                    if let Some(edit) = &mut self.edit_mode {
                        edit.editor_mut().insert_newline();
                        return AgentPanelAction::Redraw;
                    }
                } else if modifiers.shift_key() {
                    if let Some(edit) = &mut self.edit_mode {
                        edit.editor_mut().insert_newline();
                        return AgentPanelAction::Redraw;
                    }
                } else {
                    return self.submit_edit();
                }
            }
            _ => {}
        }
        if let Some(edit) = &mut self.edit_mode {
            if apply_editor_key(edit.editor_mut(), logical_key, text, modifiers) {
                return AgentPanelAction::Redraw;
            }
        }
        AgentPanelAction::None
    }

    fn on_card_key(
        &mut self,
        logical_key: &Key,
        physical: Option<KeyCode>,
        modifiers: ModifiersState,
    ) -> AgentPanelAction {
        let active_step = self.selected_or_waiting_step_id();
        let waiting_entry = active_step.and_then(|id| {
            self.active_session()
                .and_then(|s| s.step(id))
                .filter(|e| e.is_waiting())
                .cloned()
        });

        if let Some(entry) = waiting_entry {
            let step_id = entry.step.id;
            let buttons = entry.waiting_buttons();
            match logical_key {
                Key::Named(NamedKey::Enter) if modifiers.is_empty() => {
                    let btn = buttons
                        .get(self.focused_button)
                        .copied()
                        .unwrap_or(CardButton::Primary);
                    return self.activate_card_button(step_id, btn);
                }
                Key::Named(NamedKey::Space) if modifiers.is_empty() => {
                    if let Some(&btn) = buttons.get(self.focused_button) {
                        return self.activate_card_button(step_id, btn);
                    }
                }
                Key::Named(NamedKey::Tab) => {
                    if !buttons.is_empty() {
                        if modifiers.shift_key() {
                            self.focused_button =
                                (self.focused_button + buttons.len() - 1) % buttons.len();
                        } else {
                            self.focused_button = (self.focused_button + 1) % buttons.len();
                        }
                        return AgentPanelAction::Redraw;
                    }
                }
                Key::Named(NamedKey::Escape) => {
                    return self.decide(step_id, Decision::Deny);
                }
                _ => {}
            }
            if modifiers.is_empty() || modifiers == ModifiersState::SHIFT {
                let letter = key_ascii_letter(logical_key, physical);
                match letter {
                    Some('e')
                        if matches!(
                            entry.step.kind,
                            StepKind::Command { .. } | StepKind::Patch { .. }
                        ) =>
                    {
                        return self.begin_edit(step_id);
                    }
                    Some('d') => {
                        return self.decide(step_id, Decision::Deny);
                    }
                    _ => {}
                }
            }
        }

        // General timeline navigation while focused on cards.
        match logical_key {
            Key::Named(NamedKey::ArrowUp) => self.move_selection(-1),
            Key::Named(NamedKey::ArrowDown) => self.move_selection(1),
            Key::Named(NamedKey::End) => self.jump_to_latest(),
            Key::Named(NamedKey::Tab) => {
                self.focus = AgentFocus::Composer;
                AgentPanelAction::Redraw
            }
            Key::Named(NamedKey::Escape) => {
                self.focus = AgentFocus::Composer;
                AgentPanelAction::Unfocus
            }
            Key::Named(NamedKey::Space | NamedKey::Enter) => {
                if let Some(step_id) = active_step {
                    if let Some(btn) = self
                        .active_session()
                        .and_then(|s| s.step(step_id))
                        .and_then(|e| e.buttons(false).first().copied())
                    {
                        return self.activate_card_button(step_id, btn);
                    }
                }
                AgentPanelAction::None
            }
            _ => AgentPanelAction::None,
        }
    }

    fn on_composer_key(
        &mut self,
        logical_key: &Key,
        text: Option<&str>,
        modifiers: ModifiersState,
    ) -> AgentPanelAction {
        match logical_key {
            Key::Named(NamedKey::Escape) => {
                if !self.composer.is_empty() {
                    self.composer.clear();
                    return AgentPanelAction::Redraw;
                }
                if self.needs_input() {
                    self.focus = AgentFocus::Card;
                    return AgentPanelAction::Redraw;
                }
                return AgentPanelAction::Unfocus;
            }
            Key::Named(NamedKey::Tab) if modifiers.is_empty() && self.composer.is_empty() => {
                if !self.steps().is_empty() {
                    self.focus = AgentFocus::Card;
                    if self.selected_step.is_none() {
                        self.selected_step = self.steps().last().map(|s| s.step.id);
                    }
                    return AgentPanelAction::Redraw;
                }
            }
            Key::Named(NamedKey::Enter) => {
                if modifiers.shift_key() {
                    self.composer.insert_newline();
                    return AgentPanelAction::Redraw;
                }
                let prompt = self.composer.text().to_owned();
                return self.submit_prompt(prompt);
            }
            Key::Named(NamedKey::ArrowUp)
                if modifiers.is_empty() && self.composer.is_on_first_line() =>
            {
                if self.history_prev() {
                    return AgentPanelAction::Redraw;
                }
                if self.composer.is_empty() && !self.steps().is_empty() {
                    self.focus = AgentFocus::Card;
                    self.selected_step = self.steps().last().map(|s| s.step.id);
                    return AgentPanelAction::Redraw;
                }
            }
            Key::Named(NamedKey::ArrowDown)
                if modifiers.is_empty()
                    && self.composer.is_on_last_line()
                    && self.history_cursor.is_some()
                    && self.history_next() =>
            {
                return AgentPanelAction::Redraw;
            }
            _ => {}
        }
        if apply_editor_key(&mut self.composer, logical_key, text, modifiers) {
            self.history_cursor = None;
            return AgentPanelAction::Redraw;
        }
        AgentPanelAction::None
    }

    fn selected_or_waiting_step_id(&self) -> Option<StepId> {
        let sess = self.active_session()?;
        if let Some(id) = self.selected_step {
            if sess.step(id).is_some() {
                return Some(id);
            }
        }
        sess.waiting_step()
            .or_else(|| sess.steps.last())
            .map(|e| e.step.id)
    }

    fn move_selection(&mut self, delta: isize) -> AgentPanelAction {
        let Some(sess) = self.active_session() else {
            return AgentPanelAction::None;
        };
        if sess.steps.is_empty() {
            return AgentPanelAction::None;
        }
        let cur = self
            .selected_step
            .and_then(|id| sess.steps.iter().position(|e| e.step.id == id))
            .unwrap_or(sess.steps.len().saturating_sub(1));
        let next = (cur as isize + delta).clamp(0, (sess.steps.len() - 1) as isize) as usize;
        self.selected_step = Some(sess.steps[next].step.id);
        self.focused_button = 0;
        AgentPanelAction::Redraw
    }

    fn history_prev(&mut self) -> bool {
        if self.history.is_empty() {
            return false;
        }
        let next_idx = match self.history_cursor {
            None => {
                self.history_draft = self.composer.text().to_owned();
                self.history.len() - 1
            }
            Some(0) => return false,
            Some(i) => i - 1,
        };
        self.history_cursor = Some(next_idx);
        self.composer.set_text(self.history[next_idx].clone());
        true
    }

    fn history_next(&mut self) -> bool {
        let Some(cur) = self.history_cursor else {
            return false;
        };
        if cur + 1 < self.history.len() {
            let next_idx = cur + 1;
            self.history_cursor = Some(next_idx);
            self.composer.set_text(self.history[next_idx].clone());
        } else {
            self.history_cursor = None;
            let draft = std::mem::take(&mut self.history_draft);
            self.composer.set_text(draft);
        }
        true
    }

    // ---- Pointer & wheel handling ----

    /// Scroll the body timeline by `lines` wheel notches (positive = scroll up toward older steps,
    /// matching Twitty's `wheel_lines` convention).
    pub fn on_wheel(&mut self, lines: f64, line_height_logical: f32) -> bool {
        if self.inspector_open || self.max_scroll <= 0.0 {
            return false;
        }
        let step = (lines as f32) * line_height_logical.max(16.0) * 1.5;
        let old = self.scroll_y.clamp(0.0, self.max_scroll);
        let next = (old - step).clamp(0.0, self.max_scroll);
        self.scroll_y = next;
        self.pinned_to_bottom = (self.max_scroll - next) <= 2.0;
        (next - old).abs() > 0.25
    }

    /// Update scroll limits from the latest computed layout.
    pub fn sync_scroll(&mut self, max_scroll: f32) {
        self.max_scroll = max_scroll.max(0.0);
        if self.pinned_to_bottom {
            self.scroll_y = self.max_scroll;
        } else {
            self.scroll_y = self.scroll_y.clamp(0.0, self.max_scroll);
        }
    }

    /// Handle a left click on `hit`.
    pub fn on_click(&mut self, hit: AgentPanelHit) -> AgentPanelAction {
        match hit {
            AgentPanelHit::Outside => {
                if self.focused {
                    self.focused = false;
                    return AgentPanelAction::Unfocus;
                }
                AgentPanelAction::None
            }
            AgentPanelHit::Divider => {
                self.focused = true;
                AgentPanelAction::StartDividerDrag
            }
            AgentPanelHit::Close => AgentPanelAction::Close,
            AgentPanelHit::Stop => self.stop_run(),
            AgentPanelHit::JumpToLatest => self.jump_to_latest(),
            AgentPanelHit::ContextStrip => {
                self.focused = true;
                self.toggle_inspector()
            }
            AgentPanelHit::InspectorClose | AgentPanelHit::InspectorBackdrop => {
                if self.inspector_open {
                    self.toggle_inspector()
                } else {
                    AgentPanelAction::None
                }
            }
            AgentPanelHit::InspectorToggle(idx) => {
                self.focused = true;
                self.toggle_context_item(idx)
            }
            AgentPanelHit::ExampleChip(idx) => {
                self.focused = true;
                if let Some(chip) = EXAMPLE_CHIPS.get(idx) {
                    let prompt = chip.label(self.language).to_owned();
                    self.submit_prompt(prompt)
                } else {
                    AgentPanelAction::None
                }
            }
            AgentPanelHit::Composer => {
                self.focused = true;
                self.inspector_open = false;
                self.focus = AgentFocus::Composer;
                AgentPanelAction::Redraw
            }
            AgentPanelHit::Send => {
                self.focused = true;
                let prompt = self.composer.text().to_owned();
                self.submit_prompt(prompt)
            }
            AgentPanelHit::Card { step, button } => {
                self.focused = true;
                self.inspector_open = false;
                self.focus = AgentFocus::Card;
                self.selected_step = Some(step);
                self.focused_button = 0;
                match button {
                    Some(btn) => self.activate_card_button(step, btn),
                    None => AgentPanelAction::Redraw,
                }
            }
            AgentPanelHit::Inside => {
                self.focused = true;
                AgentPanelAction::Redraw
            }
        }
    }

    /// Build a snapshot view for rendering without borrowing `App`.
    /// Clones the active [`AgentSession`] via `Arc::clone` in O(1) instead of deep-cloning steps.
    pub fn view(&self, rect: Rect, language: UiLanguage, cursor_blink_on: bool) -> AgentPanelView {
        let session = self
            .active_session
            .and_then(|id| self.sessions.get(&id))
            .cloned();
        AgentPanelView {
            rect,
            language,
            focused: self.focused,
            focus: self.focus,
            cursor_blink_on,
            project_name: self.project_name.clone(),
            session,
            selected_step: self.selected_step,
            focused_button: self.focused_button,
            edit_mode: self.edit_mode.clone(),
            composer: self.composer.clone(),
            context: self.context.clone(),
            inspector_open: self.inspector_open,
            inspector_selected: self.inspector_selected,
            scroll_y: self.scroll_y,
            pinned_to_bottom: self.pinned_to_bottom,
            hover: self.hover,
        }
    }
}

fn key_ascii_letter(logical_key: &Key, physical: Option<KeyCode>) -> Option<char> {
    if let Key::Character(ch) = logical_key {
        let mut chars = ch.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_alphabetic() {
                return Some(c.to_ascii_lowercase());
            }
        }
    }
    match physical {
        Some(KeyCode::KeyE) => Some('e'),
        Some(KeyCode::KeyD) => Some('d'),
        _ => None,
    }
}

/// Apply standard editing keys to a [`TextEditor`] (shared by footer composer & card editors).
fn apply_editor_key(
    editor: &mut TextEditor,
    logical_key: &Key,
    text: Option<&str>,
    modifiers: ModifiersState,
) -> bool {
    let ctrl = modifiers.control_key();
    let shift = modifiers.shift_key();
    let alt = modifiers.alt_key();
    match logical_key {
        Key::Named(NamedKey::Backspace) => {
            if ctrl || alt {
                editor.delete_word_backward();
            } else {
                editor.backspace();
            }
            true
        }
        Key::Named(NamedKey::Delete) => {
            if ctrl || alt {
                editor.delete_word_forward();
            } else {
                editor.delete_forward();
            }
            true
        }
        Key::Named(NamedKey::ArrowLeft) => {
            if ctrl || alt {
                editor.move_word_visual_left(shift);
            } else {
                editor.move_visual_left(shift);
            }
            true
        }
        Key::Named(NamedKey::ArrowRight) => {
            if ctrl || alt {
                editor.move_word_visual_right(shift);
            } else {
                editor.move_visual_right(shift);
            }
            true
        }
        Key::Named(NamedKey::ArrowUp) => {
            editor.move_up(shift);
            true
        }
        Key::Named(NamedKey::ArrowDown) => {
            editor.move_down(shift);
            true
        }
        Key::Named(NamedKey::Home) => {
            editor.move_home(ctrl, shift);
            true
        }
        Key::Named(NamedKey::End) => {
            editor.move_end(ctrl, shift);
            true
        }
        Key::Character(ch) if ctrl && !alt => match ch.to_ascii_lowercase().as_str() {
            "a" => {
                editor.select_all();
                true
            }
            "z" if shift => {
                editor.redo();
                true
            }
            "z" => {
                editor.undo();
                true
            }
            "y" => {
                editor.redo();
                true
            }
            _ => false,
        },
        Key::Named(NamedKey::Space) if !ctrl && !alt => {
            editor.insert_text(" ");
            true
        }
        _ => {
            if ctrl || alt {
                return false;
            }
            let payload = text.or(match logical_key {
                Key::Character(s) => Some(s.as_str()),
                _ => None,
            });
            if let Some(s) = payload {
                if !s.is_empty() && !s.chars().all(char::is_control) {
                    editor.insert_text(s);
                    return true;
                }
            }
            false
        }
    }
}
