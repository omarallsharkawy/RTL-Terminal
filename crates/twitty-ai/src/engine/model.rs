//! Shared contract between the agent engine (lane s10) and the app/panel (lanes n20, n21).
//!
//! The engine runs on its own thread. The app sends [`AgentCommand`]s and receives
//! [`AgentEvent`]s over channels ([`AgentHandle`]). Everything the panel draws comes from
//! [`Step`]s; everything the user decides goes back as a [`Decision`]. See
//! docs/design/AGENT.md for the UI and the security rules this contract serves.
//!
//! Changing a type here is a cross-lane change: keep additions backwards compatible within a
//! round (new enum variants are fine; the app matches with a fallback arm).

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};

/// One agent run (one user request and its follow-ups).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub u64);

/// One card in the timeline, unique within a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StepId(pub u64);

/// Which backend answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    /// An installed coding-agent CLI (`claude`, `codex`, `gemini`, `opencode`), non-interactive.
    Cli {
        program: String,
    },
    Anthropic,
    OpenAi,
    Gemini,
    /// Ollama or another local OpenAI-compatible server.
    Ollama {
        base_url: String,
    },
}

/// Shown in the panel header: `Claude · API`, `codex · CLI`, `llama3 · Ollama`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderInfo {
    pub kind: ProviderKind,
    pub model: String,
    pub display: String,
}

/// A read-only tool the engine runs itself (inside the project root without a prompt).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadTool {
    ReadFile,
    ListDir,
    Search,
    ReadBlock,
}

/// One file in a proposed patch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PatchFile {
    /// Relative to the project root.
    pub path: PathBuf,
    pub added: u32,
    pub removed: u32,
    pub new_file: bool,
    pub deleted: bool,
}

/// Why the policy refused a tool call (AGENT.md §2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DenyReason {
    /// Matched the fixed deny list; `rule` names the pattern, e.g. `"sudo"`, `"curl|sh"`.
    HardDeny { rule: String },
    /// A path resolved outside the project root or into a protected location.
    OutsideProject { path: PathBuf },
    /// The user pressed Deny.
    User,
}

/// What a card shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepKind {
    /// The model's short plan.
    Plan { text: String },
    /// A read tool call and a bounded preview of its result.
    Read {
        tool: ReadTool,
        target: String,
        preview: String,
    },
    /// A sensitive or out-of-project read waiting for explicit approval.
    ReadApproval { tool: ReadTool, target: String },
    /// A shell command waiting for approval, then run in a visible pane by the app.
    Command {
        command: String,
        cwd: PathBuf,
        /// One line from the model: why it wants this.
        reason: String,
        /// Proposed after terminal output, file content or other data entered the context
        /// (AGENT.md §2.4). Never auto-allowed when true.
        after_untrusted_data: bool,
    },
    /// A patch waiting for approval; the engine applies it after approval.
    Patch {
        files: Vec<PatchFile>,
        /// Unified diff, already validated against the project root.
        diff: String,
        reason: String,
    },
    /// The outcome of an approved command.
    CommandResult {
        exit: Option<i32>,
        duration_ms: u64,
        /// Last lines of output, redacted.
        tail: String,
    },
    /// The outcome of an applied patch; `undo_available` until the files change again.
    PatchApplied {
        files: Vec<PathBuf>,
        undo_available: bool,
    },
    /// A fact the model suggests remembering for this project (saved only if approved).
    FactSuggestion { text: String },
    /// Final answer or a question to the user.
    Message { text: String },
    /// A refused tool call, shown in red.
    Denied { what: String, reason: DenyReason },
    /// A step/token/cost limit paused the run; Approve continues, Deny stops.
    LimitReached { what: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepStatus {
    Running,
    /// The only interactive state: Command, Patch, ReadApproval, FactSuggestion and LimitReached cards.
    WaitingApproval,
    Done,
    Denied,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub id: StepId,
    pub kind: StepKind,
    pub status: StepStatus,
}

/// The user's answer to a waiting card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decision {
    Approve,
    /// Approve with the user's edited command text or edited unified diff.
    ApproveEdited(String),
    Deny,
}

/// What goes into a request; every item is listed in the context inspector (AGENT.md §1.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    /// An approved rules file (AGENTS.md, CLAUDE.md, TWITTY.md) or a user rule.
    Rule,
    Memory,
    Block,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextItem {
    pub kind: ContextKind,
    /// Short label for the inspector, e.g. `AGENTS.md`, `fact #12`, `block: cargo test`.
    pub label: String,
    /// Already redacted.
    pub text: String,
    pub est_tokens: u32,
    /// The user may untick an item for this request.
    pub included: bool,
}

/// Assembled by the memory lane (`crate::context`), consumed by the engine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContextBundle {
    pub project_root: Option<PathBuf>,
    pub items: Vec<ContextItem>,
    pub budget_tokens: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Estimated cost in millionths of a US dollar, when the provider has a price table.
    pub cost_micros_usd: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinishReason {
    Completed,
    Stopped,
    Error(String),
}

/// App -> engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentCommand {
    Start {
        session: SessionId,
        request: String,
        context: ContextBundle,
        cwd: PathBuf,
    },
    /// A follow-up message in the same session.
    Reply {
        session: SessionId,
        text: String,
    },
    Decide {
        session: SessionId,
        step: StepId,
        decision: Decision,
    },
    /// The app finished running an approved Command step in a pane.
    CommandFinished {
        session: SessionId,
        step: StepId,
        exit: Option<i32>,
        duration_ms: u64,
        /// Bounded output tail; the engine redacts it again before use.
        output: String,
    },
    Undo {
        session: SessionId,
        step: StepId,
    },
    Stop {
        session: SessionId,
    },
}

/// Engine -> app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentEvent {
    Started {
        session: SessionId,
        provider: ProviderInfo,
    },
    StepAdded {
        session: SessionId,
        step: Step,
    },
    StepUpdated {
        session: SessionId,
        step: Step,
    },
    /// Run this approved command in a visible pane, then answer with `CommandFinished`.
    RunInPane {
        session: SessionId,
        step: StepId,
        command: String,
        cwd: PathBuf,
    },
    /// The user approved a FactSuggestion; the app stores it in project memory.
    RememberFact {
        session: SessionId,
        project_root: Option<PathBuf>,
        text: String,
    },
    Usage {
        session: SessionId,
        usage: Usage,
    },
    Finished {
        session: SessionId,
        reason: FinishReason,
    },
}

/// The app's end of the engine thread.
pub struct AgentHandle {
    pub commands: Sender<AgentCommand>,
    pub events: Receiver<AgentEvent>,
}
