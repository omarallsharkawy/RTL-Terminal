//! Render snapshot, physical-pixel layout geometry, word-wrapping, and hit testing for the Agent panel.

use twitty_ai::engine::model::{ContextBundle, StepId, StepKind};

use super::session::{AgentSession, CardButton, CardEditMode, StepEntry};
use super::state::AgentFocus;
use super::{
    format_usage_pill, CARD_EDITOR_MAX_LINES, COMPOSER_MAX_LINES, DIVIDER_HIT_PX, EMPTY_INTRO,
    EMPTY_INTRO_LINE_H, EMPTY_INTRO_MAX_LINES, EMPTY_INTRO_TOP, EXAMPLE_CHIPS, HEADER_HEIGHT_PX,
    MAX_PATCH_DIFF_ROWS, PLAN_COLLAPSED_LINES,
};
use crate::composer::editor::TextEditor;
use crate::layout::Rect;
use crate::review::diff::{parse as parse_diff, Limits as DiffLimits};
use crate::ui::chrome::UiLanguage;
use twitty_style::metrics::CellMetrics;

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
                view.language.t(EMPTY_INTRO),
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
                    let parsed_owned;
                    let parsed = match entry.parsed_diff.as_deref() {
                        Some(cached) => cached,
                        None => {
                            parsed_owned =
                                parse_diff(diff.as_bytes(), false, &DiffLimits::default());
                            &parsed_owned
                        }
                    };
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
