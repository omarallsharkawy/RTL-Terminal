//! Window-free state machine, layout geometry, hit testing, and scripted engine abstraction for
//! the Twitty Agent side panel (`docs/design/AGENT.md` §1).
//!
//! Consumes [`twitty_ai::engine::model::AgentEvent`]s and produces
//! [`twitty_ai::engine::model::AgentCommand`]s against the shared contract in
//! `twitty_ai::engine::model`. Drawing lives in `render::scene::agent`; app/PTY hooks live in
//! `app::agent_panel`.

mod engine_bridge;
mod layout;
mod session;
mod state;

use twitty_ai::engine::model::{ContextBundle, ContextKind, DenyReason, ReadTool, StepKind, Usage};
use twitty_core::en;
use twitty_core::language::fill;
use unicode_bidi::{bidi_class, BidiClass};

use crate::layout::Rect;
use crate::ui::chrome::UiLanguage;

pub use engine_bridge::{
    spawn_placeholder_engine, AgentEngineSpawner, ScriptedEngine, SpawnEngineFn,
};
pub use layout::{
    wrap_words, AgentPanelHit, AgentPanelLayout, AgentPanelView, CardLayout, InspectorLayout,
};
pub use session::{AgentSession, CardButton, CardEditMode, StepEntry};
pub use state::{AgentFocus, AgentPanelAction, AgentPanelState};

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
pub(super) const MAX_HISTORY: usize = 100;

/// One empty-state example chip (AGENT.md §1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExampleChip {
    /// English; `label` translates it.
    pub prompt: &'static str,
}

impl ExampleChip {
    pub fn label(self, language: UiLanguage) -> &'static str {
        language.t(self.prompt)
    }
}

/// Empty-state intro text (AGENT.md §1.1), wrapped by the layout so the chips sit below it.
pub const EMPTY_INTRO: &str = en!(
    "Ask the agent to read files, run commands, or edit code. Every command and patch asks first."
);
/// Logical px from the body top to the first intro line, and between intro lines.
pub const EMPTY_INTRO_TOP: f32 = 44.0;
pub const EMPTY_INTRO_LINE_H: f32 = 17.0;
pub(super) const EMPTY_INTRO_MAX_LINES: usize = 5;

/// Three bilingual empty-state starter chips (AGENT.md §1.1).
pub const EXAMPLE_CHIPS: [ExampleChip; 3] = [
    ExampleChip {
        prompt: en!("Run the tests and fix the first failure"),
    },
    ExampleChip {
        prompt: en!("Explain the last error"),
    },
    ExampleChip {
        prompt: en!("Summarize uncommitted changes"),
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
        DenyReason::HardDeny { rule } => fill(language.t("Denied by policy ({})"), &[rule]),
        DenyReason::OutsideProject { path } => {
            fill(language.t("Outside project root: {}"), &[&path.display()])
        }
        DenyReason::User => language.t("Denied by user").to_owned(),
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
