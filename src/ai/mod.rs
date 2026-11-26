//! Built-in AI assistant backed by locally installed coding-agent CLIs.
//!
//! Twitty never stores API keys or makes network calls itself: it delegates non-interactive
//! requests to whichever coding agent CLI is configured (`ai_agent`) or installed on `PATH`
//! (`claude`, `codex`, `gemini`, `opencode`), captures bounded stdout off the UI thread, and
//! enforces explicit data-vs-instruction fencing, environment stripping, timeout/cancellation,
//! one-time privacy consent, and strict review-before-run rules.

pub mod argv;
pub mod consent;
pub mod parse;
pub mod prompt;
pub mod runner;
pub mod state;

pub use argv::{
    agent_display_name, non_interactive_argv, resolve_ai_agent, AiResolveError, AI_KNOWN_AGENTS,
    NON_INTERACTIVE_AGENTS,
};
pub use consent::{AiConsentStore, ConsentNotice, AI_CONSENT_VERSION};
pub use parse::{parse_command_reply, parse_explain_reply, ExplainReply, GeneratedCommand};
pub use prompt::{
    current_os_name, detect_shell_name, explain_block_prompt, nl_to_command_prompt, tail_lines,
    BlockAiMode, MAX_BLOCK_OUTPUT_LINES,
};
pub use runner::{
    is_stripped_ai_env_var, run_command_blocking, AiCancel, AiError, MAX_AI_OUTPUT_BYTES,
    MAX_AI_STDERR_BYTES,
};
pub use state::{
    BlockAiCardState, BlockAiCardStatus, ComposerAiState, ConsentDecision, PendingAiAction,
    PendingConsent,
};

/// Result delivered back to the UI thread when an off-thread AI request finishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AiResult {
    /// Natural-language request parsed into a command for review in the composer.
    ComposerCommand(Result<GeneratedCommand, AiError>),
    /// Explanation / fix suggestion for a failed command block.
    BlockAnalysis(Result<ExplainReply, AiError>),
}
