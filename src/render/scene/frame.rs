//! One frame on the CPU: every quad back to front and every text run, built from the panes
//! without touching the GPU, so the whole layout can be tested headless.

use alacritty_terminal::vte::ansi::CursorShape;
use cosmic_text::Buffer;
use glyphon::TextBounds;

use super::blocks::{self, Badge};
use super::chrome;
use super::context::DrawContext;
use super::cursor::{
    cursor_cell_width, cursor_left, cursor_quads, effective_cursor_shape,
    geometry_under_block_cursor,
};
use super::palette::{palette_overlay, OverlayText, PaletteView};
use super::preedit::{bounds_outside, ImeArea, PreeditState};
use crate::layout::Rect;
use crate::render::metrics::CellMetrics;
use crate::render::row::cache::RowCache;
use crate::render::row::{hash_row, CachedRow, RowCursor};
use crate::render::style::Quad;
use crate::render::{PaneFrame, TabLabel};

/// Renderer state a frame depends on besides what it is drawn with.
pub struct FrameOptions<'a> {
    /// Whether the window has keyboard focus (unfocused cursors are hollow).
    pub window_focused: bool,
    /// Configured cursor style ("block", "beam"/"bar", "underline").
    pub cursor_style: &'a str,
    /// Blink phase: while false the focused cursor is not drawn.
    pub cursor_blink_on: bool,
    /// Surface size in physical pixels.
    pub surface: (f32, f32),
}

/// A shaped buffer and where to draw it.
pub struct TextRun<'a> {
    pub buffer: &'a Buffer,
    pub left: f32,
    pub top: f32,
    pub bounds: TextBounds,
    pub color: glyphon::Color,
}

/// Everything a frame draws. Row text stays in the `RowCache` and the preedit text in its
/// `PreeditState`; `text_runs` collects them with the labels shaped here.
pub struct Frame {
    /// Back to front: tab bar, per pane the selected block's tint, its rows (backgrounds,
    /// geometry, decorations) and block marks and badges, then the preedit box,
    /// unfocused panes' cursors, the focused cursor (or preedit caret), status bars, dividers.
    pub quads: Vec<Quad>,
    /// Window clear color (the theme background) in surface space.
    pub background: [f32; 4],
    /// The candidate popup area, when it moved since it was last reported.
    pub ime_area: Option<ImeArea>,
    tab_titles: Vec<(Buffer, Rect, glyphon::Color)>,
    sidebar_labels: Vec<super::sidebar::Label>,
    /// Labels of the code review panel.
    review_texts: Vec<super::review::ReviewText>,
    /// Per pane: the status line text and its top y.
    statuses: Vec<Option<(Buffer, f32)>>,
    /// Per pane: the badges of its command blocks.
    badges: Vec<Vec<Badge>>,
    /// The preedit box (left, y, width), when one is visible.
    preedit_box: Option<(f32, f32, f32)>,
    /// Clip of the pane the preedit is drawn in.
    ime_bounds: Option<TextBounds>,
    metrics: CellMetrics,
    foreground: glyphon::Color,
    surface: (f32, f32),
    /// Overlays drawn above everything (the command palette).
    overlay: Vec<OverlayText>,
    /// Area the overlay covers: other text is clipped out of it, since all text is drawn
    /// after all quads and would otherwise show through the overlay's background.
    overlay_rect: Option<Rect>,
}

/// The parts of `bounds` outside `hole`: up to four strips (above, below, left, right).
pub fn bounds_around(bounds: TextBounds, hole: Rect) -> Vec<TextBounds> {
    let (hl, ht) = (hole.x.floor() as i32, hole.y.floor() as i32);
    let (hr, hb) = (hole.right().ceil() as i32, hole.bottom().ceil() as i32);
    if hr <= bounds.left || hl >= bounds.right || hb <= bounds.top || ht >= bounds.bottom {
        return vec![bounds];
    }
    let mid_top = bounds.top.max(ht);
    let mid_bottom = bounds.bottom.min(hb);
    [
        TextBounds {
            bottom: ht,
            ..bounds
        },
        TextBounds { top: hb, ..bounds },
        TextBounds {
            top: mid_top,
            bottom: mid_bottom,
            right: hl,
            ..bounds
        },
        TextBounds {
            top: mid_top,
            bottom: mid_bottom,
            left: hr,
            ..bounds
        },
    ]
    .into_iter()
    .filter(|b| {
        b.left.max(bounds.left) < b.right.min(bounds.right)
            && b.top.max(bounds.top) < b.bottom.min(bounds.bottom)
    })
    .map(|b| TextBounds {
        left: b.left.max(bounds.left),
        top: b.top.max(bounds.top),
        right: b.right.min(bounds.right),
        bottom: b.bottom.min(bounds.bottom),
    })
    .collect()
}

fn rect_bounds(r: Rect) -> TextBounds {
    TextBounds {
        left: r.x as i32,
        top: r.y as i32,
        right: r.right() as i32,
        bottom: r.bottom() as i32,
    }
}

/// Lay out the tab bar (with two or more tabs), every pane and the dividers between them.
/// Rows that changed are reshaped into `cache`; the preedit is shaped into `ime`.
pub fn build_frame(
    ctx: &mut DrawContext,
    opts: &FrameOptions,
    cache: &mut RowCache,
    ime: &mut PreeditState,
    tabs: &[TabLabel],
    panes: &[PaneFrame],
    dividers: &[Rect],
) -> Frame {
    let m = ctx.metrics;
    let (width, _) = opts.surface;
    let background = ctx.color(ctx.palette.background);
    let cursor_color = ctx.color(ctx.palette.cursor);
    let mut quads: Vec<Quad> = Vec::new();
    // The focused pane's cursor is separate: a preedit replaces it.
    let mut focused_cursor: Vec<Quad> = Vec::new();
    let mut other_cursors: Vec<Quad> = Vec::new();
    let mut ime_caret = None;
    let mut ime_bounds: Option<TextBounds> = None;
    let mut badges = Vec::with_capacity(panes.len());

    let tab_geometry = chrome::tab_geometry(ctx, tabs.len(), width);
    chrome::tab_bar_quads(ctx, tabs, &tab_geometry, width, &mut quads);

    for pane in panes {
        let rect = pane.rect;
        let (origin_x, origin_y) = (rect.x + m.padding_left, rect.y + m.padding_top);
        let mut previous = cache.take(pane.id);
        let mut rows = Vec::with_capacity(pane.lines.len());
        let shape = effective_cursor_shape(opts.cursor_style, pane.cursor.shape);
        let cursor_focused = opts.window_focused && pane.focused;
        blocks::tint_quads(ctx, pane, &mut quads);

        for (r, line) in pane.lines.iter().enumerate() {
            let y = origin_y + r as f32 * m.line_height;
            // The cursor row stays marked during the blink's off phase: shaping depends on it
            // (an Arabic line being typed keeps its LTR base), only the block recolor turns off.
            let row_cursor =
                (pane.cursor.is_visible && pane.cursor.row == r).then_some(RowCursor {
                    col: pane.cursor.col,
                    block: cursor_focused && opts.cursor_blink_on && shape == CursorShape::Block,
                });

            let hash = hash_row(line, row_cursor);
            let row = previous.take(r, hash).unwrap_or_else(|| {
                let segments = ctx.shaper().build_row(line, row_cursor);
                let row_quads = ctx.row_quads(line, &segments);
                CachedRow::new(hash, line, segments, row_quads, m.char_width)
            });
            rows.push(row);

            let cached = &rows[r];
            quads.extend(
                cached
                    .quads
                    .iter()
                    .map(|&(x, qy, w, h, c)| (origin_x + x, y + qy, w, h, c)),
            );

            if pane.focused && pane.cursor.row == r {
                let (x, ext) = cached.cursor_anchor(pane.cursor.col, m.char_width);
                ime_caret = Some((origin_x + x, y, ext, rect));
                ime_bounds = Some(rect_bounds(rect));
            }

            // Only the focused pane's cursor blinks.
            if let Some(rc) = row_cursor.filter(|_| !pane.focused || opts.cursor_blink_on) {
                let (x, extends_left) = cached.cursor_anchor(rc.col, m.char_width);
                let x = cursor_left(origin_x + x, extends_left, shape, &m);
                let out = if pane.focused {
                    &mut focused_cursor
                } else {
                    &mut other_cursors
                };
                let w = cursor_cell_width(line, rc.col, m.char_width);
                cursor_quads((x, y, w), shape, cursor_focused, &m, cursor_color, out);
                if let Some(cell) = line.cells.get(rc.col).filter(|_| rc.block) {
                    let rect = (x, y, w, m.line_height);
                    out.extend(geometry_under_block_cursor(
                        ctx.palette,
                        ctx.is_srgb,
                        cell,
                        rect,
                    ));
                }
            }
        }
        cache.put(pane.id, rows);
        blocks::mark_quads(ctx, pane, &mut quads);
        let status_row = pane.status.as_ref().and(pane.lines.len().checked_sub(1));
        badges.push(blocks::badges(ctx, pane, status_row, &mut quads));
    }

    let (preedit_box, ime_area) = ime.layout(ctx, ime_caret, &mut quads, &mut focused_cursor);
    // The cursor sits above cell backgrounds but below text (its glyph was recolored).
    quads.extend(other_cursors);
    quads.extend(focused_cursor);

    let mut statuses = Vec::with_capacity(panes.len());
    for pane in panes {
        let status = match &pane.status {
            Some(text) => chrome::status_line(ctx, pane.rect, pane.lines.len(), text),
            None => None,
        };
        statuses.push(status.map(|(quad, buffer, y)| {
            quads.push(quad);
            (buffer, y)
        }));
    }

    quads.extend(chrome::divider_quads(ctx, dividers));

    let tab_titles = chrome::tab_titles(ctx, tabs, &tab_geometry);

    Frame {
        quads,
        background,
        ime_area,
        tab_titles,
        sidebar_labels: Vec::new(),
        review_texts: Vec::new(),
        statuses,
        badges,
        preedit_box,
        ime_bounds,
        metrics: m,
        foreground: ctx.palette.foreground.to_glyphon(),
        surface: opts.surface,
        overlay: Vec::new(),
        overlay_rect: None,
    }
}

impl Frame {
    /// Draw the command palette above everything else.
    pub fn add_palette(&mut self, ctx: &mut DrawContext, view: &PaletteView) {
        let (outer, texts) = palette_overlay(ctx, view, self.surface, &mut self.quads);
        self.overlay.extend(texts);
        self.overlay_rect = outer;
    }

    /// Optional chrome is attached explicitly so offscreen/golden callers keep their layout.
    pub fn add_sidebar(
        &mut self,
        ctx: &mut DrawContext,
        view: &crate::sidebar::View,
        cache: &mut super::sidebar::LabelCache,
    ) {
        let scene = super::sidebar::build(ctx, view, cache);
        self.quads.extend(scene.quads);
        self.sidebar_labels = scene.labels;
    }

    /// The code review panel, beside the panes (they are laid out to leave it room).
    pub fn add_review(
        &mut self,
        ctx: &mut DrawContext,
        view: &crate::review::panel::View,
        cache: &mut super::review::ReviewCache,
    ) {
        let scene = super::review::build(ctx, view, cache);
        self.quads.extend(scene.quads);
        self.review_texts = scene.texts;
    }

    pub fn add_attention(&mut self, ctx: &DrawContext, rects: &[Rect]) {
        self.quads
            .extend(super::sidebar::attention_quads(ctx, rects));
    }

    /// Every text run, in drawing order: tab titles, per-pane rows (the last one hidden under
    /// a status line) and status line, preedit, sidebar labels, the review panel, then the
    /// command palette.
    pub fn text_runs<'a>(
        &'a self,
        panes: &[PaneFrame],
        cache: &'a RowCache,
        ime: &'a PreeditState,
    ) -> Vec<TextRun<'a>> {
        let m = &self.metrics;
        let fg = self.foreground;
        let mut runs = Vec::new();
        for (buffer, rect, color) in &self.tab_titles {
            runs.push(TextRun {
                buffer,
                left: rect.x + m.char_width,
                top: rect.y,
                bounds: rect_bounds(*rect),
                color: *color,
            });
        }
        for ((pane, status), badges) in panes.iter().zip(&self.statuses).zip(&self.badges) {
            let bounds = rect_bounds(pane.rect);
            let rows = pane.lines.len();
            for r in 0..rows {
                if status.is_some() && r + 1 == rows {
                    break;
                }
                let Some(cached_row) = cache.row(pane.id, r) else {
                    continue;
                };
                let y = pane.rect.y + m.padding_top + r as f32 * m.line_height;
                // The row under the preedit is clipped around it, so its text does not show
                // through the preedit's background.
                let badge = badges.iter().find(|b| b.row == r);
                let (parts, n) = match (self.preedit_box, badge) {
                    (Some((left, _, width)), _) if pane.focused && pane.cursor.row == r => {
                        (bounds_outside(bounds, left, left + width), 2)
                    }
                    // A block badge covers the end of its row.
                    (_, Some(b)) => (bounds_outside(bounds, b.left, b.right), 1),
                    _ => ([bounds, bounds], 1),
                };
                for seg in &cached_row.segments {
                    for &bounds in &parts[..n] {
                        runs.push(TextRun {
                            buffer: &seg.buffer,
                            left: pane.rect.x + m.padding_left + seg.seg_x,
                            top: y,
                            bounds,
                            color: fg,
                        });
                    }
                }
            }
            for badge in badges {
                runs.push(TextRun {
                    buffer: &badge.buffer,
                    left: badge.text_left,
                    top: badge.top,
                    bounds,
                    color: badge.color,
                });
            }
            if let Some((buffer, y)) = status {
                runs.push(TextRun {
                    buffer,
                    left: pane.rect.x + m.padding_left,
                    top: *y,
                    bounds,
                    color: fg,
                });
            }
        }
        if let Some((left, top, _)) = self.preedit_box {
            let (width, height) = self.surface;
            let bounds = self
                .ime_bounds
                .unwrap_or_else(|| rect_bounds(Rect::new(0.0, 0.0, width, height)));
            if let Some(buffer) = ime.buffer() {
                runs.push(TextRun {
                    buffer,
                    left,
                    top,
                    bounds,
                    color: fg,
                });
            }
        }
        // Sidebar labels after the panes, so they draw above anything a pane draws there.
        for label in &self.sidebar_labels {
            runs.push(TextRun {
                buffer: &label.buffer,
                left: label.left,
                top: label.top,
                bounds: label.bounds,
                color: label.color,
            });
        }
        for text in &self.review_texts {
            for seg in &text.row.segments {
                runs.push(TextRun {
                    buffer: &seg.buffer,
                    left: text.left + seg.seg_x,
                    top: text.top,
                    bounds: text.bounds,
                    color: fg,
                });
            }
        }
        if let Some(hole) = self.overlay_rect {
            runs = runs
                .into_iter()
                .flat_map(|run| {
                    bounds_around(run.bounds, hole)
                        .into_iter()
                        .map(move |bounds| TextRun { bounds, ..run })
                })
                .collect();
        }
        for text in &self.overlay {
            runs.push(TextRun {
                buffer: &text.buffer,
                left: text.left,
                top: text.top,
                bounds: text.bounds,
                color: text.color,
            });
        }
        runs
    }
}
