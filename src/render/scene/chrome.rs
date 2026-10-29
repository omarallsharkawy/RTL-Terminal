//! Window chrome around the panes: the tab bar, search status lines and pane dividers.

use cosmic_text::Buffer;

use super::context::DrawContext;
use crate::layout::{truncate_title, Rect};
use crate::render::style::Quad;
use crate::render::TabLabel;

/// Where each tab goes in a window `width` wide; empty with fewer than two tabs (no tab bar).
pub(crate) fn tab_geometry(ctx: &DrawContext, tab_count: usize, width: f32) -> Vec<Rect> {
    ctx.metrics.tab_rects(tab_count, width)
}

/// The tab bar background and the active tab's highlight and accent line.
pub(crate) fn tab_bar_quads(
    ctx: &DrawContext,
    tabs: &[TabLabel],
    geometry: &[Rect],
    width: f32,
    out: &mut Vec<Quad>,
) {
    if geometry.is_empty() {
        return;
    }
    let palette = ctx.palette;
    let bar = palette.background.mix(palette.foreground, 0.08);
    out.push((0.0, 0.0, width, ctx.metrics.line_height, ctx.color(bar)));
    let accent = (ctx.metrics.scale * 2.0).round().max(1.0);
    for (_, rect) in tabs.iter().zip(geometry).filter(|(t, _)| t.active) {
        out.push((
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            ctx.color(palette.background),
        ));
        out.push((rect.x, rect.y, rect.w, accent, ctx.color(palette.cursor)));
    }
}

/// Tab titles, shaped with full shaping so Arabic titles render, and their colors.
pub(crate) fn tab_titles(
    ctx: &mut DrawContext,
    tabs: &[TabLabel],
    geometry: &[Rect],
) -> Vec<(Buffer, Rect, glyphon::Color)> {
    let mut titles = Vec::new();
    for (rect, tab) in geometry.iter().zip(tabs) {
        let cols = ((rect.w / ctx.metrics.char_width).floor() as usize).saturating_sub(2);
        let title = truncate_title(&tab.title, cols);
        let buffer = ctx.label(&title);
        let color = if tab.active {
            ctx.palette.foreground
        } else {
            ctx.palette.foreground.mix(ctx.palette.background, 0.2)
        };
        titles.push((buffer, *rect, color.to_glyphon()));
    }
    titles
}

/// Background quad, shaped text and top y of a pane's status line, over the last of its
/// `rows` rows.
pub(crate) fn status_line(
    ctx: &mut DrawContext,
    rect: Rect,
    rows: usize,
    text: &str,
) -> Option<(Quad, Buffer, f32)> {
    let m = ctx.metrics;
    let y = rect.y + m.padding_top + rows.checked_sub(1)? as f32 * m.line_height;
    let bar = ctx.palette.background.mix(ctx.palette.foreground, 0.14);
    let quad = (rect.x, y, rect.w, m.line_height, ctx.color(bar));
    Some((quad, ctx.label(text), y))
}

/// The dividers between panes.
pub(crate) fn divider_quads<'a>(
    ctx: &DrawContext,
    dividers: &'a [Rect],
) -> impl Iterator<Item = Quad> + 'a {
    let color = ctx.color(ctx.palette.background.mix(ctx.palette.foreground, 0.28));
    dividers.iter().map(move |d| (d.x, d.y, d.w, d.h, color))
}
