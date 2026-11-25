//! The code review panel on the CPU: quads and text runs from a `review::panel::View`.
//!
//! Every label, diff line included, is shaped by the terminal's own row shaper from cells
//! built here, so Arabic and other right-to-left text in a diff line is laid out exactly as the
//! terminal would show that line (per-line direction marks, isolated Latin runs, wide
//! characters on two cells, box drawing as geometry). Shaped rows are cached by text and color
//! across frames, so scrolling and blinking reshape only what changed.
//!
//! The drawing surface is small on purpose (`build`, `ReviewCache`, the colors in `Colors`):
//! restyling (rounded shapes, UI fonts) only has to touch this file.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, Rgb};
use glyphon::TextBounds;
use unicode_bidi::{bidi_class, BidiClass};
use unicode_width::UnicodeWidthChar;

use super::context::DrawContext;
use crate::color::Rgba;
use crate::layout::Rect;
use crate::render::metrics::CellMetrics;
use crate::render::row::CachedRow;
use crate::render::style::Quad;
use crate::review::layout::PanelLayout;
use crate::review::panel::{RowStyle, View};
use crate::terminal::{CellData, LineData, MAX_DRAWN_MARKS};

/// A shaped label of the panel and where it goes.
pub struct ReviewText {
    pub row: Rc<CachedRow>,
    pub left: f32,
    pub top: f32,
    pub bounds: TextBounds,
}

/// The panel's quads (back to front) and labels.
pub struct ReviewScene {
    pub quads: Vec<Quad>,
    pub texts: Vec<ReviewText>,
}

type Key = (String, [u8; 3]);

/// Shaped labels of the previous frame, by text and color.
#[derive(Default)]
pub struct ReviewCache {
    font: Option<(CellMetrics, Option<&'static str>)>,
    rows: HashMap<Key, (Rc<CachedRow>, bool)>,
}

impl ReviewCache {
    fn begin_frame(&mut self, ctx: &DrawContext) {
        let font = Some((ctx.metrics, ctx.family));
        if self.font != font {
            self.rows.clear();
            self.font = font;
        }
        for (_, used) in self.rows.values_mut() {
            *used = false;
        }
    }

    fn get(&mut self, ctx: &mut DrawContext, text: &str, color: Rgba) -> Rc<CachedRow> {
        let rgb = rgb8(color);
        let key = (text.to_owned(), [rgb.r, rgb.g, rgb.b]);
        let (row, used) = self.rows.entry(key).or_insert_with(|| {
            let line = text_line(text, rgb);
            let segments = ctx.shaper().build_row(&line, None);
            let quads = ctx.row_quads(&line, &segments);
            let row = CachedRow::new(line.hash, &line, segments, quads, ctx.metrics.char_width);
            (Rc::new(row), true)
        });
        *used = true;
        row.clone()
    }

    fn end_frame(&mut self) {
        self.rows.retain(|_, (_, used)| *used);
    }

    pub fn clear(&mut self) {
        self.rows.clear();
    }

    /// Labels kept for the next frame.
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

fn rgb8(c: Rgba) -> Rgb {
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgb {
        r: to8(c.r),
        g: to8(c.g),
        b: to8(c.b),
    }
}

/// `text` as one row of cells in color `fg`: combining marks on their base cell, wide
/// characters on two cells. Control characters must already be replaced (`display_text`).
pub fn text_line(text: &str, fg: Rgb) -> LineData {
    let fg = AnsiColor::Spec(fg);
    let mut cells: Vec<CellData> = Vec::with_capacity(text.len());
    let mut has_rtl = false;
    for c in text.chars() {
        if matches!(bidi_class(c), BidiClass::R | BidiClass::AL) {
            has_rtl = true;
        }
        match c.width() {
            Some(0) | None => {
                if let Some(base) = cells.iter_mut().rev().find(|c| !c.is_spacer()) {
                    if base.zerowidth.len() < MAX_DRAWN_MARKS {
                        base.zerowidth.push(c);
                    }
                }
            }
            Some(2) => {
                cells.push(CellData {
                    c,
                    fg,
                    flags: CellFlags::WIDE_CHAR,
                    ..CellData::blank()
                });
                cells.push(CellData {
                    flags: CellFlags::WIDE_CHAR_SPACER,
                    ..CellData::blank()
                });
            }
            Some(_) => cells.push(CellData {
                c,
                fg,
                ..CellData::blank()
            }),
        }
    }
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    (fg_bytes(&fg)).hash(&mut hasher);
    LineData {
        cells: cells.into(),
        has_rtl,
        hash: hasher.finish(),
    }
}

fn fg_bytes(fg: &AnsiColor) -> [u8; 3] {
    match fg {
        AnsiColor::Spec(rgb) => [rgb.r, rgb.g, rgb.b],
        _ => [0; 3],
    }
}

/// The panel's colors, all derived from the theme so light themes work.
struct Colors {
    panel: Rgba,
    header: Rgba,
    border: Rgba,
    accent: Rgba,
    text: Rgba,
    dim: Rgba,
    faint: Rgba,
    file: Rgba,
    hunk: Rgba,
    add: Rgba,
    add_gutter: Rgba,
    del: Rgba,
    del_gutter: Rgba,
    green: Rgba,
    red: Rgba,
    warn: Rgba,
    selected: Rgba,
    button: Rgba,
    commit: Rgba,
    input: Rgba,
}

impl Colors {
    fn new(ctx: &DrawContext) -> Self {
        let p = ctx.palette;
        let (fg, bg) = (p.foreground, p.background);
        let (accent, green, red) = (p.ansi[4], p.ansi[2], p.ansi[1]);
        Self {
            panel: bg.mix(fg, 0.03),
            header: bg.mix(fg, 0.07),
            border: bg.mix(fg, 0.18),
            accent,
            text: fg,
            dim: fg.mix(bg, 0.42),
            faint: fg.mix(bg, 0.6),
            file: bg.mix(fg, 0.10),
            hunk: bg.mix(accent, 0.10),
            add: bg.mix(green, 0.15),
            add_gutter: bg.mix(green, 0.27),
            del: bg.mix(red, 0.15),
            del_gutter: bg.mix(red, 0.27),
            green: green.mix(fg, 0.15),
            red: red.mix(fg, 0.15),
            warn: p.ansi[3],
            selected: bg.mix(p.selection, 0.55),
            button: bg.mix(fg, 0.13),
            commit: bg.mix(accent, 0.32),
            input: bg.mix(fg, 0.11),
        }
    }
}

fn clip(r: Rect, to: Rect) -> TextBounds {
    TextBounds {
        left: r.x.max(to.x).ceil() as i32,
        top: r.y.max(to.y).ceil() as i32,
        right: r.right().min(to.right()).floor() as i32,
        bottom: r.bottom().min(to.bottom()).floor() as i32,
    }
}

/// Lay out the panel described by `view`.
pub fn build(ctx: &mut DrawContext, view: &View, cache: &mut ReviewCache) -> ReviewScene {
    cache.begin_frame(ctx);
    let m = ctx.metrics;
    let colors = Colors::new(ctx);
    let commit_cols = view.commit_label.chars().count();
    let layout = PanelLayout::new(
        view.rect,
        m.line_height,
        m.char_width,
        m.scale,
        view.notice.is_some(),
        commit_cols,
    );
    let (lh, cw) = (layout.line_height, layout.char_width);
    let panel = view.rect;
    let mut quads: Vec<Quad> = Vec::new();
    let mut texts: Vec<ReviewText> = Vec::new();
    let quad = |r: Rect, c: Rgba, quads: &mut Vec<Quad>| {
        if r.w > 0.0 && r.h > 0.0 {
            quads.push((r.x, r.y, r.w, r.h, ctx.color(c)));
        }
    };

    // Background, the line between panes and panel (accent while it has the keyboard), header.
    quad(panel, colors.panel, &mut quads);
    quad(layout.header, colors.header, &mut quads);
    let edge = if view.focused {
        colors.accent
    } else {
        colors.border
    };
    quad(
        Rect::new(panel.x, panel.y, layout.border, panel.h),
        edge,
        &mut quads,
    );
    quad(
        Rect::new(
            layout.header.x,
            layout.header.bottom(),
            layout.header.w,
            layout.border,
        ),
        colors.border,
        &mut quads,
    );
    for button in [layout.refresh, layout.close] {
        quad(button, colors.button, &mut quads);
    }
    quad(layout.commit, colors.commit, &mut quads);

    // Body rows: backgrounds first.
    let gutter_end = layout.gutter_left() + layout.gutter_width(view.gutter_cols);
    let body = layout.body;
    for (i, row) in view.rows.iter().enumerate() {
        let top = layout.row_top(i);
        if top >= body.bottom() {
            break;
        }
        let h = lh.min(body.bottom() - top);
        let full = Rect::new(body.x, top, body.w, h);
        let gutter = Rect::new(body.x, top, (gutter_end + cw / 2.0 - body.x).max(0.0), h);
        match row.style {
            RowStyle::File { .. } => quad(full, colors.file, &mut quads),
            RowStyle::Hunk => quad(full, colors.hunk, &mut quads),
            RowStyle::Add => {
                quad(full, colors.add, &mut quads);
                quad(gutter, colors.add_gutter, &mut quads);
            }
            RowStyle::Del => {
                quad(full, colors.del, &mut quads);
                quad(gutter, colors.del_gutter, &mut quads);
            }
            RowStyle::CommentInput => {
                let left = layout.text_left(view.gutter_cols) - cw / 2.0;
                let bx = Rect::new(left, top, (body.right() - layout.pad - left).max(0.0), h);
                quad(bx, colors.accent, &mut quads);
                let t = layout.border;
                quad(
                    Rect::new(bx.x + t, bx.y + t, bx.w - 2.0 * t, bx.h - 2.0 * t),
                    colors.input,
                    &mut quads,
                );
            }
            _ => {}
        }
        if row.selected {
            quad(full, colors.selected, &mut quads);
            let bar = (2.0 * layout.border).min(full.w);
            quad(
                Rect::new(body.x, top, bar, h),
                if view.focused {
                    colors.accent
                } else {
                    colors.border
                },
                &mut quads,
            );
        }
    }
    if let Some(footer) = layout.footer {
        quad(footer, colors.header, &mut quads);
        quad(
            Rect::new(footer.x, footer.y, footer.w, layout.border),
            colors.border,
            &mut quads,
        );
    }

    let mut label = |ctx: &mut DrawContext, text: &str, color: Rgba, x: f32, y: f32, area: Rect| {
        if text.is_empty() {
            return;
        }
        let row = cache.get(ctx, text, color);
        let bounds = clip(area, panel);
        if bounds.left >= bounds.right || bounds.top >= bounds.bottom {
            return;
        }
        quads.extend(
            row.quads
                .iter()
                .filter(|&&(qx, ..)| x + qx < area.right())
                .map(|&(qx, qy, w, h, c)| (x + qx, y + qy, w.min(area.right() - x - qx), h, c)),
        );
        texts.push(ReviewText {
            row,
            left: x,
            top: y,
            bounds,
        });
    };

    // Header: title, totals and file count, buttons.
    label(
        ctx,
        &view.title,
        colors.text,
        layout.title.x,
        layout.title.y,
        layout.title,
    );
    let mut x = layout.summary.x;
    if let Some((added, removed)) = view.totals {
        let plus = format!("+{added}");
        let minus = format!("−{removed}");
        label(
            ctx,
            &plus,
            colors.green,
            x,
            layout.summary.y,
            layout.summary,
        );
        x += (plus.chars().count() + 1) as f32 * cw;
        label(ctx, &minus, colors.red, x, layout.summary.y, layout.summary);
        x += (minus.chars().count() + 2) as f32 * cw;
    }
    let summary_area = Rect::new(
        x,
        layout.summary.y,
        (layout.summary.right() - x).max(0.0),
        lh,
    );
    label(
        ctx,
        &view.summary,
        colors.dim,
        x,
        layout.summary.y,
        summary_area,
    );
    for (button, glyph) in [(layout.refresh, "↻"), (layout.close, "✕")] {
        let gx = button.x + ((button.w - cw) / 2.0).round();
        label(ctx, glyph, colors.text, gx, button.y, button);
    }
    label(
        ctx,
        view.commit_label,
        colors.text,
        layout.commit.x + cw,
        layout.commit.y,
        layout.commit,
    );

    // Rows.
    let text_left = layout.text_left(view.gutter_cols);
    let inner_right = body.right() - layout.pad;
    for (i, row) in view.rows.iter().enumerate() {
        let top = layout.row_top(i);
        if top >= body.bottom() {
            break;
        }
        let area = Rect::new(body.x, top, body.w, lh.min(body.bottom() - top));
        let mut right = inner_right;
        if let Some((added, removed)) = row.counts {
            let plus = format!("+{added}");
            let minus = format!("−{removed}");
            let cols = plus.chars().count() + 1 + minus.chars().count();
            let cx = inner_right - cols as f32 * cw;
            label(ctx, &plus, colors.green, cx, top, area);
            let mx = cx + (plus.chars().count() + 1) as f32 * cw;
            label(ctx, &minus, colors.red, mx, top, area);
            right = cx - cw;
        }
        let (left, color) = match row.style {
            RowStyle::File { .. } => (layout.gutter_left(), colors.text),
            RowStyle::Hunk => (layout.gutter_left(), colors.dim),
            RowStyle::Note | RowStyle::Message => (layout.gutter_left(), colors.dim),
            RowStyle::Context => (text_left, colors.text),
            RowStyle::Add => (text_left, colors.text),
            RowStyle::Del => (text_left, colors.text),
            RowStyle::CommentInput => (text_left, colors.text),
            RowStyle::CommentHint => (text_left, colors.faint),
        };
        if !row.gutter.is_empty() {
            label(
                ctx,
                &row.gutter,
                colors.faint,
                layout.gutter_left(),
                top,
                area,
            );
        }
        let text_area = Rect::new(left, area.y, (right - left).max(0.0), area.h);
        label(ctx, &row.text, color, left, top, text_area);
    }
    if let (Some(footer), Some(notice)) = (layout.footer, &view.notice) {
        let color = if notice.warn { colors.warn } else { colors.dim };
        let y = footer.y + ((footer.h - lh) / 2.0).round();
        let area = Rect::new(
            footer.x + layout.pad,
            footer.y,
            (footer.w - 2.0 * layout.pad).max(0.0),
            footer.h,
        );
        label(ctx, &notice.text, color, area.x, y, area);
    }
    cache.end_frame();
    ReviewScene { quads, texts }
}
