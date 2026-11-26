//! Command composer CPU-side layout and rendering.
//!
//! Draws a rounded Warp-style card docked at the bottom of the focused pane using `UiStyle`
//! tokens, with multi-line text shaped by `RowShaper` (the exact same bidi rules as the terminal
//! grid), visual caret placement, RTL-accurate selection highlights, inline IME preedit, faint
//! ghost suggestion, and an optional completion popup above the box.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Rgb};
use glyphon::TextBounds;
use unicode_bidi::{bidi_class, BidiClass};
use unicode_width::UnicodeWidthChar;

use super::context::DrawContext;
use super::preedit::ImeArea;
use crate::color::Rgba;
use crate::composer::complete::CompletionKind;
use crate::layout::Rect;
use crate::render::icons::Icon;
use crate::render::metrics::CellMetrics;
use crate::render::row::{CachedRow, RowCursor};
use crate::render::shapes::{Border, Shadow, Shape};
use crate::render::text::ui::{label_width, UiAlign, UiFont, UiLabel, UiSize, UiWeight};
use crate::render::ui_style::{
    UiStyle, OVERLAY_BLUR, OVERLAY_OFFSET, OVERLAY_SHADOW, RADIUS_12, RADIUS_6, RADIUS_8, SPACE_12,
    SPACE_4, SPACE_8,
};
use crate::terminal::{CellData, LineData, MAX_DRAWN_MARKS};
use crate::ui::chrome::UiLanguage;

/// Maximum editor lines shown simultaneously before scrolling inside the composer box.
pub const MAX_VISIBLE_LINES: usize = 6;

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
    /// Whether the composer owns keyboard focus.
    pub focused: bool,
    /// Cursor blink phase.
    pub cursor_blink_on: bool,
    /// Optional completion popup above the box.
    pub popup: Option<ComposerPopupView>,
    /// System UI language for chrome hints and placeholders.
    pub language: UiLanguage,
}

/// Pointer hit test target on the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerHit {
    Outside,
    Card,
    Input,
    Popup,
    PopupItem(usize),
}

/// Shared drawing and hit-testing geometry for the composer card and popup (physical pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct ComposerLayout {
    pub card: Rect,
    pub header: Rect,
    pub input: Rect,
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

        let max_card_h = (pane_rect.h * 0.45).max(line_height + 2.0 * pad_y);
        let max_card_h = max_card_h.min((pane_rect.h - 2.0 * margin).max(0.0));
        let chrome_h = header_h + header_gap + 2.0 * pad_y;
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
        let input_y = if show_header {
            header.bottom() + header_gap
        } else {
            card.y + pad_y
        };
        let input = Rect::new(
            card.x + pad_x,
            input_y,
            inner_w,
            input_h.min((card.bottom() - input_y).max(0.0)),
        );

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
            input,
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
        if self.input.contains(x, y) {
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
    let fg_color = AnsiColor::Spec(rgb8(fg));
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

    for (byte_idx, c) in line_text.char_indices() {
        if !spliced_preedit && caret_pos == Some(byte_idx) {
            if let Some((pre_text, pre_cursor)) = preedit {
                let start_cell = cells.len();
                let pre_byte = pre_cursor.map_or(pre_text.len(), |(a, _)| a.min(pre_text.len()));
                let pre_byte = clamp_char_boundary(pre_text, pre_byte);
                push_text(
                    &pre_text[..pre_byte],
                    fg_color,
                    CellFlags::UNDERLINE,
                    &mut cells,
                    &mut has_rtl,
                );
                preedit_caret_cell = Some(cells.len());
                push_text(
                    &pre_text[pre_byte..],
                    fg_color,
                    CellFlags::UNDERLINE,
                    &mut cells,
                    &mut has_rtl,
                );
                preedit_cells = Some((start_cell, cells.len()));
                spliced_preedit = true;
            }
        }
        let cell_start = cells.len();
        for slot in &mut byte_to_cell[byte_idx..byte_idx + c.len_utf8()] {
            *slot = cell_start;
        }
        let mut buf = [0u8; 4];
        let s = c.encode_utf8(&mut buf);
        push_text(s, fg_color, CellFlags::empty(), &mut cells, &mut has_rtl);
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
                fg_color,
                CellFlags::UNDERLINE,
                &mut cells,
                &mut has_rtl,
            );
            preedit_caret_cell = Some(cells.len());
            push_text(
                &pre_text[pre_byte..],
                fg_color,
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

/// Build the full CPU-side scene for the composer card, text lines, caret, selection, preedit,
/// ghost suggestion, and optional completion popup.
pub fn build(
    ctx: &mut DrawContext,
    style: UiStyle,
    font: &UiFont,
    view: &ComposerView,
) -> ComposerScene {
    let popup_count = view.popup.as_ref().map_or(0, |p| p.items.len());
    let layout = ComposerLayout::new(
        view.pane_rect,
        ctx.metrics,
        style.scale,
        view.lines.len(),
        view.caret.0,
        popup_count,
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

    // 3. Header bar: terminal icon + bilingual title + compact hints.
    if layout.header.h > 0.0 && layout.header.w > 0.0 {
        let icon_side = style.px(13.0).min(layout.header.h);
        let icon_rect = Rect::new(
            layout.header.x,
            layout.header.y + (layout.header.h - icon_side) / 2.0,
            icon_side,
            icon_side,
        );
        Icon::Terminal.draw(icon_rect, ctx.color(style.accent), &mut scene.shapes);

        let title_text = match view.language {
            UiLanguage::Arabic => "اكتب أمر",
            UiLanguage::English => "Compose command",
        };
        let bilingual_title = "Compose command / اكتب أمر";
        let title_x = icon_rect.right() + style.px(6.0);
        let avail_w = (layout.header.right() - title_x).max(0.0);

        let hint_text = match view.language {
            UiLanguage::Arabic => "Enter تشغيل · Shift+Enter سطر · Tab إكمال · Esc إغلاق",
            UiLanguage::English => "Enter Run · Shift+Enter Newline · Tab Complete · Esc Close",
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
            bilingual_title,
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
                layout.header.right() - hint_w,
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
        } else {
            let short_buf = font.fit(
                ctx.font_system,
                UiSize::Small,
                UiWeight::Semibold,
                style.scale,
                title_text,
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

    // 4. Editor lines (shaped with RowShaper), selection highlights, IME preedit underline, caret.
    let all_empty = view.lines.len() == 1 && view.lines[0].is_empty() && view.preedit.is_none();
    let input_bounds = TextBounds {
        left: layout.input.x.floor() as i32,
        top: layout.input.y.floor() as i32,
        right: layout.input.right().ceil() as i32,
        bottom: layout.input.bottom().ceil() as i32,
    };

    if all_empty {
        let placeholder = match view.language {
            UiLanguage::Arabic => "اكتب أمرك هنا…",
            UiLanguage::English => "Type a command… / اكتب أمرك هنا…",
        };
        let ph_buf = font.fit(
            ctx.font_system,
            UiSize::Body,
            UiWeight::Regular,
            style.scale,
            placeholder,
            layout.input.w,
        );
        let ph_rect = Rect::new(
            layout.input.x,
            layout.input.y,
            layout.input.w,
            layout.line_height,
        );
        scene.labels.push(UiLabel::new(
            ph_buf,
            ph_rect,
            UiAlign::Start,
            style.text_faint,
        ));
    }

    let caret_w = style.px(2.0).max(1.5).round();
    let visible_end = (layout.scroll_line + layout.visible_lines).min(view.lines.len());
    for (vis_idx, line_idx) in (layout.scroll_line..visible_end).enumerate() {
        let line_text = &view.lines[line_idx];
        let y = layout.input.y + vis_idx as f32 * layout.line_height;
        let is_caret_line = line_idx == view.caret.0;
        let line_caret = is_caret_line.then_some(view.caret.1);
        let line_preedit = is_caret_line
            .then(|| view.preedit.as_ref().map(|(t, c)| (t.as_str(), *c)))
            .flatten();
        let line_ghost = (is_caret_line && line_preedit.is_none() && view.selection.is_none())
            .then(|| view.ghost.as_deref().map(|g| (g, style.text_faint)))
            .flatten();

        let shaped = shape_composer_line(
            ctx,
            line_text,
            line_caret,
            style.text,
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
            if badge_w + style.px(40.0) < inner.w {
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

            let label_w = (inner.w - badge_w - style.px(SPACE_8)).max(0.0);
            let display = match &item.detail {
                Some(detail) if !detail.is_empty() => format!("{}  —  {detail}", item.label),
                _ => item.label.clone(),
            };
            let label_buf = font.fit(
                ctx.font_system,
                UiSize::Caption,
                if selected {
                    UiWeight::Semibold
                } else {
                    UiWeight::Regular
                },
                style.scale,
                &display,
                label_w,
            );
            let label_rect = Rect::new(inner.x, inner.y, label_w, inner.h);
            scene.labels.push(UiLabel::new(
                label_buf,
                label_rect,
                UiAlign::Start,
                if selected { style.text } else { style.text_dim },
            ));
        }
    }

    scene
}

fn rgb8(c: Rgba) -> Rgb {
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb {
        r: to8(c.r),
        g: to8(c.g),
        b: to8(c.b),
    }
}

fn clamp_char_boundary(s: &str, mut byte: usize) -> usize {
    byte = byte.min(s.len());
    while byte > 0 && !s.is_char_boundary(byte) {
        byte -= 1;
    }
    byte
}
