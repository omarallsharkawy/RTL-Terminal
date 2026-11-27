//! Window-free state machine, layout geometry, hit testing, and scripted engine abstraction for
//! the Twitty Agent side panel (`docs/design/AGENT.md` §1).
//!
//! Consumes [`AgentEvent`]s and produces [`AgentCommand`]s against the shared contract in
//! `twitty_ai::engine::model`. Drawing lives in `render::scene::agent`; app/PTY hooks live in
//! `app::agent_panel`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use twitty_ai::engine::model::{
    AgentCommand, AgentEvent, AgentHandle, ContextBundle, ContextKind, Decision, DenyReason,
    FinishReason, ProviderInfo, ProviderKind, ReadTool, SessionId, Step, StepId, StepKind,
    StepStatus, Usage,
};
use twitty_ai::engine::EngineConfig;
use unicode_bidi::{bidi_class, BidiClass};
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

use crate::composer::editor::TextEditor;
use crate::layout::{PaneId, Rect};
use crate::review::diff::{parse as parse_diff, Limits as DiffLimits};
use crate::ui::chrome::UiLanguage;
use twitty_render::metrics::CellMetrics;

// ---- Docking geometry constants (AGENT.md §1.1) ----

/// Default panel width as a fraction of the tab content area.
pub const DEFAULT_WIDTH_FRACTION: f32 = 0.38;
/// Narrowest panel width in logical pixels.
pub const MIN_PANEL_PX: f32 = 340.0;
/// Widest panel width in logical pixels.
pub const MAX_PANEL_PX: f32 = 560.0;
/// Maximum share of the tab area the panel may occupy (terminal keeps at least 50%).
pub const MAX_TAB_SHARE: f32 = 0.50;
/// The panel never docks narrower than this (logical px).
pub const MIN_USABLE_PX: f32 = 240.0;
/// On a narrow window the terminal may shrink below 50%, but never below this (logical px).
pub const MIN_TERMINAL_PX: f32 = 160.0;
/// Fixed header height in logical pixels (AGENT.md §1.1).
pub const HEADER_HEIGHT_PX: f32 = 36.0;
/// Hit slop along the draggable divider edge in logical pixels.
pub const DIVIDER_HIT_PX: f32 = 6.0;
/// Maximum visible lines in a collapsed Plan card before expansion.
pub const PLAN_COLLAPSED_LINES: usize = 6;
/// Maximum visible lines in the footer composer editor.
pub const COMPOSER_MAX_LINES: usize = 5;
/// Maximum visible lines in an inline diff / command editor on a card.
pub const CARD_EDITOR_MAX_LINES: usize = 10;
/// Maximum visible diff rows in an expanded Patch card before capping height.
pub const MAX_PATCH_DIFF_ROWS: usize = 24;
/// Maximum prompt history entries kept in the panel composer.
const MAX_HISTORY: usize = 100;

/// One empty-state example chip (AGENT.md §1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExampleChip {
    pub prompt_en: &'static str,
    pub prompt_ar: &'static str,
}

impl ExampleChip {
    pub fn label(self, language: UiLanguage) -> &'static str {
        language.text(self.prompt_en, self.prompt_ar)
    }
}

/// Empty-state intro text (AGENT.md §1.1), wrapped by the layout so the chips sit below it.
pub const EMPTY_INTRO: (&str, &str) = (
    "Ask the agent to read files, run commands, or edit code. Every command and patch asks first.",
    "اطلب من الوكيل قراءة الملفات أو تشغيل الأوامر أو تعديل الكود. كل أمر وتعديل يطلب موافقتك أولاً.",
);
/// Logical px from the body top to the first intro line, and between intro lines.
pub const EMPTY_INTRO_TOP: f32 = 44.0;
pub const EMPTY_INTRO_LINE_H: f32 = 17.0;
const EMPTY_INTRO_MAX_LINES: usize = 5;

/// Three bilingual empty-state starter chips (AGENT.md §1.1).
pub const EXAMPLE_CHIPS: [ExampleChip; 3] = [
    ExampleChip {
        prompt_en: "Run the tests and fix the first failure",
        prompt_ar: "شغّل الاختبارات وصلّح أول فشل",
    },
    ExampleChip {
        prompt_en: "Explain the last error",
        prompt_ar: "اشرح آخر خطأ",
    },
    ExampleChip {
        prompt_en: "Summarize uncommitted changes",
        prompt_ar: "لخّص التغييرات غير المثبتة",
    },
];

/// Split the tab's `content` rect into `(terminal_area, panel_area)` per AGENT.md §1.1 & §1.5:
/// 38% default (340–560 logical px), keeping the terminal at least 50% wide.
/// Docks on the right in English UI and on the left in Arabic UI.
pub fn split_agent_content(
    content: Rect,
    scale: f32,
    custom_width_logical: Option<f32>,
    language: UiLanguage,
) -> (Rect, Option<Rect>) {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let min_px = MIN_PANEL_PX * scale;
    let max_px = MAX_PANEL_PX * scale;
    let want = match custom_width_logical {
        Some(w) if w.is_finite() => (w.clamp(MIN_PANEL_PX, MAX_PANEL_PX) * scale).round(),
        _ => (content.w * DEFAULT_WIDTH_FRACTION)
            .clamp(min_px, max_px)
            .round(),
    };
    let max_share = (content.w * MAX_TAB_SHARE).floor();
    let mut width = want.min(max_share);
    if width < MIN_USABLE_PX * scale {
        // Narrow window: give the panel its usable minimum while the terminal keeps a few
        // columns, instead of silently not opening.
        width = (MIN_USABLE_PX * scale).round();
        if content.w - width < MIN_TERMINAL_PX * scale {
            return (content, None);
        }
    }
    if !width.is_finite() {
        return (content, None);
    }
    let term_w = (content.w - width).max(0.0);
    if language == UiLanguage::Arabic {
        (
            Rect::new(content.x + width, content.y, term_w, content.h),
            Some(Rect::new(content.x, content.y, width, content.h)),
        )
    } else {
        (
            Rect::new(content.x, content.y, term_w, content.h),
            Some(Rect::new(
                content.right() - width,
                content.y,
                width,
                content.h,
            )),
        )
    }
}

/// Convert a pointer drag X coordinate inside the full tab content area into a clamped logical
/// width for the docked agent panel.
pub fn width_from_divider_drag(
    tab_content: Rect,
    pointer_x: f64,
    scale: f32,
    language: UiLanguage,
) -> f32 {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let px = pointer_x as f32;
    let raw_phys = if language == UiLanguage::Arabic {
        px - tab_content.x
    } else {
        tab_content.right() - px
    };
    let max_logical = ((tab_content.w * MAX_TAB_SHARE) / scale).clamp(MIN_PANEL_PX, MAX_PANEL_PX);
    (raw_phys / scale).clamp(MIN_PANEL_PX, max_logical)
}

/// Determine paragraph base direction from the first strong directional character (AGENT.md §1.5).
pub fn paragraph_is_rtl(text: &str) -> bool {
    for c in text.chars() {
        match bidi_class(c) {
            BidiClass::R | BidiClass::AL => return true,
            BidiClass::L => return false,
            _ => {}
        }
    }
    false
}

/// Format an Arabic noun with proper number agreement:
/// - `1` → `one` (e.g. `"ذكرى واحدة"`, `"كتلة واحدة"`)
/// - `2` → `two` (e.g. `"ذكريان"`, `"كتلتان"`)
/// - `n % 100` in `3..=10` → `"{n} {few}"` (e.g. `"3 ذكريات"`, `"10 كتل"`)
/// - otherwise (`0`, `11+`, `100`, `101`, `102`, ...) → `"{n} {many}"` (e.g. `"11 ذكرى"`, `"12 كتلة"`)
pub fn arabic_count(n: usize, one: &str, two: &str, few: &str, many: &str) -> String {
    match n {
        1 => one.to_owned(),
        2 => two.to_owned(),
        n if (3..=10).contains(&(n % 100)) => format!("{n} {few}"),
        n => format!("{n} {many}"),
    }
}

/// Format token count and optional USD cost for the header pill.
pub fn format_usage_pill(usage: Usage, language: UiLanguage) -> Option<String> {
    let total_tokens = usage.input_tokens.saturating_add(usage.output_tokens);
    if total_tokens == 0 && usage.cost_micros_usd.is_none() {
        return None;
    }
    let tok_label = if total_tokens >= 1_000_000 {
        format!("{:.1}M", total_tokens as f64 / 1_000_000.0)
    } else if total_tokens >= 1_000 {
        format!("{:.1}k", total_tokens as f64 / 1_000.0)
    } else {
        total_tokens.to_string()
    };
    match (language, usage.cost_micros_usd) {
        (UiLanguage::English, Some(micros)) => {
            let usd = micros as f64 / 1_000_000.0;
            if usd >= 0.01 {
                Some(format!("{tok_label} tok · ${usd:.2}"))
            } else {
                Some(format!("{tok_label} tok · ${usd:.4}"))
            }
        }
        (UiLanguage::Arabic, Some(micros)) => {
            // Pure numeric/symbol string avoids Unicode bidi scrambling of `$` and `·` in RTL.
            let usd = micros as f64 / 1_000_000.0;
            if usd >= 0.01 {
                Some(format!("{tok_label} · ${usd:.2}"))
            } else {
                Some(format!("{tok_label} · ${usd:.4}"))
            }
        }
        (UiLanguage::English, None) => Some(format!("{tok_label} tok")),
        (UiLanguage::Arabic, None) => {
            if total_tokens < 1_000 {
                Some(arabic_count(
                    total_tokens as usize,
                    "رمز واحد",
                    "رمزان",
                    "رموز",
                    "رمز",
                ))
            } else {
                Some(format!("{tok_label} رمز"))
            }
        }
    }
}

/// Format the one-line footer context strip summary (AGENT.md §1.1):
/// e.g. `Rules · 3 memories · 2 blocks`.
pub fn format_context_strip(bundle: &ContextBundle, language: UiLanguage) -> String {
    let mut rules = 0usize;
    let mut memories = 0usize;
    let mut blocks = 0usize;
    let mut files = 0usize;
    for item in bundle.items.iter().filter(|i| i.included) {
        match item.kind {
            ContextKind::Rule => rules += 1,
            ContextKind::Memory => memories += 1,
            ContextKind::Block => blocks += 1,
            ContextKind::File => files += 1,
        }
    }
    let mut parts = Vec::new();
    match language {
        UiLanguage::English => {
            if rules > 0 {
                if rules == 1 {
                    parts.push("Rules".to_owned());
                } else {
                    parts.push(format!("{rules} rules"));
                }
            }
            if memories > 0 {
                parts.push(if memories == 1 {
                    "1 memory".to_owned()
                } else {
                    format!("{memories} memories")
                });
            }
            if blocks > 0 {
                parts.push(if blocks == 1 {
                    "1 block".to_owned()
                } else {
                    format!("{blocks} blocks")
                });
            }
            if files > 0 {
                parts.push(if files == 1 {
                    "1 file".to_owned()
                } else {
                    format!("{files} files")
                });
            }
            if parts.is_empty() {
                "Context · 0 items".to_owned()
            } else {
                parts.join(" · ")
            }
        }
        UiLanguage::Arabic => {
            if rules > 0 {
                if rules == 1 {
                    parts.push("القواعد".to_owned());
                } else {
                    parts.push(arabic_count(
                        rules,
                        "قاعدة واحدة",
                        "قاعدتان",
                        "قواعد",
                        "قاعدة",
                    ));
                }
            }
            if memories > 0 {
                parts.push(arabic_count(
                    memories,
                    "ذكرى واحدة",
                    "ذكريان",
                    "ذكريات",
                    "ذكرى",
                ));
            }
            if blocks > 0 {
                parts.push(arabic_count(blocks, "كتلة واحدة", "كتلتان", "كتل", "كتلة"));
            }
            if files > 0 {
                parts.push(arabic_count(files, "ملف واحد", "ملفان", "ملفات", "ملف"));
            }
            if parts.is_empty() {
                "السياق · لا عناصر".to_owned()
            } else {
                parts.join(" · ")
            }
        }
    }
}

/// Format total included tokens against the bundle budget.
pub fn included_context_tokens(bundle: &ContextBundle) -> u32 {
    bundle
        .items
        .iter()
        .filter(|item| item.included)
        .fold(0u32, |acc, item| acc.saturating_add(item.est_tokens))
}

/// Format duration in milliseconds cleanly (e.g. `420ms`, `2.3s`, `1m 12s`).
pub fn format_duration_ms(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1_000.0)
    } else {
        let secs = ms / 1_000;
        format!("{}m {}s", secs / 60, secs % 60)
    }
}

/// Label for a [`ReadTool`] invocation.
pub fn read_tool_name(tool: ReadTool) -> &'static str {
    match tool {
        ReadTool::ReadFile => "read_file",
        ReadTool::ListDir => "list_dir",
        ReadTool::Search => "search",
        ReadTool::ReadBlock => "read_block",
    }
}

/// Bilingual description of a [`DenyReason`].
pub fn format_deny_reason(reason: &DenyReason, language: UiLanguage) -> String {
    match reason {
        DenyReason::HardDeny { rule } => match language {
            UiLanguage::English => format!("Denied by policy ({rule})"),
            UiLanguage::Arabic => format!("مرفوض بواسطة السياسة ({rule})"),
        },
        DenyReason::OutsideProject { path } => match language {
            UiLanguage::English => format!("Outside project root: {}", path.display()),
            UiLanguage::Arabic => format!("خارج مجلد المشروع: {}", path.display()),
        },
        DenyReason::User => language.text("Denied by user", "رفضه المستخدم").to_owned(),
    }
}

/// Cards that wait for the user's Approve/Deny (AGENT.md §1.3).
pub fn is_approval_kind(kind: &StepKind) -> bool {
    matches!(
        kind,
        StepKind::Command { .. }
            | StepKind::Patch { .. }
            | StepKind::ReadApproval { .. }
            | StepKind::FactSuggestion { .. }
            | StepKind::LimitReached { .. }
    )
}

// ---- Engine spawner abstraction & scripted fake engine ----

/// Factory producing an [`AgentHandle`]; the app uses `twitty_ai::engine::spawn`, tests a script.
pub trait AgentEngineSpawner: Send + Sync {
    fn spawn(&self, config: EngineConfig) -> Result<AgentHandle, String>;
}

/// Function-pointer spawner implementation for lightweight wiring.
pub type SpawnEngineFn = fn(EngineConfig) -> Result<AgentHandle, String>;

impl AgentEngineSpawner for SpawnEngineFn {
    fn spawn(&self, config: EngineConfig) -> Result<AgentHandle, String> {
        (self)(config)
    }
}

/// Callback invoked by [`ScriptedEngine`] for each incoming [`AgentCommand`].
type ScriptedHandler =
    Box<dyn Fn(AgentCommand, &mpsc::Sender<AgentEvent>) -> bool + Send + Sync + 'static>;

/// Scripted fake engine that speaks the exact `AgentCommand`/`AgentEvent` channel contract.
/// Used by window-free tests and local development before lane s10's engine is linked.
pub struct ScriptedEngine {
    handler: ScriptedHandler,
}

impl ScriptedEngine {
    /// Create a scripted engine whose `handler` is called for each incoming [`AgentCommand`] and
    /// emits any number of [`AgentEvent`]s. Return `false` from `handler` to stop the loop.
    pub fn new(
        handler: impl Fn(AgentCommand, &mpsc::Sender<AgentEvent>) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            handler: Box::new(handler),
        }
    }

    /// Spawn a background thread speaking `AgentCommand` -> `AgentEvent` over standard channels.
    pub fn spawn_handle(
        handler: impl FnMut(AgentCommand, &mpsc::Sender<AgentEvent>) -> bool + Send + 'static,
    ) -> AgentHandle {
        let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
        let (evt_tx, evt_rx) = mpsc::channel::<AgentEvent>();
        let mut handler = handler;
        let _ = thread::Builder::new()
            .name("twitty-agent-scripted".into())
            .spawn(move || {
                while let Ok(cmd) = cmd_rx.recv() {
                    if !handler(cmd, &evt_tx) {
                        break;
                    }
                }
            });
        AgentHandle {
            commands: cmd_tx,
            events: evt_rx,
        }
    }
}

impl AgentEngineSpawner for ScriptedEngine {
    fn spawn(&self, _config: EngineConfig) -> Result<AgentHandle, String> {
        // Because `ScriptedEngine` may be spawned once per app lifetime, we forward through a
        // channel owned by a background thread only if created via `spawn_handle`; here we provide
        // a clean fallback if called via trait object directly.
        let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
        let (evt_tx, evt_rx) = mpsc::channel::<AgentEvent>();
        let _ = thread::Builder::new()
            .name("twitty-agent-placeholder".into())
            .spawn(move || {
                placeholder_engine_loop(cmd_rx, evt_tx);
            });
        let _ = &self.handler;
        Ok(AgentHandle {
            commands: cmd_tx,
            events: evt_rx,
        })
    }
}

/// Offline stand-in engine that answers every request with a fixed message (tests, demos).
pub fn spawn_placeholder_engine(_config: EngineConfig) -> Result<AgentHandle, String> {
    let (cmd_tx, cmd_rx) = mpsc::channel::<AgentCommand>();
    let (evt_tx, evt_rx) = mpsc::channel::<AgentEvent>();
    thread::Builder::new()
        .name("twitty-agent-stub".into())
        .spawn(move || placeholder_engine_loop(cmd_rx, evt_tx))
        .map_err(|e| format!("failed to spawn agent thread: {e}"))?;
    Ok(AgentHandle {
        commands: cmd_tx,
        events: evt_rx,
    })
}

fn placeholder_engine_loop(cmd_rx: mpsc::Receiver<AgentCommand>, evt_tx: mpsc::Sender<AgentEvent>) {
    let mut next_step = 1u64;
    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            AgentCommand::Start { session, .. } | AgentCommand::Reply { session, .. } => {
                let _ = evt_tx.send(AgentEvent::Started {
                    session,
                    provider: ProviderInfo {
                        kind: ProviderKind::Cli {
                            program: "agent".into(),
                        },
                        model: "pending".into(),
                        display: "Agent · Pending".into(),
                    },
                });
                let id = StepId(next_step);
                next_step += 1;
                let _ = evt_tx.send(AgentEvent::StepAdded {
                    session,
                    step: Step {
                        id,
                        kind: StepKind::Message {
                            text: crate::ui::chrome::tr("Agent engine backend is not linked in this build yet. Configure a provider in Settings → AI.", "محرك الوكيل غير مرتبط في هذا البناء بعد. اضبط المزوّد في الإعدادات ← الذكاء الاصطناعي.").into(),
                        },
                        status: StepStatus::Done,
                    },
                });
                let _ = evt_tx.send(AgentEvent::Finished {
                    session,
                    reason: FinishReason::Completed,
                });
            }
            AgentCommand::Stop { session } => {
                let _ = evt_tx.send(AgentEvent::Finished {
                    session,
                    reason: FinishReason::Stopped,
                });
            }
            AgentCommand::Decide { .. }
            | AgentCommand::CommandFinished { .. }
            | AgentCommand::Undo { .. } => {}
        }
    }
}

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
}

impl StepEntry {
    pub fn new(step: Step) -> Self {
        Self {
            step,
            expanded: false,
            block_link: None,
            user_edited: false,
        }
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
    /// Sessions keyed by id so multiple runs remain addressable.
    pub sessions: HashMap<SessionId, AgentSession>,
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
        self.active_session.and_then(|id| self.sessions.get(&id))
    }

    pub fn active_session_mut(&mut self) -> Option<&mut AgentSession> {
        let id = self.active_session?;
        self.sessions.get_mut(&id)
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
        if let Some(sess) = self.sessions.get_mut(&session) {
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
                let entry = self
                    .sessions
                    .entry(session)
                    .or_insert_with(|| AgentSession::new(session));
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
                let entry = self
                    .sessions
                    .entry(session)
                    .or_insert_with(|| AgentSession::new(session));
                entry.steps.push(StepEntry::new(step));
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
                let sess = self
                    .sessions
                    .entry(session)
                    .or_insert_with(|| AgentSession::new(session));
                if let Some(existing) = sess.step_mut(id) {
                    existing.step = step;
                } else {
                    sess.steps.push(StepEntry::new(step));
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
                let sess = self
                    .sessions
                    .entry(session)
                    .or_insert_with(|| AgentSession::new(session));
                sess.usage = usage;
                AgentPanelAction::Redraw
            }
            AgentEvent::Finished { session, reason } => {
                if let Some(sess) = self.sessions.get_mut(&session) {
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
            if let Some(sess) = self.sessions.get_mut(&session) {
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
            self.sessions.insert(session, AgentSession::new(session));
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
        if let Some(sess) = self.sessions.get_mut(&session) {
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
    pub fn view(&self, rect: Rect, language: UiLanguage, cursor_blink_on: bool) -> AgentPanelView {
        let session = self.active_session().cloned();
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

// ---- Render snapshot & shared layout geometry ----

/// Immutable snapshot handed to `render::scene::agent` and `AgentPanelLayout`.
#[derive(Clone, Debug)]
pub struct AgentPanelView {
    pub rect: Rect,
    pub language: UiLanguage,
    pub focused: bool,
    pub focus: AgentFocus,
    pub cursor_blink_on: bool,
    pub project_name: String,
    pub session: Option<AgentSession>,
    pub selected_step: Option<StepId>,
    pub focused_button: usize,
    pub edit_mode: Option<CardEditMode>,
    pub composer: TextEditor,
    pub context: ContextBundle,
    pub inspector_open: bool,
    pub inspector_selected: usize,
    pub scroll_y: f32,
    pub pinned_to_bottom: bool,
    pub hover: AgentPanelHit,
}

impl AgentPanelView {
    pub fn steps(&self) -> &[StepEntry] {
        self.session.as_ref().map_or(&[], |s| s.steps.as_slice())
    }

    pub fn is_running(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.running)
    }

    pub fn mirrored(&self) -> bool {
        self.language == UiLanguage::Arabic
    }
}

/// Pointer hit target inside the Agent panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentPanelHit {
    #[default]
    Outside,
    Inside,
    Divider,
    Close,
    Stop,
    JumpToLatest,
    ExampleChip(usize),
    ContextStrip,
    Composer,
    Send,
    Card {
        step: StepId,
        button: Option<CardButton>,
    },
    InspectorClose,
    InspectorToggle(usize),
    InspectorBackdrop,
}

/// Computed physical-pixel layout of one step card.
#[derive(Clone, Debug)]
pub struct CardLayout {
    pub step_id: StepId,
    pub rect: Rect,
    /// Leading 2 px status strip (left in English, right in Arabic).
    pub strip: Rect,
    /// Inner content rect (respecting 12 px padding).
    pub inner: Rect,
    /// Clickable buttons on the card.
    pub buttons: Vec<(CardButton, Rect)>,
    /// Inline editor rect when this card is in edit mode.
    pub editor_rect: Option<Rect>,
    /// Diff preview rect when a Patch card is expanded.
    pub diff_rect: Option<Rect>,
}

/// Computed physical-pixel layout of the Context Inspector overlay sheet.
#[derive(Clone, Debug)]
pub struct InspectorLayout {
    pub sheet: Rect,
    pub header: Rect,
    pub close_button: Rect,
    /// `(item_index, row_rect, checkbox_rect)`.
    pub items: Vec<(usize, Rect, Rect)>,
}

/// Complete physical-pixel layout shared by drawing (`render::scene::agent`) and pointer hit testing.
#[derive(Clone, Debug)]
pub struct AgentPanelLayout {
    pub panel: Rect,
    pub divider_hit: Rect,
    pub border_line: Rect,
    pub header: Rect,
    pub close_button: Rect,
    pub stop_button: Option<Rect>,
    pub usage_pill: Option<Rect>,
    pub title_area: Rect,
    pub body: Rect,
    pub jump_to_latest: Option<Rect>,
    pub empty_chips: Vec<Rect>,
    /// Wrapped empty-state intro lines (drawn above the chips).
    pub empty_intro: Vec<String>,
    pub cards: Vec<CardLayout>,
    pub footer: Rect,
    pub context_strip: Rect,
    pub composer_box: Rect,
    pub send_button: Rect,
    pub inspector: Option<InspectorLayout>,
    pub max_scroll: f32,
    pub scroll_px: f32,
    pub mirrored: bool,
}

impl AgentPanelLayout {
    pub fn build(view: &AgentPanelView, metrics: CellMetrics) -> Self {
        let scale = if metrics.scale.is_finite() && metrics.scale > 0.0 {
            metrics.scale
        } else {
            1.0
        };
        let px = |logical: f32| logical * scale;
        let panel = view.rect;
        let mirrored = view.mirrored();
        let border = px(1.0).round().max(1.0);
        let pad = px(12.0).round();
        let gap = px(8.0).round();

        // Inner edge divider + hairline border.
        let (border_line, divider_hit, inner_x, inner_w) = if mirrored {
            let bx = panel.right() - border;
            let hit_w = px(DIVIDER_HIT_PX);
            (
                Rect::new(bx, panel.y, border, panel.h),
                Rect::new(panel.right() - hit_w * 0.5, panel.y, hit_w, panel.h),
                panel.x,
                (panel.w - border).max(0.0),
            )
        } else {
            let hit_w = px(DIVIDER_HIT_PX);
            (
                Rect::new(panel.x, panel.y, border, panel.h),
                Rect::new(panel.x - hit_w * 0.5, panel.y, hit_w, panel.h),
                panel.x + border,
                (panel.w - border).max(0.0),
            )
        };

        // Header (36 px logical).
        let header_h = px(HEADER_HEIGHT_PX).round().min(panel.h);
        let header = Rect::new(inner_x, panel.y, inner_w, header_h);
        let btn_side = px(24.0).min(header_h - px(4.0)).max(0.0);
        let btn_y = header.y + (header.h - btn_side) * 0.5;
        let close_logical = Rect::new(header.right() - pad - btn_side, btn_y, btn_side, btn_side);
        let mut trail_left = close_logical.x - px(6.0);

        let stop_logical = if view.is_running() {
            let stop_w = px(54.0).min((header.w * 0.22).max(btn_side));
            let r = Rect::new(trail_left - stop_w, btn_y, stop_w, btn_side);
            trail_left = r.x - px(6.0);
            Some(r)
        } else {
            None
        };

        let usage_text = view
            .session
            .as_ref()
            .and_then(|s| format_usage_pill(s.usage, view.language));
        let usage_logical = usage_text.map(|txt| {
            let w = ((txt.chars().count() as f32) * px(6.2) + px(14.0))
                .clamp(px(48.0), (header.w * 0.34).max(px(48.0)));
            let r = Rect::new(trail_left - w, btn_y, w, btn_side);
            trail_left = r.x - px(6.0);
            r
        });

        let title_logical = Rect::new(
            header.x + pad,
            header.y,
            (trail_left - (header.x + pad)).max(0.0),
            header.h,
        );

        let close_button = view.language.mirror(close_logical, header);
        let stop_button = stop_logical.map(|r| view.language.mirror(r, header));
        let usage_pill = usage_logical.map(|r| view.language.mirror(r, header));
        let title_area = view.language.mirror(title_logical, header);

        // Footer: context strip (24 px) + composer box (dynamic 1..5 lines) + padding.
        let strip_h = px(24.0).round();
        let composer_lines = view.composer.line_count().clamp(1, COMPOSER_MAX_LINES);
        let composer_line_h = metrics.line_height.max(px(18.0));
        let composer_h = (composer_lines as f32 * composer_line_h + px(14.0)).round();
        let footer_h =
            (strip_h + px(6.0) + composer_h + px(16.0)).min((panel.h - header_h).max(0.0));
        let footer = Rect::new(inner_x, panel.bottom() - footer_h, inner_w, footer_h);

        let context_strip = Rect::new(
            footer.x + pad,
            footer.y + px(4.0),
            (footer.w - 2.0 * pad).max(0.0),
            strip_h,
        );
        let send_w = px(56.0).min((footer.w * 0.25).max(px(36.0)));
        let send_h = px(28.0).min(composer_h);
        let input_y = context_strip.bottom() + px(4.0);
        let send_logical = Rect::new(
            footer.right() - pad - send_w,
            input_y + (composer_h - send_h) * 0.5,
            send_w,
            send_h,
        );
        let composer_logical = Rect::new(
            footer.x + pad,
            input_y,
            (send_logical.x - px(6.0) - (footer.x + pad)).max(0.0),
            composer_h,
        );
        let send_button = view.language.mirror(send_logical, footer);
        let composer_box = view.language.mirror(composer_logical, footer);

        // Body scrolling area between header and footer.
        let body_top = header.bottom() + border;
        let body_h = (footer.y - border - body_top).max(0.0);
        let body = Rect::new(inner_x, body_top, inner_w, body_h);

        // Empty state chips or timeline cards.
        let mut empty_chips = Vec::new();
        let mut empty_intro = Vec::new();
        let mut cards = Vec::new();
        let mut max_scroll = 0.0_f32;
        let mut scroll_px = 0.0_f32;
        let mut jump_to_latest = None;

        let steps = view.steps();
        if steps.is_empty() {
            let chip_h = px(32.0).round();
            let chip_gap = px(8.0).round();
            let chip_w = (body.w - 2.0 * pad).max(0.0);
            empty_intro = wrap_words(
                view.language.text(EMPTY_INTRO.0, EMPTY_INTRO.1),
                chip_w,
                px(7.0),
                EMPTY_INTRO_MAX_LINES,
            );
            let start_y = (body.y
                + px(EMPTY_INTRO_TOP)
                + empty_intro.len() as f32 * px(EMPTY_INTRO_LINE_H)
                + px(12.0))
            .round();
            for i in 0..EXAMPLE_CHIPS.len() {
                let y = start_y + i as f32 * (chip_h + chip_gap);
                if y + chip_h <= body.bottom() {
                    empty_chips.push(Rect::new(body.x + pad, y, chip_w, chip_h));
                }
            }
        } else {
            let card_w = (body.w - 2.0 * pad).max(0.0);
            let card_heights: Vec<f32> = steps
                .iter()
                .map(|entry| {
                    let editing = view
                        .edit_mode
                        .as_ref()
                        .filter(|e| e.step_id() == entry.step.id);
                    measure_card_height(entry, editing, card_w, metrics, scale)
                })
                .collect();
            let total_h: f32 = card_heights.iter().sum::<f32>()
                + gap * (card_heights.len().saturating_sub(1) as f32)
                + 2.0 * pad;
            max_scroll = ((total_h - body.h).max(0.0)) / scale;
            let clamped_scroll = if view.pinned_to_bottom {
                max_scroll
            } else {
                view.scroll_y.clamp(0.0, max_scroll)
            };
            scroll_px = (clamped_scroll * scale).round();

            let mut y = body.y + pad - scroll_px;
            for (entry, h) in steps.iter().zip(card_heights) {
                let editing = view
                    .edit_mode
                    .as_ref()
                    .filter(|e| e.step_id() == entry.step.id);
                let card_rect = Rect::new(body.x + pad, y, card_w, h);
                let strip_w = px(2.0).round().max(2.0);
                let strip_inset_y = px(6.0).min(card_rect.h * 0.25);
                let strip_logical = Rect::new(
                    card_rect.x + px(1.0),
                    card_rect.y + strip_inset_y,
                    strip_w,
                    (card_rect.h - 2.0 * strip_inset_y).max(0.0),
                );
                let strip = view.language.mirror(strip_logical, card_rect);
                let inner = Rect::new(
                    card_rect.x + pad,
                    card_rect.y + pad,
                    (card_rect.w - 2.0 * pad).max(0.0),
                    (card_rect.h - 2.0 * pad).max(0.0),
                );
                let btns = entry.buttons(editing.is_some());
                let mut button_rects = Vec::with_capacity(btns.len());
                if !btns.is_empty() {
                    let btn_h = px(26.0).round();
                    let btn_y = inner.bottom() - btn_h;
                    let mut bx = inner.x;
                    for btn in btns {
                        let bw = button_width(btn, &entry.step.kind, entry.expanded, scale);
                        let logical_r =
                            Rect::new(bx, btn_y, bw.min((inner.right() - bx).max(0.0)), btn_h);
                        button_rects.push((btn, view.language.mirror(logical_r, inner)));
                        bx = logical_r.right() + px(6.0);
                    }
                }
                let editor_rect = editing.map(|ed| {
                    let lines = ed.editor().line_count().clamp(2, CARD_EDITOR_MAX_LINES);
                    let eh = (lines as f32 * metrics.line_height.max(px(18.0)) + px(12.0)).round();
                    let bottom_reserved = if button_rects.is_empty() {
                        0.0
                    } else {
                        px(32.0)
                    };
                    Rect::new(
                        inner.x,
                        (inner.bottom() - bottom_reserved - eh).max(inner.y + px(22.0)),
                        inner.w,
                        eh,
                    )
                });
                let diff_rect = if editing.is_none()
                    && entry.expanded
                    && matches!(entry.step.kind, StepKind::Patch { .. })
                {
                    let bottom_reserved = if button_rects.is_empty() {
                        0.0
                    } else {
                        px(32.0)
                    };
                    let reason_h = match &entry.step.kind {
                        StepKind::Patch { reason, .. } if !reason.is_empty() => px(20.0),
                        _ => 0.0,
                    };
                    let top = inner.y
                        + px(24.0)
                        + patch_files_height(&entry.step.kind, metrics.line_height.max(px(17.0)))
                        + reason_h
                        + px(4.0);
                    let dh = (inner.bottom() - bottom_reserved - top - px(6.0)).max(0.0);
                    (dh > 0.0).then_some(Rect::new(inner.x, top, inner.w, dh))
                } else {
                    None
                };

                cards.push(CardLayout {
                    step_id: entry.step.id,
                    rect: card_rect,
                    strip,
                    inner,
                    buttons: button_rects,
                    editor_rect,
                    diff_rect,
                });
                y += h + gap;
            }

            if !view.pinned_to_bottom && max_scroll > 1.0 && body.h > px(48.0) {
                let pill_w = px(124.0).min(body.w - 2.0 * pad);
                let pill_h = px(26.0).round();
                jump_to_latest = Some(Rect::new(
                    body.x + (body.w - pill_w) * 0.5,
                    body.bottom() - pill_h - px(10.0),
                    pill_w,
                    pill_h,
                ));
            }
        }

        // Context Inspector sheet overlay inside the panel.
        let inspector = view.inspector_open.then(|| {
            let margin = px(12.0);
            let sheet_h = (body.h - margin * 2.0).clamp(px(120.0), px(360.0));
            let sheet = Rect::new(
                body.x + margin,
                (body.bottom() - margin - sheet_h).max(body.y + margin),
                (body.w - 2.0 * margin).max(0.0),
                sheet_h.min(body.h.max(0.0)),
            );
            let sh_header = Rect::new(sheet.x, sheet.y, sheet.w, px(34.0).min(sheet.h));
            let close_s = px(22.0).min(sh_header.h);
            let close_logical = Rect::new(
                sh_header.right() - px(8.0) - close_s,
                sh_header.y + (sh_header.h - close_s) * 0.5,
                close_s,
                close_s,
            );
            let close_btn = view.language.mirror(close_logical, sh_header);
            let mut items = Vec::new();
            let row_h = px(30.0).round();
            let mut ry = sh_header.bottom() + px(6.0);
            for idx in 0..view.context.items.len() {
                if ry + row_h > sheet.bottom() - px(6.0) {
                    break;
                }
                let row = Rect::new(sheet.x + px(8.0), ry, (sheet.w - px(16.0)).max(0.0), row_h);
                let cb_s = px(16.0);
                let cb_logical =
                    Rect::new(row.x + px(6.0), row.y + (row.h - cb_s) * 0.5, cb_s, cb_s);
                let cb = view.language.mirror(cb_logical, row);
                items.push((idx, row, cb));
                ry += row_h + px(2.0);
            }
            InspectorLayout {
                sheet,
                header: sh_header,
                close_button: close_btn,
                items,
            }
        });

        Self {
            panel,
            divider_hit,
            border_line,
            header,
            close_button,
            stop_button,
            usage_pill,
            title_area,
            body,
            jump_to_latest,
            empty_chips,
            empty_intro,
            cards,
            footer,
            context_strip,
            composer_box,
            send_button,
            inspector,
            max_scroll,
            scroll_px,
            mirrored,
        }
    }

    pub fn hit(&self, x: f64, y: f64) -> AgentPanelHit {
        if self.divider_hit.contains(x, y) {
            return AgentPanelHit::Divider;
        }
        if !self.panel.contains(x, y) {
            return AgentPanelHit::Outside;
        }
        if self.close_button.contains(x, y) {
            return AgentPanelHit::Close;
        }
        if self.stop_button.is_some_and(|r| r.contains(x, y)) {
            return AgentPanelHit::Stop;
        }
        if let Some(insp) = &self.inspector {
            if insp.close_button.contains(x, y) {
                return AgentPanelHit::InspectorClose;
            }
            if insp.sheet.contains(x, y) {
                for &(idx, row, _) in &insp.items {
                    if row.contains(x, y) {
                        return AgentPanelHit::InspectorToggle(idx);
                    }
                }
                return AgentPanelHit::Inside;
            }
            if self.body.contains(x, y) {
                return AgentPanelHit::InspectorBackdrop;
            }
        }
        if self.context_strip.contains(x, y) {
            return AgentPanelHit::ContextStrip;
        }
        if self.send_button.contains(x, y) {
            return AgentPanelHit::Send;
        }
        if self.composer_box.contains(x, y) {
            return AgentPanelHit::Composer;
        }
        if self.jump_to_latest.is_some_and(|r| r.contains(x, y)) {
            return AgentPanelHit::JumpToLatest;
        }
        if self.body.contains(x, y) {
            for (idx, chip) in self.empty_chips.iter().enumerate() {
                if chip.contains(x, y) {
                    return AgentPanelHit::ExampleChip(idx);
                }
            }
            for card in &self.cards {
                if card.rect.bottom() < self.body.y || card.rect.y > self.body.bottom() {
                    continue;
                }
                if card.rect.contains(x, y) {
                    for &(btn, rect) in &card.buttons {
                        if rect.contains(x, y) {
                            return AgentPanelHit::Card {
                                step: card.step_id,
                                button: Some(btn),
                            };
                        }
                    }
                    return AgentPanelHit::Card {
                        step: card.step_id,
                        button: None,
                    };
                }
            }
        }
        AgentPanelHit::Inside
    }
}

fn button_width(btn: CardButton, kind: &StepKind, expanded: bool, scale: f32) -> f32 {
    let logical = match btn {
        CardButton::Primary => match kind {
            StepKind::Command { .. } => 60.0,
            StepKind::Patch { .. } => 64.0,
            StepKind::FactSuggestion { .. } => 86.0,
            StepKind::LimitReached { .. } => 80.0,
            _ => 64.0,
        },
        CardButton::Edit => 54.0,
        CardButton::Deny => match kind {
            StepKind::FactSuggestion { .. } => 72.0,
            StepKind::LimitReached { .. } => 56.0,
            _ => 58.0,
        },
        CardButton::Expand => {
            if expanded {
                76.0
            } else {
                68.0
            }
        }
        CardButton::OpenBlock => 88.0,
        CardButton::Undo => 60.0,
        CardButton::Copy => 62.0,
        CardButton::ApplyEdit => 96.0,
        CardButton::CancelEdit => 68.0,
    };
    (logical * scale).round()
}

fn patch_files_height(kind: &StepKind, row_h: f32) -> f32 {
    if let StepKind::Patch { files, .. } = kind {
        (files.len().clamp(1, 6) as f32) * row_h
    } else {
        0.0
    }
}

/// Estimate wrapped line count for a piece of text inside `max_w` physical pixels.
/// Greedy word wrap by an estimated character width (UI text, not terminal cells).
pub fn wrap_words(text: &str, max_w: f32, char_w: f32, max_lines: usize) -> Vec<String> {
    let cols = (max_w / char_w.max(6.0)).floor().max(12.0) as usize;
    let mut out = Vec::new();
    for para in text.lines() {
        if para.trim().is_empty() {
            out.push(String::new());
            if out.len() >= max_lines {
                return out;
            }
            continue;
        }
        let mut cur = String::new();
        let mut cur_len = 0usize;
        for word in para.split_whitespace() {
            let wlen = word.chars().count();
            if cur_len > 0 && cur_len + 1 + wlen > cols {
                out.push(std::mem::take(&mut cur));
                cur_len = 0;
                if out.len() >= max_lines {
                    return out;
                }
            }
            if cur_len > 0 {
                cur.push(' ');
                cur_len += 1;
            }
            cur.push_str(word);
            cur_len += wlen;
        }
        if !cur.is_empty() {
            out.push(cur);
            if out.len() >= max_lines {
                return out;
            }
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

pub fn estimate_wrapped_lines(text: &str, max_w: f32, char_w: f32, max_lines: usize) -> usize {
    let cols = (max_w / char_w.max(6.0)).floor().max(12.0) as usize;
    let mut total = 0usize;
    for raw_line in text.lines() {
        let len = raw_line.chars().count();
        total += (len.max(1)).div_ceil(cols);
        if total >= max_lines {
            return max_lines;
        }
    }
    total.max(1).min(max_lines)
}

fn measure_card_height(
    entry: &StepEntry,
    editing: Option<&CardEditMode>,
    card_w: f32,
    metrics: CellMetrics,
    scale: f32,
) -> f32 {
    let px = |l: f32| l * scale;
    let pad = px(12.0);
    let inner_w = (card_w - 2.0 * pad).max(px(120.0));
    let line_h = px(18.0);
    let mono_lh = metrics.line_height.max(px(17.0));
    let title_h = px(20.0);
    let btn_row = if entry.buttons(editing.is_some()).is_empty() {
        0.0
    } else {
        px(34.0)
    };

    let content_h = if let Some(ed) = editing {
        let lines = ed.editor().line_count().clamp(2, CARD_EDITOR_MAX_LINES);
        title_h + px(8.0) + lines as f32 * mono_lh + px(14.0)
    } else {
        match &entry.step.kind {
            StepKind::Plan { text } => {
                let cap = if entry.expanded {
                    24
                } else {
                    PLAN_COLLAPSED_LINES
                };
                let lines = estimate_wrapped_lines(text, inner_w, px(7.2), cap);
                title_h + px(4.0) + lines as f32 * line_h
            }
            StepKind::Read { preview, .. } => {
                let extra = if entry.expanded && !preview.is_empty() {
                    let lines = preview.lines().count().clamp(1, 8);
                    px(8.0) + lines as f32 * mono_lh + px(8.0)
                } else {
                    0.0
                };
                title_h + px(18.0) + extra
            }
            StepKind::ReadApproval { .. } => title_h + px(18.0),
            StepKind::Command {
                command,
                reason,
                after_untrusted_data,
                ..
            } => {
                let cmd_lines = estimate_wrapped_lines(
                    command,
                    inner_w - px(12.0),
                    metrics.char_width.max(px(7.5)),
                    4,
                );
                let cmd_box = cmd_lines as f32 * mono_lh + px(12.0);
                let reason_h = if reason.is_empty() { 0.0 } else { px(20.0) };
                let untrusted_h = if *after_untrusted_data { px(20.0) } else { 0.0 };
                title_h + px(4.0) + cmd_box + px(4.0) + mono_lh + reason_h + untrusted_h
            }
            StepKind::Patch {
                files,
                diff,
                reason,
            } => {
                let files_h = (files.len().clamp(1, 6) as f32) * mono_lh;
                let reason_h = if reason.is_empty() { 0.0 } else { px(20.0) };
                let diff_h = if entry.expanded {
                    let parsed = parse_diff(diff.as_bytes(), false, &DiffLimits::default());
                    let rows = parsed
                        .files
                        .iter()
                        .map(|f| 1 + f.hunks.iter().map(|h| 1 + h.lines.len()).sum::<usize>())
                        .sum::<usize>()
                        .clamp(1, MAX_PATCH_DIFF_ROWS);
                    px(12.0) + rows as f32 * mono_lh + px(8.0)
                } else {
                    0.0
                };
                title_h + px(4.0) + files_h + reason_h + diff_h
            }
            StepKind::CommandResult { tail, .. } => {
                let tail_h = if tail.trim().is_empty() {
                    0.0
                } else {
                    let lines = tail.lines().count().clamp(1, 6);
                    px(6.0) + lines as f32 * mono_lh + px(10.0)
                };
                title_h + tail_h
            }
            StepKind::PatchApplied { files, .. } => {
                title_h + px(4.0) + (files.len().clamp(1, 5) as f32) * mono_lh
            }
            StepKind::FactSuggestion { text } => {
                let lines = estimate_wrapped_lines(text, inner_w, px(7.2), 4);
                title_h + px(4.0) + lines as f32 * line_h
            }
            StepKind::Message { text } => {
                let lines = estimate_wrapped_lines(text, inner_w, px(7.2), 18);
                title_h + px(4.0) + lines as f32 * line_h
            }
            StepKind::Denied { what, .. } => {
                let what_lines =
                    estimate_wrapped_lines(what, inner_w, metrics.char_width.max(px(7.5)), 3);
                title_h + px(4.0) + what_lines as f32 * mono_lh + px(22.0)
            }
            StepKind::LimitReached { what } => {
                let lines = estimate_wrapped_lines(what, inner_w, px(7.2), 3);
                title_h + px(4.0) + lines as f32 * line_h
            }
        }
    };

    (2.0 * pad + content_h + btn_row).round()
}
