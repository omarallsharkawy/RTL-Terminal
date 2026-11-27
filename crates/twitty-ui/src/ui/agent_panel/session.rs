//! Step card, inline edit mode, and session state for the Agent panel.

use twitty_ai::engine::model::{
    FinishReason, ProviderInfo, SessionId, Step, StepId, StepKind, StepStatus, Usage,
};

use super::{is_approval_kind, PLAN_COLLAPSED_LINES};
use crate::composer::editor::TextEditor;
use crate::layout::PaneId;
use crate::review::diff::{parse as parse_diff, Limits as DiffLimits};

// ---- Card & session state ----

/// Interactive buttons that can appear on a step card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CardButton {
    /// Run (Command), Apply (Patch), Remember (FactSuggestion), Continue (LimitReached).
    Primary,
    /// Edit Command or Patch inline.
    Edit,
    /// Deny (Command/Patch), Dismiss (FactSuggestion), Stop (LimitReached).
    Deny,
    /// Expand or collapse Read preview / Patch diff / long Plan.
    Expand,
    /// Jump to the terminal block where a command ran.
    OpenBlock,
    /// Undo an applied patch.
    Undo,
    /// Copy a Message card's text.
    Copy,
    /// Submit an inline edit (Command or Patch edit mode).
    ApplyEdit,
    /// Cancel an inline edit.
    CancelEdit,
}

/// Active inline editor on a waiting Command or Patch card.
#[derive(Clone, Debug)]
pub enum CardEditMode {
    Command { step: StepId, editor: TextEditor },
    Patch { step: StepId, editor: TextEditor },
}

impl CardEditMode {
    pub fn step_id(&self) -> StepId {
        match self {
            Self::Command { step, .. } | Self::Patch { step, .. } => *step,
        }
    }

    pub fn editor(&self) -> &TextEditor {
        match self {
            Self::Command { editor, .. } | Self::Patch { editor, .. } => editor,
        }
    }

    pub fn editor_mut(&mut self) -> &mut TextEditor {
        match self {
            Self::Command { editor, .. } | Self::Patch { editor, .. } => editor,
        }
    }

    pub fn is_patch(&self) -> bool {
        matches!(self, Self::Patch { .. })
    }
}

/// One step in a session's timeline plus its per-card UI state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StepEntry {
    pub step: Step,
    pub expanded: bool,
    pub block_link: Option<(PaneId, u64)>,
    pub user_edited: bool,
    /// Pre-parsed unified diff for `StepKind::Patch`, shared via `Arc` so layout and drawing
    /// avoid re-parsing on every frame.
    pub parsed_diff: Option<std::sync::Arc<crate::review::diff::Diff>>,
}

pub(super) fn parse_patch_diff_cached(
    kind: &StepKind,
) -> Option<std::sync::Arc<crate::review::diff::Diff>> {
    match kind {
        StepKind::Patch { diff, .. } => Some(std::sync::Arc::new(parse_diff(
            diff.as_bytes(),
            false,
            &DiffLimits::default(),
        ))),
        _ => None,
    }
}

impl StepEntry {
    pub fn new(step: Step) -> Self {
        let parsed_diff = parse_patch_diff_cached(&step.kind);
        Self {
            step,
            expanded: false,
            block_link: None,
            user_edited: false,
            parsed_diff,
        }
    }

    pub fn set_step(&mut self, step: Step) {
        self.parsed_diff = parse_patch_diff_cached(&step.kind);
        self.step = step;
    }

    /// True if this card is currently waiting for user approval.
    pub fn is_waiting(&self) -> bool {
        self.step.status == StepStatus::WaitingApproval && is_approval_kind(&self.step.kind)
    }

    /// Buttons currently offered by this card, in keyboard Tab order.
    pub fn buttons(&self, editing_this: bool) -> Vec<CardButton> {
        if editing_this {
            return vec![CardButton::ApplyEdit, CardButton::CancelEdit];
        }
        match &self.step.kind {
            StepKind::Plan { text } => {
                if text.lines().count() > PLAN_COLLAPSED_LINES {
                    vec![CardButton::Expand]
                } else {
                    Vec::new()
                }
            }
            StepKind::Read { .. } => vec![CardButton::Expand],
            StepKind::Command { .. } => {
                if self.step.status == StepStatus::WaitingApproval {
                    vec![CardButton::Primary, CardButton::Edit, CardButton::Deny]
                } else {
                    Vec::new()
                }
            }
            StepKind::Patch { .. } => {
                let mut btns = Vec::new();
                if self.step.status == StepStatus::WaitingApproval {
                    btns.push(CardButton::Primary);
                    btns.push(CardButton::Edit);
                    btns.push(CardButton::Deny);
                }
                btns.push(CardButton::Expand);
                btns
            }
            StepKind::CommandResult { .. } => {
                if self.block_link.is_some() {
                    vec![CardButton::OpenBlock]
                } else {
                    Vec::new()
                }
            }
            StepKind::PatchApplied { undo_available, .. } => {
                if *undo_available {
                    vec![CardButton::Undo]
                } else {
                    Vec::new()
                }
            }
            StepKind::ReadApproval { .. }
            | StepKind::FactSuggestion { .. }
            | StepKind::LimitReached { .. } => {
                if self.step.status == StepStatus::WaitingApproval {
                    vec![CardButton::Primary, CardButton::Deny]
                } else {
                    Vec::new()
                }
            }
            StepKind::Message { .. } => vec![CardButton::Copy],
            StepKind::Denied { .. } => Vec::new(),
        }
    }

    /// Interactive approval buttons (cycled by Tab while waiting).
    pub fn waiting_buttons(&self) -> Vec<CardButton> {
        if !self.is_waiting() {
            return Vec::new();
        }
        match self.step.kind {
            StepKind::Command { .. } | StepKind::Patch { .. } => {
                vec![CardButton::Primary, CardButton::Edit, CardButton::Deny]
            }
            StepKind::ReadApproval { .. }
            | StepKind::FactSuggestion { .. }
            | StepKind::LimitReached { .. } => {
                vec![CardButton::Primary, CardButton::Deny]
            }
            _ => Vec::new(),
        }
    }
}

/// Maximum step cards retained in a single [`AgentSession`] timeline so marathon runs stay bounded.
pub const MAX_SESSION_STEPS: usize = 500;

/// Timeline and metadata for one agent session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSession {
    pub id: SessionId,
    pub provider: Option<ProviderInfo>,
    pub steps: Vec<StepEntry>,
    pub usage: Usage,
    pub running: bool,
    pub finish_reason: Option<FinishReason>,
}

impl Default for AgentSession {
    fn default() -> Self {
        Self {
            id: SessionId(0),
            provider: None,
            steps: Vec::new(),
            usage: Usage::default(),
            running: false,
            finish_reason: None,
        }
    }
}

impl AgentSession {
    pub fn new(id: SessionId) -> Self {
        Self {
            id,
            provider: None,
            steps: Vec::new(),
            usage: Usage::default(),
            running: true,
            finish_reason: None,
        }
    }

    pub fn push_step(&mut self, entry: StepEntry) {
        if self.steps.len() >= MAX_SESSION_STEPS {
            if let Some(pos) = self.steps.iter().position(|s| !s.is_waiting()) {
                self.steps.remove(pos);
            } else if !self.steps.is_empty() {
                self.steps.remove(0);
            }
        }
        self.steps.push(entry);
    }

    pub fn step(&self, id: StepId) -> Option<&StepEntry> {
        self.steps.iter().find(|s| s.step.id == id)
    }

    pub fn step_mut(&mut self, id: StepId) -> Option<&mut StepEntry> {
        self.steps.iter_mut().find(|s| s.step.id == id)
    }

    /// The newest step currently waiting for user approval, if any.
    pub fn waiting_step(&self) -> Option<&StepEntry> {
        self.steps.iter().rev().find(|s| s.is_waiting())
    }

    pub fn needs_input(&self) -> bool {
        self.waiting_step().is_some()
    }
}
