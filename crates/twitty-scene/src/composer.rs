//! Command composer CPU-side layout and rendering.
//!
//! Draws a rounded Warp-style card docked at the bottom of the focused pane using `UiStyle`
//! tokens, with multi-line syntax-highlighted text shaped by `RowShaper` (the exact same bidi
//! rules as the terminal grid), visual caret placement, RTL-accurate selection highlights,
//! wavy lint underlines and a bilingual diagnostic tooltip line, inline IME preedit, faint ghost
//! suggestion, and an optional completion popup above the box.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::Color as AnsiColor;
use glyphon::TextBounds;
use unicode_bidi::{bidi_class, BidiClass};
use unicode_width::UnicodeWidthChar;

use super::context::DrawContext;
use super::preedit::ImeArea;
use twitty_core::color::Rgba;
use twitty_core::layout::Rect;
use twitty_core::terminal::{CellData, LineData, MAX_DRAWN_MARKS};
use twitty_render::icons::Icon;
use twitty_render::metrics::CellMetrics;
use twitty_render::row::{CachedRow, RowCursor};
use twitty_render::shapes::{Border, Shadow, Shape};
use twitty_render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use twitty_render::ui_style::{
    UiStyle, OVERLAY_BLUR, OVERLAY_OFFSET, OVERLAY_SHADOW, RADIUS_12, RADIUS_6, RADIUS_8, SPACE_12,
    SPACE_4, SPACE_8,
};
use twitty_ui::composer::complete::CompletionKind;
use twitty_ui::composer::highlight::{line_highlight_spans, HighlightSpan, TokenKind};
use twitty_ui::composer::lint::{active_lint_for_caret, LintMark, LintSeverity};
use twitty_ui::ui::chrome::UiLanguage;

/// Maximum editor lines shown simultaneously before scrolling inside the composer box.
pub const MAX_VISIBLE_LINES: usize = 6;
/// Cap wavy underline segments per lint mark to avoid excessive shape geometry on huge lines.
const MAX_WAVE_SEGMENTS: usize = 120;

/// One item in the composer's completion popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerPopupItem {
    pub kind: CompletionKind,
    pub label: String,
    pub detail: Option<String>,
}

/// Visible slice of the completion popup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerPopupView {
    pub items: Vec<ComposerPopupItem>,
    pub selected: usize,
}

/// Status banner shown inside the composer during or after AI command generation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ComposerAiBanner {
    #[default]
    None,
    /// Waiting on the non-interactive coding agent (`phase` animates the spinner dots).
    Loading { phase: usize },
    /// A generated command is in the editor awaiting review (never auto-run).
    Ready { explanation: String },
    /// Bilingual error message from the last AI attempt.
    Error { message: String },
}

/// Built-in AI UI state rendered inside the command composer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComposerAiView {
    pub enabled: bool,
    pub banner: ComposerAiBanner,
}

impl ComposerAiView {
    pub fn has_banner(&self) -> bool {
        !matches!(self.banner, ComposerAiBanner::None)
    }
}

/// Everything the renderer needs to draw the docked command composer.
#[derive(Clone, Debug, PartialEq)]
pub struct ComposerView {
    /// Grid rect of the focused pane (physical pixels).
    pub pane_rect: Rect,
    /// Logical lines of the editor buffer (at least one).
    pub lines: Vec<String>,
    /// Caret `(line_idx, byte_offset_within_line)`.
    pub caret: (usize, usize),
    /// Active selection `((start_line, start_byte), (end_line, end_byte))`.
    pub selection: Option<((usize, usize), (usize, usize))>,
    /// Active IME composition to render inline at the caret.
    pub preedit: Option<(String, Option<(usize, usize)>)>,
    /// Faint inline ghost suffix rendered after the caret line when at end of buffer.
    pub ghost: Option<String>,
    /// Syntax highlight spans covering byte offsets in the full buffer (`lines.join("\n")`).
    pub highlights: Vec<HighlightSpan>,
    /// Pre-run lint marks covering byte offsets in the full buffer.
    pub lints: Vec<LintMark>,
    /// Whether the composer owns keyboard focus.
    pub focused: bool,
    /// Cursor blink phase.
    pub cursor_blink_on: bool,
    /// Optional completion popup above the box.
    pub popup: Option<ComposerPopupView>,
    /// System UI language for chrome hints, descriptions, and lint tooltips.
    pub language: UiLanguage,
    /// Built-in AI button and thinking/review/error banner.
    pub ai: ComposerAiView,
    /// Where the command will run, shown in the header: `~/src/app · main`.
    pub context: Option<String>,
}

/// Pointer hit test target on the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerHit {
    Outside,
    Card,
    Input,
    GenerateButton,
    Popup,
    PopupItem(usize),
}

/// Shared drawing and hit-testing geometry for the composer card and popup (physical pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct ComposerLayout {
    pub card: Rect,
    pub header: Rect,
    pub generate_button: Option<Rect>,
    pub input: Rect,
    /// Optional status banner below `input` during/after AI command generation.
    pub banner: Option<Rect>,
    /// Optional diagnostic tooltip strip below `input` when pre-run lint marks exist.
    pub lint_bar: Option<Rect>,
    pub popup: Option<Rect>,
    pub popup_row_height: f32,
    pub popup_rows: usize,
    pub visible_lines: usize,
    pub line_height: f32,
    pub scroll_line: usize,
}

impl ComposerLayout {
    pub fn new(
        pane_rect: Rect,
        metrics: CellMetrics,
        scale: f32,
        line_count: usize,
        caret_line: usize,
        popup_items: usize,
    ) -> Self {
        Self::with_ai_and_lint(
            pane_rect,
            metrics,
            scale,
            line_count,
            caret_line,
            popup_items,
            false,
            false,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_ai(
        pane_rect: Rect,
        metrics: CellMetrics,
        scale: f32,
        line_count: usize,
        caret_line: usize,
        popup_items: usize,
        ai_enabled: bool,
        has_banner: bool,
    ) -> Self {
        Self::with_ai_and_lint(
            pane_rect,
            metrics,
            scale,
            line_count,
            caret_line,
            popup_items,
            ai_enabled,
            has_banner,
            false,
        )
    }

    pub fn with_lint(
        pane_rect: Rect,
        metrics: CellMetrics,
        scale: f32,
        line_count: usize,
        caret_line: usize,
        popup_items: usize,
        has_lint: bool,
    ) -> Self {
        Self::with_ai_and_lint(
            pane_rect,
            metrics,
            scale,
            line_count,
            caret_line,
            popup_items,
            false,
            false,
            has_lint,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_ai_and_lint(
        pane_rect: Rect,
        metrics: CellMetrics,
        scale: f32,
        line_count: usize,
        caret_line: usize,
        popup_items: usize,
        ai_enabled: bool,
        has_banner: bool,
        has_lint: bool,
    ) -> Self {
        let margin = (SPACE_8 * scale)
            .min(pane_rect.w / 8.0)
            .min(pane_rect.h / 8.0);
        let card_w = (pane_rect.w - 2.0 * margin).max(0.0);
        let pad_x = (SPACE_12 * scale).min(card_w / 4.0);
        let pad_y = (SPACE_8 * scale).round();
        let line_height = metrics.line_height.max(1.0);
        let show_header = pane_rect.h >= line_height * 3.5;
        let header_h = if show_header {
            (20.0 * scale).round()
        } else {
            0.0
        };
        let header_gap = if show_header {
            (SPACE_4 * scale).round()
        } else {
            0.0
        };
        let show_banner = has_banner && pane_rect.h >= line_height * 3.0;
        let banner_h = if show_banner {
            (22.0 * scale).round()
        } else {
            0.0
        };
        let banner_gap = if show_banner {
            (SPACE_4 * scale).round()
        } else {
            0.0
        };
        let show_lint = has_lint && pane_rect.h >= line_height * 4.5;
        let lint_gap = if show_lint {
            (SPACE_4 * scale).round()
        } else {
            0.0
        };
        let lint_h = if show_lint {
            (18.0 * scale).round()
        } else {
            0.0
        };

        let footer_h = banner_h + banner_gap + lint_h + lint_gap;
        let max_card_h = (pane_rect.h * 0.48).max(line_height + 2.0 * pad_y + footer_h);
        let max_card_h = max_card_h.min((pane_rect.h - 2.0 * margin).max(0.0));
        let chrome_h = header_h + header_gap + footer_h + 2.0 * pad_y;
        let max_lines = (((max_card_h - chrome_h).max(line_height)) / line_height)
            .floor()
            .clamp(1.0, MAX_VISIBLE_LINES as f32) as usize;
        let line_count = line_count.max(1);
        let visible_lines = line_count.min(max_lines);
        let input_h = visible_lines as f32 * line_height;
        let card_h = (chrome_h + input_h).min((pane_rect.h - margin).max(0.0));

        let card_x = (pane_rect.x + margin).round();
        let card_y = (pane_rect.bottom() - margin - card_h)
            .max(pane_rect.y)
            .round();
        let card = Rect::new(card_x, card_y, card_w.round(), card_h.round());

        let inner_w = (card.w - 2.0 * pad_x).max(0.0);
        let header = Rect::new(card.x + pad_x, card.y + pad_y, inner_w, header_h);
        let generate_button = if ai_enabled && show_header && header.w >= (110.0 * scale) {
            let btn_w = if header.w >= (320.0 * scale) {
                (118.0 * scale).round()
            } else {
                (76.0 * scale).round()
            }
            .min(header.w * 0.45);
            let btn_h = header.h;
            Some(Rect::new(header.right() - btn_w, header.y, btn_w, btn_h))
        } else {
            None
        };

        let input_y = if show_header {
            header.bottom() + header_gap
        } else {
            card.y + pad_y
        };
        let input_bottom_limit = if show_banner || show_lint {
            (card.bottom() - pad_y - footer_h).max(input_y)
        } else {
            (card.bottom() - pad_y).max(input_y)
        };
        let input = Rect::new(
            card.x + pad_x,
            input_y,
            inner_w,
            input_h.min((input_bottom_limit - input_y).max(0.0)),
        );
        let mut cursor_y = input.bottom();
        let lint_bar = if show_lint && inner_w > 0.0 {
            let ly = (cursor_y + lint_gap).round();
            let lh = lint_h.min((card.bottom() - pad_y - ly).max(0.0));
            if lh > 0.0 {
                let rect = Rect::new(card.x + pad_x, ly, inner_w, lh);
                cursor_y = rect.bottom();
                Some(rect)
            } else {
                None
            }
        } else {
            None
        };
        let banner = if show_banner && inner_w > 0.0 {
            let by = (cursor_y + banner_gap).round();
            let bh = banner_h.min((card.bottom() - pad_y - by).max(0.0));
            (bh > 0.0).then_some(Rect::new(card.x + pad_x, by, inner_w, bh))
        } else {
            None
        };

        let scroll_line = if caret_line >= visible_lines {
            caret_line + 1 - visible_lines
        } else {
            0
        };

        let popup_row_height = (26.0 * scale).round().max(1.0);
        let popup_pad = (SPACE_4 * scale).round();
        let popup_gap = (SPACE_4 * scale).round();
        let avail_above = (card.y - pane_rect.y - margin - popup_gap).max(0.0);
        let max_popup_rows =
            ((avail_above - 2.0 * popup_pad).max(0.0) / popup_row_height).floor() as usize;
        let popup_rows = popup_items.min(max_popup_rows);
        let popup = if popup_rows > 0 && card.w > 0.0 {
            let popup_h = popup_rows as f32 * popup_row_height + 2.0 * popup_pad;
            let popup_y = (card.y - popup_gap - popup_h).max(pane_rect.y).round();
            Some(Rect::new(card.x, popup_y, card.w, popup_h.round()))
        } else {
            None
        };

        Self {
            card,
            header,
            generate_button,
            input,
            banner,
            lint_bar,
            popup,
            popup_row_height,
            popup_rows,
            visible_lines,
            line_height,
            scroll_line,
        }
    }

    pub fn popup_row(&self, index: usize) -> Option<Rect> {
        let popup = self.popup?;
        if index >= self.popup_rows {
            return None;
        }
        let pad = ((popup.h - self.popup_rows as f32 * self.popup_row_height) / 2.0).max(0.0);
        Some(Rect::new(
            popup.x + pad,
            popup.y + pad + index as f32 * self.popup_row_height,
            (popup.w - 2.0 * pad).max(0.0),
            self.popup_row_height,
        ))
    }

    pub fn hit(&self, x: f64, y: f64) -> ComposerHit {
        if let Some(popup) = self.popup {
            if popup.contains(x, y) {
                for i in 0..self.popup_rows {
                    if self.popup_row(i).is_some_and(|r| r.contains(x, y)) {
                        return ComposerHit::PopupItem(i);
                    }
                }
                return ComposerHit::Popup;
            }
        }
        if self.generate_button.is_some_and(|b| b.contains(x, y)) {
            ComposerHit::GenerateButton
        } else if self.input.contains(x, y) {
            ComposerHit::Input
        } else if self.card.contains(x, y) {
            ComposerHit::Card
        } else {
            ComposerHit::Outside
        }
    }
}

/// One shaped editor line inside the composer.
pub struct ComposerText {
    pub row: CachedRow,
    pub left: f32,
    pub top: f32,
    pub bounds: TextBounds,
}

/// CPU-side scene output for the composer, attached to `Frame`.
pub struct ComposerScene {
    pub shapes: Vec<Shape>,
    pub texts: Vec<ComposerText>,
    pub labels: Vec<UiLabel>,
    /// Rectangles covered by the composer (card + optional popup) whose underlying terminal text
    /// must be clipped.
    pub clips: Vec<Rect>,
    /// Caret rectangle in physical pixels for OS IME candidate window positioning.
    pub ime_area: Option<ImeArea>,
}

/// Result of shaping one composer line with `RowShaper`, exposed for headless visual-caret tests.
pub struct ShapedComposerLine {
    pub row: CachedRow,
    /// Visual caret X relative to the line's left origin.
    pub caret_x: f32,
    /// True when the caret sits in a right-to-left run (extends left from `caret_x`).
    pub extends_left: bool,
    /// Cell index for each UTF-8 byte offset `0..=line_text.len()`.
    pub byte_to_cell: Vec<usize>,
    /// Cell index range of the spliced IME preedit, if any.
    pub preedit_cells: Option<(usize, usize)>,
}

/// Shape one logical composer line using Twitty's `RowShaper` (identical bidi rules to the
/// terminal grid) and compute the exact visual caret anchor.
pub fn shape_composer_line(
    ctx: &mut DrawContext,
    line_text: &str,
    caret_byte: Option<usize>,
    fg: Rgba,
    preedit: Option<(&str, Option<(usize, usize)>)>,
    ghost: Option<(&str, Rgba)>,
) -> ShapedComposerLine {
    let style = UiStyle::new(ctx.palette, None, ctx.metrics.scale);
    shape_composer_line_highlighted(ctx, line_text, caret_byte, fg, &[], style, preedit, ghost)
}

/// Shape one logical composer line with per-token syntax highlighting spans.
///
/// Adjacent characters with identical [`TokenKind`] receive identical `fg` and flags so `RowShaper`
/// merges them into a single shaping span and preserves Arabic joining and bidi order.
#[allow(clippy::too_many_arguments)]
pub fn shape_composer_line_highlighted(
    ctx: &mut DrawContext,
    line_text: &str,
    caret_byte: Option<usize>,
    fg: Rgba,
    line_spans: &[HighlightSpan],
    style: UiStyle,
    preedit: Option<(&str, Option<(usize, usize)>)>,
    ghost: Option<(&str, Rgba)>,
) -> ShapedComposerLine {
    let default_fg = AnsiColor::Spec(rgb8(fg));
    let mut cells: Vec<CellData> = Vec::with_capacity(line_text.len() + 16);
    let mut has_rtl = false;
    let mut byte_to_cell = vec![0usize; line_text.len() + 1];
    let mut preedit_cells = None;
    let mut preedit_caret_cell = None;

    let push_text = |text: &str,
                     color: AnsiColor,
                     flags: CellFlags,
                     cells: &mut Vec<CellData>,
                     has_rtl: &mut bool| {
        for c in text.chars() {
            if c == '\t' {
                for _ in 0..4 {
                    cells.push(CellData {
                        c: ' ',
                        fg: color,
                        flags,
                        ..CellData::blank()
                    });
                }
                continue;
            }
            if matches!(bidi_class(c), BidiClass::R | BidiClass::AL) {
                *has_rtl = true;
            }
            match c.width() {
                Some(0) | None => {
                    if let Some(base) = cells.iter_mut().rev().find(|cell| !cell.is_spacer()) {
                        if base.zerowidth.len() < MAX_DRAWN_MARKS {
                            base.zerowidth.push(c);
                        }
                    }
                }
                Some(2) => {
                    cells.push(CellData {
                        c,
                        fg: color,
                        flags: flags | CellFlags::WIDE_CHAR,
                        ..CellData::blank()
                    });
                    cells.push(CellData {
                        flags: CellFlags::WIDE_CHAR_SPACER,
                        ..CellData::blank()
                    });
                }
                Some(_) => {
                    cells.push(CellData {
                        c,
                        fg: color,
                        flags,
                        ..CellData::blank()
                    });
                }
            }
        }
    };

    let caret_pos = caret_byte.map(|b| b.min(line_text.len()));
    let mut spliced_preedit = false;
    let mut span_idx = 0usize;

    for (byte_idx, c) in line_text.char_indices() {
        if !spliced_preedit && caret_pos == Some(byte_idx) {
            if let Some((pre_text, pre_cursor)) = preedit {
                let start_cell = cells.len();
                let pre_byte = pre_cursor.map_or(pre_text.len(), |(a, _)| a.min(pre_text.len()));
                let pre_byte = clamp_char_boundary(pre_text, pre_byte);
                push_text(
                    &pre_text[..pre_byte],
                    default_fg,
                    CellFlags::UNDERLINE,
                    &mut cells,
                    &mut has_rtl,
                );
                preedit_caret_cell = Some(cells.len());
                push_text(
                    &pre_text[pre_byte..],
                    default_fg,
                    CellFlags::UNDERLINE,
                    &mut cells,
                    &mut has_rtl,
                );
                preedit_cells = Some((start_cell, cells.len()));
                spliced_preedit = true;
            }
        }
        while span_idx < line_spans.len() && line_spans[span_idx].end <= byte_idx {
            span_idx += 1;
        }
        let (char_color, char_flags) = match line_spans.get(span_idx) {
            Some(span) if byte_idx >= span.start && byte_idx < span.end => {
                let col = AnsiColor::Spec(rgb8(span.kind.color(ctx.palette, style)));
                let mut fl = CellFlags::empty();
                if span.kind.italic() {
                    fl |= CellFlags::ITALIC;
                }
                (col, fl)
            }
            _ => (default_fg, CellFlags::empty()),
        };

        let cell_start = cells.len();
        for slot in &mut byte_to_cell[byte_idx..byte_idx + c.len_utf8()] {
            *slot = cell_start;
        }
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        push_text(s, char_color, char_flags, &mut cells, &mut has_rtl);
    }

    let end_cell_before_preedit = cells.len();
    byte_to_cell[line_text.len()] = end_cell_before_preedit;

    if !spliced_preedit && caret_pos == Some(line_text.len()) {
        if let Some((pre_text, pre_cursor)) = preedit {
            let start_cell = cells.len();
            let pre_byte = pre_cursor.map_or(pre_text.len(), |(a, _)| a.min(pre_text.len()));
            let pre_byte = clamp_char_boundary(pre_text, pre_byte);
            push_text(
                &pre_text[..pre_byte],
                default_fg,
                CellFlags::UNDERLINE,
                &mut cells,
                &mut has_rtl,
            );
            preedit_caret_cell = Some(cells.len());
            push_text(
                &pre_text[pre_byte..],
                default_fg,
                CellFlags::UNDERLINE,
                &mut cells,
                &mut has_rtl,
            );
            preedit_cells = Some((start_cell, cells.len()));
        }
    }

    if let Some((ghost_text, ghost_rgba)) = ghost {
        let ghost_color = AnsiColor::Spec(rgb8(ghost_rgba));
        push_text(
            ghost_text,
            ghost_color,
            CellFlags::empty(),
            &mut cells,
            &mut has_rtl,
        );
    }

    let caret_cell = preedit_caret_cell.or_else(|| caret_pos.map(|b| byte_to_cell[b]));

    let mut hasher = DefaultHasher::new();
    line_text.hash(&mut hasher);
    caret_cell.hash(&mut hasher);
    for sp in line_spans {
        sp.start.hash(&mut hasher);
        sp.end.hash(&mut hasher);
        (sp.kind as u8).hash(&mut hasher);
    }
    let line_data = LineData {
        cells: cells.into(),
        has_rtl,
        hash: hasher.finish(),
    };

    let row_cursor = caret_cell.map(|col| RowCursor { col, block: false });
    let segments = ctx.shaper().build_row(&line_data, row_cursor);
    let row = CachedRow::new(
        line_data.hash,
        &line_data,
        segments,
        Vec::new(),
        ctx.metrics.char_width,
    );
    let (caret_x, extends_left) = caret_cell.map_or((0.0, false), |col| {
        row.cursor_anchor(col, ctx.metrics.char_width)
    });

    ShapedComposerLine {
        row,
        caret_x,
        extends_left,
        byte_to_cell,
        preedit_cells,
    }
}

/// Visual horizontal intervals `(left, right)` covering cells `start_cell..end_cell` (following
/// reordered RTL glyph extents where present).
pub fn cell_range_spans(
    row: &CachedRow,
    start_cell: usize,
    end_cell: usize,
    char_width: f32,
) -> Vec<(f32, f32)> {
    if start_cell >= end_cell {
        return Vec::new();
    }
    let mut intervals: Vec<(f32, f32)> = Vec::new();
    for c in start_cell..end_cell {
        let (l, r) = row
            .visual
            .get(c)
            .and_then(|&v| v)
            .unwrap_or((c as f32 * char_width, (c + 1) as f32 * char_width));
        if r > l {
            intervals.push((l, r));
        }
    }
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f32, f32)> = Vec::new();
    for (l, r) in intervals {
        if let Some(last) = merged.last_mut() {
            if l <= last.1 + 1.0 {
                last.1 = last.1.max(r);
                continue;
            }
        }
        merged.push((l, r));
    }
    merged
}

/// Emit crisp DPI-scaled wavy underline capsules across `[x0, x1]` near `y_base`.
fn push_wavy_underline(
    x0: f32,
    x1: f32,
    y_base: f32,
    scale: f32,
    color: [f32; 4],
    out: &mut Vec<Shape>,
) {
    if x1 <= x0 {
        return;
    }
    let half_wave = (3.0 * scale).max(2.0);
    let amp = (1.25 * scale).max(1.0);
    let stroke = (1.25 * scale).max(1.0);
    let mut x = x0;
    let mut up = true;
    let mut count = 0usize;
    while x < x1 && count < MAX_WAVE_SEGMENTS {
        let next_x = (x + half_wave).min(x1);
        let y0 = if up { y_base + amp } else { y_base - amp };
        let y1 = if up { y_base - amp } else { y_base + amp };
        out.push(Shape::Capsule {
            from: [x, y0],
            to: [next_x, y1],
            width: stroke,
            color,
        });
        x = next_x;
        up = !up;
        count += 1;
    }
}

/// Build the full CPU-side scene for the composer card, syntax-highlighted text lines, caret,
/// selection, lint underlines + tooltip line, preedit, ghost suggestion, and completion popup.
pub fn build(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    view: &ComposerView,
) -> ComposerScene {
    let popup_count = view.popup.as_ref().map_or(0, |p| p.items.len());
    let has_lint = !view.lints.is_empty();
    let layout = ComposerLayout::with_ai_and_lint(
        view.pane_rect,
        ctx.metrics,
        style.scale,
        view.lines.len(),
        view.caret.0,
        popup_count,
        view.ai.enabled,
        view.ai.has_banner(),
        has_lint,
    );
    let mut scene = ComposerScene {
        shapes: Vec::new(),
        texts: Vec::new(),
        labels: Vec::new(),
        clips: Vec::new(),
        ime_area: None,
    };
    if layout.card.w <= 0.0 || layout.card.h <= 0.0 {
        return scene;
    }
    scene.clips.push(layout.card);

    // 1. Composer card background + border + shadow.
    let border_color = if view.focused {
        style.accent
    } else {
        style.border
    };
    scene.shapes.push(Shape::RoundedRect {
        rect: layout.card,
        radii: [style.px(RADIUS_12); 4],
        fill: ctx.color(style.bg1),
        border: Some(Border {
            width: style.px(if view.focused { 1.5 } else { 1.0 }),
            color: ctx.color(border_color),
        }),
        shadow: Some(Shadow {
            blur: style.px(OVERLAY_BLUR),
            offset: OVERLAY_OFFSET.map(|v| style.px(v)),
            color: ctx.color(OVERLAY_SHADOW),
        }),
    });

    // 2. Subtle inner input surface.
    let input_bg = Rect::new(
        (layout.input.x - style.px(SPACE_4)).max(layout.card.x + style.px(2.0)),
        (layout.input.y - style.px(2.0)).max(layout.card.y + style.px(2.0)),
        (layout.input.w + 2.0 * style.px(SPACE_4)).min(layout.card.w - style.px(4.0)),
        (layout.input.h + style.px(4.0)).min(layout.card.bottom() - layout.input.y + style.px(2.0)),
    );
    scene.shapes.push(Shape::rounded(
        input_bg,
        style.px(RADIUS_8),
        ctx.color(style.bg2),
    ));

    // 3. Header bar: terminal icon + bilingual title + compact hints + optional AI Generate button.
    let hash_mode = view.ai.enabled
        && view
            .lines
            .first()
            .is_some_and(|l| l.trim_start().starts_with('#'));
    if layout.header.h > 0.0 && layout.header.w > 0.0 {
        let icon_side = style.px(13.0).min(layout.header.h);
        let icon_rect = Rect::new(
            layout.header.x,
            layout.header.y + (layout.header.h - icon_side) / 2.0,
            icon_side,
            icon_side,
        );
        let header_icon = if hash_mode {
            Icon::Agent
        } else {
            Icon::Terminal
        };
        header_icon.draw(icon_rect, ctx.color(style.accent), &mut scene.shapes);

        let header_right = match layout.generate_button {
            Some(btn) => {
                let fill =
                    if hash_mode || matches!(view.ai.banner, ComposerAiBanner::Loading { .. }) {
                        style.accent_tint(style.bg2)
                    } else {
                        style.bg2
                    };
                scene.shapes.push(Shape::RoundedRect {
                    rect: btn,
                    radii: [style.px(RADIUS_6); 4],
                    fill: ctx.color(fill),
                    border: Some(Border {
                        width: style.hairline(),
                        color: ctx.color(if hash_mode {
                            style.accent
                        } else {
                            style.border
                        }),
                    }),
                    shadow: None,
                });
                let bi_side = style.px(11.0).min(btn.h - style.px(4.0)).max(0.0);
                let bi_rect = Rect::new(
                    btn.x + style.px(6.0),
                    btn.y + (btn.h - bi_side) / 2.0,
                    bi_side,
                    bi_side,
                );
                Icon::Agent.draw(bi_rect, ctx.color(style.accent), &mut scene.shapes);
                let label_text = view.language.t("Generate");
                let label_x = bi_rect.right() + style.px(4.0);
                let label_rect = Rect::new(
                    label_x,
                    btn.y,
                    (btn.right() - label_x - style.px(4.0)).max(0.0),
                    btn.h,
                );
                let btn_buf = font.fit(
                    ctx.font_system,
                    UiSize::Small,
                    UiWeight::Semibold,
                    style.scale,
                    label_text,
                    label_rect.w,
                );
                scene.labels.push(UiLabel::new(
                    btn_buf,
                    label_rect,
                    UiAlign::Center,
                    style.text,
                ));
                btn.x - style.px(SPACE_8)
            }
            None => layout.header.right(),
        };

        let title_text = view.language.t("Compose command");
        let full_title = view.context.as_deref().unwrap_or(title_text);
        let title_x = icon_rect.right() + style.px(6.0);
        let avail_w = (header_right - title_x).max(0.0);

        let hint_text = if view.ai.enabled {
            view.language
                .t("Enter Run · # / Ctrl+Enter AI · Tab Complete · Esc Close")
        } else {
            view.language
                .t("Enter Run · Shift+Enter Newline · Tab Complete · Esc Close")
        };
        let hint_buf = font.shape(
            ctx.font_system,
            UiSize::Small,
            UiWeight::Regular,
            style.scale,
            hint_text,
        );
        let hint_w = label_width(&hint_buf).ceil() + style.px(8.0);

        let title_buf_full = font.shape(
            ctx.font_system,
            UiSize::Small,
            UiWeight::Semibold,
            style.scale,
            full_title,
        );
        let title_w_full = label_width(&title_buf_full).ceil() + style.px(8.0);

        if avail_w >= title_w_full + hint_w {
            let title_rect = Rect::new(title_x, layout.header.y, title_w_full, layout.header.h);
            scene.labels.push(UiLabel::new(
                title_buf_full,
                title_rect,
                UiAlign::Left,
                style.text_dim,
            ));
            let hint_rect = Rect::new(
                header_right - hint_w,
                layout.header.y,
                hint_w,
                layout.header.h,
            );
            scene.labels.push(UiLabel::new(
                hint_buf,
                hint_rect,
                UiAlign::Right,
                style.text_faint,
            ));
        } else if avail_w > 0.0 {
            let short_buf = font.fit(
                ctx.font_system,
                UiSize::Small,
                UiWeight::Semibold,
                style.scale,
                full_title,
                avail_w,
            );
            let title_rect = Rect::new(title_x, layout.header.y, avail_w, layout.header.h);
            scene.labels.push(UiLabel::new(
                short_buf,
                title_rect,
                UiAlign::Start,
                style.text_dim,
            ));
        }
    }

    // Precompute byte offsets of every line in the joined buffer (for highlights, lints, caret).
    let mut line_starts = Vec::with_capacity(view.lines.len());
    let mut acc = 0usize;
    for line in &view.lines {
        line_starts.push(acc);
        acc += line.len() + 1;
    }
    let caret_global_byte = line_starts
        .get(view.caret.0)
        .copied()
        .unwrap_or(0)
        .saturating_add(view.caret.1);

    // 4. Editor lines (shaped with RowShaper), syntax highlights, path underlines, lint waves,
    //    selection highlights, IME preedit underline, and caret.
    let all_empty = view.lines.len() == 1 && view.lines[0].is_empty() && view.preedit.is_none();
    let input_bounds = TextBounds {
        left: layout.input.x.floor() as i32,
        top: layout.input.y.floor() as i32,
        right: layout.input.right().ceil() as i32,
        bottom: layout.input.bottom().ceil() as i32,
    };

    let caret_w = style.px(2.0).max(1.5).round();
    if all_empty {
        let placeholder = if view.ai.enabled {
            view.language.t("Type a command or # for AI…")
        } else {
            view.language.t("Type a command…|composer")
        };
        // Start at the input's text inset and leave room for the empty editor's beam.
        // Reserve it during both blink phases so the placeholder never shifts.
        let start = (layout.input.x + caret_w + style.px(2.0)).min(layout.input.right());
        let hint = Rect::new(
            start,
            layout.input.y,
            layout.input.right() - start,
            layout.line_height,
        );
        let ph_buf = font.fit(
            ctx.font_system,
            UiSize::Body,
            UiWeight::Regular,
            style.scale,
            placeholder,
            hint.w,
        );
        scene
            .labels
            .push(UiLabel::new(ph_buf, hint, UiAlign::Left, style.text_faint));
    }

    let visible_end = (layout.scroll_line + layout.visible_lines).min(view.lines.len());
    for (vis_idx, line_idx) in (layout.scroll_line..visible_end).enumerate() {
        let line_text = &view.lines[line_idx];
        let line_start = line_starts[line_idx];
        let line_end = line_start + line_text.len();
        let line_spans = line_highlight_spans(&view.highlights, line_start, line_end);

        let y = layout.input.y + vis_idx as f32 * layout.line_height;
        let is_caret_line = line_idx == view.caret.0;
        let line_caret = is_caret_line.then_some(view.caret.1);
        let line_preedit = is_caret_line
            .then(|| view.preedit.as_ref().map(|(t, c)| (t.as_str(), *c)))
            .flatten();
        let line_ghost = (is_caret_line && line_preedit.is_none() && view.selection.is_none())
            .then(|| view.ghost.as_deref().map(|g| (g, style.text_faint)))
            .flatten();

        let shaped = shape_composer_line_highlighted(
            ctx,
            line_text,
            line_caret,
            style.text,
            &line_spans,
            style,
            line_preedit,
            line_ghost,
        );

        // Horizontal scroll so the caret always stays visible inside `layout.input`.
        let scroll_x = if is_caret_line && shaped.caret_x > layout.input.w - caret_w {
            (shaped.caret_x - (layout.input.w - caret_w - ctx.metrics.char_width)).max(0.0)
        } else {
            0.0
        };
        let origin_x = layout.input.x - scroll_x;

        // Selection highlight spans for this line.
        if let Some(((sl, sb), (el, eb))) = view.selection {
            if line_idx >= sl && line_idx <= el {
                let start_byte = if line_idx == sl {
                    sb.min(line_text.len())
                } else {
                    0
                };
                let end_byte = if line_idx == el {
                    eb.min(line_text.len())
                } else {
                    line_text.len()
                };
                let start_cell = shaped.byte_to_cell[start_byte];
                let end_cell = shaped.byte_to_cell[end_byte];
                let sel_color = ctx.color(style.accent_tint(style.bg2));
                for (l, r) in
                    cell_range_spans(&shaped.row, start_cell, end_cell, ctx.metrics.char_width)
                {
                    let x0 = (origin_x + l).max(layout.input.x);
                    let x1 = (origin_x + r).min(layout.input.right());
                    if x1 > x0 {
                        scene.shapes.push(Shape::rounded(
                            Rect::new(x0, y, x1 - x0, layout.line_height),
                            style.px(2.0),
                            sel_color,
                        ));
                    }
                }
            }
        }

        // Subtle underline for existing filesystem paths.
        let path_ul_stroke = style.hairline();
        let path_ul_color = ctx.color(TokenKind::ExistingPath.color(ctx.palette, style));
        for span in &line_spans {
            if !span.kind.underline() {
                continue;
            }
            let sc = shaped.byte_to_cell[span.start.min(line_text.len())];
            let ec = shaped.byte_to_cell[span.end.min(line_text.len())];
            for (l, r) in cell_range_spans(&shaped.row, sc, ec, ctx.metrics.char_width) {
                let x0 = (origin_x + l).max(layout.input.x);
                let x1 = (origin_x + r).min(layout.input.right());
                if x1 > x0 {
                    scene.shapes.push(Shape::rounded(
                        Rect::new(
                            x0,
                            y + layout.line_height - path_ul_stroke - style.px(1.0),
                            x1 - x0,
                            path_ul_stroke,
                        ),
                        0.0,
                        path_ul_color,
                    ));
                }
            }
        }

        // Wavy underlines for pre-run lint marks overlapping this line.
        let wave_y = y + layout.line_height - style.px(2.0);
        for mark in &view.lints {
            if mark.end <= line_start || mark.start > line_end {
                continue;
            }
            if mark.start == line_end && mark.end == line_end {
                continue;
            }
            let rel_s = mark.start.saturating_sub(line_start).min(line_text.len());
            let rel_e = mark.end.saturating_sub(line_start).min(line_text.len());
            let sc = shaped.byte_to_cell[rel_s];
            let ec = shaped.byte_to_cell[rel_e].max(sc + 1);
            let color = ctx.color(match mark.severity {
                LintSeverity::Error => style.danger,
                LintSeverity::Warning => style.warn,
            });
            for (l, r) in cell_range_spans(&shaped.row, sc, ec, ctx.metrics.char_width) {
                let x0 = (origin_x + l).max(layout.input.x);
                let x1 = (origin_x + r).min(layout.input.right());
                push_wavy_underline(x0, x1, wave_y, style.scale, color, &mut scene.shapes);
            }
        }

        // Inline IME preedit underline.
        if let Some((ps, pe)) = shaped.preedit_cells {
            let stroke = ctx.metrics.stroke().max(1.0);
            let ul_color = ctx.color(style.accent);
            for (l, r) in cell_range_spans(&shaped.row, ps, pe, ctx.metrics.char_width) {
                let x0 = (origin_x + l).max(layout.input.x);
                let x1 = (origin_x + r).min(layout.input.right());
                if x1 > x0 {
                    scene.shapes.push(Shape::rounded(
                        Rect::new(x0, y + layout.line_height - stroke, x1 - x0, stroke),
                        0.0,
                        ul_color,
                    ));
                }
            }
        }

        // Visible beam caret (and IME candidate popup anchor).
        if is_caret_line {
            let raw_cx = if shaped.extends_left {
                origin_x + shaped.caret_x - caret_w
            } else {
                origin_x + shaped.caret_x
            };
            let cx = raw_cx.clamp(
                layout.input.x,
                (layout.input.right() - caret_w).max(layout.input.x),
            );
            scene.ime_area = Some((
                cx.round() as i32,
                y.round() as i32,
                ctx.metrics.char_width.ceil().max(1.0) as u32,
                layout.line_height.ceil().max(1.0) as u32,
            ));
            if view.focused && (view.cursor_blink_on || line_preedit.is_some()) {
                scene.shapes.push(Shape::rounded(
                    Rect::new(
                        cx,
                        y + style.px(1.0),
                        caret_w,
                        (layout.line_height - style.px(2.0)).max(1.0),
                    ),
                    style.px(1.0),
                    ctx.color(style.accent),
                ));
            }
        }

        if !shaped.row.segments.is_empty() {
            let line_bounds = TextBounds {
                left: input_bounds.left,
                top: input_bounds.top.max(y.floor() as i32),
                right: input_bounds.right,
                bottom: input_bounds
                    .bottom
                    .min((y + layout.line_height).ceil() as i32),
            };
            scene.texts.push(ComposerText {
                row: shaped.row,
                left: origin_x,
                top: y,
                bounds: line_bounds,
            });
        }
    }

    // 4b. Pre-run diagnostic tooltip line below the input box.
    if let (Some(bar), Some(mark)) = (
        layout.lint_bar,
        active_lint_for_caret(&view.lints, caret_global_byte),
    ) {
        let color = match mark.severity {
            LintSeverity::Error => style.danger,
            LintSeverity::Warning => style.warn,
        };
        let icon_side = style.px(12.0).min(bar.h);
        let gap = style.px(6.0);
        let (icon_rect, text_rect, align) = match view.language {
            UiLanguage::Arabic => {
                let ix = (bar.right() - icon_side).max(bar.x);
                let ir = Rect::new(ix, bar.y + (bar.h - icon_side) / 2.0, icon_side, icon_side);
                let tr = Rect::new(bar.x, bar.y, (ix - gap - bar.x).max(0.0), bar.h);
                (ir, tr, UiAlign::Right)
            }
            UiLanguage::English => {
                let ir = Rect::new(
                    bar.x,
                    bar.y + (bar.h - icon_side) / 2.0,
                    icon_side,
                    icon_side,
                );
                let tx = ir.right() + gap;
                let tr = Rect::new(tx, bar.y, (bar.right() - tx).max(0.0), bar.h);
                (ir, tr, UiAlign::Left)
            }
        };
        Icon::Warning.draw(icon_rect, ctx.color(color), &mut scene.shapes);
        if text_rect.w > 0.0 {
            let msg = mark.message(view.language);
            let buf = font.fit(
                ctx.font_system,
                UiSize::Small,
                UiWeight::Regular,
                style.scale,
                &msg,
                text_rect.w,
            );
            scene
                .labels
                .push(UiLabel::new(buf, text_rect, align, color));
        }
    }

    // 4c. Optional AI banner below the input area (thinking spinner, explanation review, or error).
    if let Some(banner_rect) = layout.banner {
        let pad = style.px(SPACE_8);
        match &view.ai.banner {
            ComposerAiBanner::None => {}
            ComposerAiBanner::Loading { phase } => {
                scene.shapes.push(Shape::rounded(
                    banner_rect,
                    style.px(RADIUS_6),
                    ctx.color(style.accent_tint(style.bg2)),
                ));
                let dot_r = style.px(2.5).max(1.5);
                let dot_step = style.px(7.0);
                let dot_y = banner_rect.y + banner_rect.h * 0.5;
                let active = phase % 3;
                for i in 0..3 {
                    let dot_x = banner_rect.x + pad + dot_r + i as f32 * dot_step;
                    let color = if i == active {
                        style.accent
                    } else {
                        style.text_faint
                    };
                    scene.shapes.push(Shape::rounded(
                        Rect::new(dot_x - dot_r, dot_y - dot_r, dot_r * 2.0, dot_r * 2.0),
                        dot_r,
                        ctx.color(color),
                    ));
                }
                let text_x = banner_rect.x + pad + 3.0 * dot_step + style.px(4.0);
                let text_rect = Rect::new(
                    text_x,
                    banner_rect.y,
                    (banner_rect.right() - text_x - pad).max(0.0),
                    banner_rect.h,
                );
                let label = twitty_ui::ui::chrome::t("Thinking…");
                let buf = font.fit(
                    ctx.font_system,
                    UiSize::Small,
                    UiWeight::Semibold,
                    style.scale,
                    label,
                    text_rect.w,
                );
                scene
                    .labels
                    .push(UiLabel::new(buf, text_rect, UiAlign::Start, style.text));
            }
            ComposerAiBanner::Ready { explanation } => {
                scene.shapes.push(Shape::rounded(
                    banner_rect,
                    style.px(RADIUS_6),
                    ctx.color(style.accent_tint(style.bg2)),
                ));
                let icon_side = style.px(12.0).min(banner_rect.h - style.px(4.0)).max(0.0);
                let icon_rect = Rect::new(
                    banner_rect.x + pad,
                    banner_rect.y + (banner_rect.h - icon_side) / 2.0,
                    icon_side,
                    icon_side,
                );
                Icon::Agent.draw(icon_rect, ctx.color(style.accent), &mut scene.shapes);

                let review_hint = view.language.t("Enter Run · Esc Discard");
                let hint_buf = font.shape(
                    ctx.font_system,
                    UiSize::Small,
                    UiWeight::Regular,
                    style.scale,
                    review_hint,
                );
                let hint_w = label_width(&hint_buf).ceil() + style.px(8.0);
                let text_x = icon_rect.right() + style.px(6.0);
                let total_w = (banner_rect.right() - text_x - pad).max(0.0);
                let (exp_w, show_hint) = if total_w > hint_w + style.px(80.0) {
                    (total_w - hint_w, true)
                } else {
                    (total_w, false)
                };
                let exp_rect = Rect::new(text_x, banner_rect.y, exp_w, banner_rect.h);
                let exp_buf = font.fit(
                    ctx.font_system,
                    UiSize::Small,
                    UiWeight::Regular,
                    style.scale,
                    explanation,
                    exp_rect.w,
                );
                scene
                    .labels
                    .push(UiLabel::new(exp_buf, exp_rect, UiAlign::Start, style.text));
                if show_hint {
                    let hint_rect = Rect::new(
                        banner_rect.right() - pad - hint_w,
                        banner_rect.y,
                        hint_w,
                        banner_rect.h,
                    );
                    scene.labels.push(UiLabel::new(
                        hint_buf,
                        hint_rect,
                        UiAlign::Right,
                        style.text_dim,
                    ));
                }
            }
            ComposerAiBanner::Error { message } => {
                scene.shapes.push(Shape::rounded(
                    banner_rect,
                    style.px(RADIUS_6),
                    ctx.color(style.bg2.mix(style.danger, 0.16)),
                ));
                let icon_side = style.px(12.0).min(banner_rect.h - style.px(4.0)).max(0.0);
                let icon_rect = Rect::new(
                    banner_rect.x + pad,
                    banner_rect.y + (banner_rect.h - icon_side) / 2.0,
                    icon_side,
                    icon_side,
                );
                Icon::Warning.draw(icon_rect, ctx.color(style.danger), &mut scene.shapes);
                let text_x = icon_rect.right() + style.px(6.0);
                let text_rect = Rect::new(
                    text_x,
                    banner_rect.y,
                    (banner_rect.right() - text_x - pad).max(0.0),
                    banner_rect.h,
                );
                let buf = font.fit(
                    ctx.font_system,
                    UiSize::Small,
                    UiWeight::Regular,
                    style.scale,
                    message,
                    text_rect.w,
                );
                scene
                    .labels
                    .push(UiLabel::new(buf, text_rect, UiAlign::Start, style.text));
            }
        }
    }

    // 5. Optional completion popup above the composer card.
    if let (Some(popup_rect), Some(popup_view)) = (layout.popup, &view.popup) {
        scene.clips.push(popup_rect);
        scene.shapes.push(Shape::RoundedRect {
            rect: popup_rect,
            radii: [style.px(RADIUS_8); 4],
            fill: ctx.color(style.bg1),
            border: Some(Border {
                width: style.px(1.0),
                color: ctx.color(style.border),
            }),
            shadow: Some(Shadow {
                blur: style.px(OVERLAY_BLUR),
                offset: OVERLAY_OFFSET.map(|v| style.px(v)),
                color: ctx.color(OVERLAY_SHADOW),
            }),
        });

        for (i, item) in popup_view.items.iter().take(layout.popup_rows).enumerate() {
            let Some(row) = layout.popup_row(i) else {
                break;
            };
            let selected = i == popup_view.selected;
            if selected {
                scene.shapes.push(Shape::rounded(
                    row,
                    style.px(RADIUS_6),
                    ctx.color(style.accent_tint(style.bg2)),
                ));
            }

            let pad = style.px(SPACE_8);
            let inner = Rect::new(row.x + pad, row.y, (row.w - 2.0 * pad).max(0.0), row.h);

            // Kind badge pill on the right.
            let badge_text = match view.language {
                UiLanguage::Arabic => item.kind.badge_ar(),
                UiLanguage::English => item.kind.badge_en(),
            };
            let badge_buf = font.shape(
                ctx.font_system,
                UiSize::Small,
                UiWeight::Regular,
                style.scale,
                badge_text,
            );
            let badge_w = label_width(&badge_buf).ceil() + style.px(10.0);
            let badge_h = style.px(17.0).min(inner.h);
            let badge_rect = Rect::new(
                inner.right() - badge_w,
                inner.y + (inner.h - badge_h) / 2.0,
                badge_w,
                badge_h,
            );
            let show_badge = badge_w + style.px(40.0) < inner.w;
            if show_badge {
                scene.shapes.push(Shape::rounded(
                    badge_rect,
                    style.px(RADIUS_6),
                    ctx.color(style.bg2),
                ));
                scene.labels.push(UiLabel::new(
                    badge_buf,
                    badge_rect,
                    UiAlign::Center,
                    style.text_dim,
                ));
            }

            let content_w = if show_badge {
                (inner.w - badge_w - style.px(SPACE_8)).max(0.0)
            } else {
                inner.w
            };
            let detail = item.detail.as_deref().filter(|d| !d.is_empty());
            if let Some(desc) = detail {
                // Render primary item label and dimmer short description side by side.
                let full_label_buf = font.shape(
                    ctx.font_system,
                    UiSize::Caption,
                    if selected {
                        UiWeight::Semibold
                    } else {
                        UiWeight::Regular
                    },
                    style.scale,
                    &item.label,
                );
                let natural_w = label_width(&full_label_buf).ceil() + style.px(4.0);
                let gap = style.px(SPACE_12);
                let max_label_w = (content_w * 0.55).max(style.px(48.0)).min(content_w);
                let used_label_w = natural_w.min(max_label_w);
                let label_buf = if natural_w <= max_label_w {
                    full_label_buf
                } else {
                    font.fit(
                        ctx.font_system,
                        UiSize::Caption,
                        if selected {
                            UiWeight::Semibold
                        } else {
                            UiWeight::Regular
                        },
                        style.scale,
                        &item.label,
                        used_label_w,
                    )
                };
                scene.labels.push(UiLabel::new(
                    label_buf,
                    Rect::new(inner.x, inner.y, used_label_w, inner.h),
                    UiAlign::Start,
                    if selected { style.text } else { style.text_dim },
                ));
                let desc_x = inner.x + used_label_w + gap;
                let desc_w = (inner.x + content_w - desc_x).max(0.0);
                if desc_w >= style.px(32.0) {
                    let desc_buf = font.fit(
                        ctx.font_system,
                        UiSize::Small,
                        UiWeight::Regular,
                        style.scale,
                        desc,
                        desc_w,
                    );
                    scene.labels.push(UiLabel::new(
                        desc_buf,
                        Rect::new(desc_x, inner.y, desc_w, inner.h),
                        UiAlign::Start,
                        if selected {
                            style.text_dim
                        } else {
                            style.text_faint
                        },
                    ));
                }
            } else {
                let label_buf = font.fit(
                    ctx.font_system,
                    UiSize::Caption,
                    if selected {
                        UiWeight::Semibold
                    } else {
                        UiWeight::Regular
                    },
                    style.scale,
                    &item.label,
                    content_w,
                );
                scene.labels.push(UiLabel::new(
                    label_buf,
                    Rect::new(inner.x, inner.y, content_w, inner.h),
                    UiAlign::Start,
                    if selected { style.text } else { style.text_dim },
                ));
            }
        }
    }

    scene
}

use crate::helpers::rgb8;

fn clamp_char_boundary(s: &str, mut byte: usize) -> usize {
    byte = byte.min(s.len());
    while byte > 0 && !s.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}
