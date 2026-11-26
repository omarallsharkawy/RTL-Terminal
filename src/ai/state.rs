//! Window-free state machines for built-in AI UI flows:
//! - Composer NL-to-command status (idle / loading / review-ready / error)
//! - Failed command block explanation & fix card
//! - One-time privacy consent prompt

use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::ai::parse::ExplainReply;
use crate::ai::prompt::BlockAiMode;
use crate::layout::PaneId;

/// Status of natural-language command generation in the command composer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ComposerAiState {
    #[default]
    Idle,
    /// Agent is running off-thread; Esc cancels it.
    Loading { request_id: u64, prompt: String },
    /// A generated command was placed into the composer editor for user review (NEVER auto-run).
    Ready {
        explanation: String,
        original_prompt: String,
    },
    /// Last AI request failed; message is bilingual.
    Error { message: String },
}

impl ComposerAiState {
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading { .. })
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    pub fn loading_request_id(&self) -> Option<u64> {
        match self {
            Self::Loading { request_id, .. } => Some(*request_id),
            _ => None,
        }
    }
}

/// Content state of the failed-block explanation/fix popover card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockAiCardStatus {
    Loading { request_id: u64 },
    Ready(ExplainReply),
    Error(String),
}

/// State of the readable side card / popover next to a failed command block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockAiCardState {
    pub pane_id: PaneId,
    pub block_id: u64,
    pub mode: BlockAiMode,
    pub command: Option<String>,
    pub exit_code: i32,
    pub status: BlockAiCardStatus,
    pub scroll_lines: usize,
}

impl BlockAiCardState {
    pub fn new_loading(
        pane_id: PaneId,
        block_id: u64,
        mode: BlockAiMode,
        command: Option<String>,
        exit_code: i32,
        request_id: u64,
    ) -> Self {
        Self {
            pane_id,
            block_id,
            mode,
            command,
            exit_code,
            status: BlockAiCardStatus::Loading { request_id },
            scroll_lines: 0,
        }
    }

    pub fn is_loading(&self) -> bool {
        matches!(self.status, BlockAiCardStatus::Loading { .. })
    }

    pub fn suggested_command(&self) -> Option<&str> {
        match &self.status {
            BlockAiCardStatus::Ready(reply) => reply.suggested_command.as_deref(),
            _ => None,
        }
    }

    pub fn scroll_by(&mut self, delta: isize, max_scroll: usize) {
        let next = (self.scroll_lines as isize + delta).clamp(0, max_scroll as isize) as usize;
        self.scroll_lines = next;
    }
}

/// Action deferred behind the one-time privacy consent prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingAiAction {
    ComposerGenerate {
        request: String,
    },
    BlockAnalyze {
        pane_id: PaneId,
        block_id: u64,
        mode: BlockAiMode,
    },
}

/// Decision produced by handling a key press on the privacy consent modal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsentDecision {
    None,
    Accept,
    Cancel,
}

/// Active one-time privacy consent prompt waiting for the user's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingConsent {
    pub agent_name: String,
    pub action: PendingAiAction,
}

impl PendingConsent {
    pub fn key(&self, key: &Key, modifiers: ModifiersState) -> ConsentDecision {
        if !modifiers.is_empty() {
            return ConsentDecision::None;
        }
        match key {
            Key::Named(NamedKey::Enter) => ConsentDecision::Accept,
            Key::Named(NamedKey::Escape) => ConsentDecision::Cancel,
            _ => ConsentDecision::None,
        }
    }
}
