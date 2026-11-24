//! Command blocks drawn over a pane (`blocks::BlockDecor`): a thin separator above each prompt,
//! a gutter strip at the pane's leading edge colored by the command's status, a tint over the
//! selected block, and a badge with the exit code and duration at the end of its first line.
//! Only quads and one shaped label per badge; the grid is never touched.

use cosmic_text::Buffer;

use super::context::DrawContext;
use crate::blocks::{BlockDecor, BlockStatus};
use crate::color::Rgba;
use crate::render::style::Quad;
use crate::render::PaneFrame;

/// A badge's shaped text and where it goes.
pub struct Badge {
    pub buffer: Buffer,
    /// Viewport row it is drawn on.
    pub row: usize,
    /// Pixel span of its background (row text under it is clipped).
    pub left: f32,
    pub right: f32,
    /// Where the text starts.
    pub text_left: f32,
    pub top: f32,
    pub color: glyphon::Color,
}

/// Rows of the pane a block covers, clipped to the viewport.
fn visible_rows(decor: &BlockDecor, rows: usize) -> Option<(usize, usize)> {
    let first = decor.first_row.max(0);
    let last = decor.last_row.min(rows as i64 - 1);
    (first <= last && last >= 0).then_some((first as usize, last as usize))
}

fn status_color(ctx: &DrawContext, status: BlockStatus) -> Option<Rgba> {
    let p = ctx.palette;
    match status {
        BlockStatus::Prompt => None,
        BlockStatus::Running => Some(p.cursor),
        BlockStatus::Succeeded => Some(p.ansi[2]),
        BlockStatus::Failed(_) => Some(p.ansi[1]),
        BlockStatus::Finished => Some(p.foreground.mix(p.background, 0.5)),
    }
}

/// The selected block's background tint, drawn under the rows (cells with a background of
/// their own cover it).
pub(crate) fn tint_quads(ctx: &DrawContext, pane: &PaneFrame, out: &mut Vec<Quad>) {
    let m = ctx.metrics;
    let rows = pane.lines.len();
    let top = pane.rect.y + m.padding_top;
    let color = ctx.color(ctx.palette.background.mix(ctx.palette.cursor, 0.08));
    for decor in pane.blocks.iter().filter(|d| d.selected) {
        if let Some((first, last)) = visible_rows(decor, rows) {
            let y = top + first as f32 * m.line_height;
            let h = (last - first + 1) as f32 * m.line_height;
            out.push((pane.rect.x, y, pane.rect.w, h, color));
        }
    }
}

/// Separators and gutter strips, drawn over the rows.
pub(crate) fn mark_quads(ctx: &DrawContext, pane: &PaneFrame, out: &mut Vec<Quad>) {
    let m = ctx.metrics;
    let rows = pane.lines.len();
    let top = pane.rect.y + m.padding_top;
    let thin = m.scale.round().max(1.0);
    let separator = ctx.color(ctx.palette.background.mix(ctx.palette.foreground, 0.18));
    // The strip sits in the pane's left padding, clear of the first column.
    let gutter_w = (m.scale * 2.0)
        .round()
        .max(2.0)
        .min(m.padding_left.max(2.0));
    let gutter_x = pane.rect.x + ((m.padding_left - gutter_w) * 0.5).max(0.0).floor();
    for decor in &pane.blocks {
        if (0..rows as i64).contains(&decor.first_row) && decor.first_row > 0 {
            let y = top + decor.first_row as f32 * m.line_height;
            out.push((pane.rect.x, y, pane.rect.w, thin, separator));
        }
        let (Some(color), Some((first, last))) =
            (status_color(ctx, decor.status), visible_rows(decor, rows))
        else {
            continue;
        };
        let y = top + first as f32 * m.line_height;
        let h = (last - first + 1) as f32 * m.line_height;
        // A small gap under the separator keeps blocks apart.
        let gap = if first as i64 == decor.first_row {
            thin
        } else {
            0.0
        };
        out.push((gutter_x, y + gap, gutter_w, h - gap, ctx.color(color)));
    }
}

/// Badges at the end of each visible block's first line, with their backgrounds in `out`.
/// `hidden_row` is a row covered by the pane's status line.
pub(crate) fn badges(
    ctx: &mut DrawContext,
    pane: &PaneFrame,
    hidden_row: Option<usize>,
    out: &mut Vec<Quad>,
) -> Vec<Badge> {
    let m = ctx.metrics;
    let rows = pane.lines.len();
    let cols = pane.lines.first().map_or(0, |l| l.cells.len());
    let mut badges = Vec::new();
    for decor in &pane.blocks {
        let Some(text) = decor.badge.as_deref() else {
            continue;
        };
        let Ok(row) = usize::try_from(decor.first_row) else {
            continue;
        };
        if row >= rows || Some(row) == hidden_row {
            continue;
        }
        let buffer = ctx.label(text);
        let width = buffer
            .layout_runs()
            .map(|run| run.line_w)
            .fold(0.0_f32, f32::max);
        let pad = m.char_width * 0.5;
        let grid_left = pane.rect.x + m.padding_left;
        let right = (grid_left + cols as f32 * m.char_width).min(pane.rect.right());
        let left = (right - width - 2.0 * pad).max(grid_left);
        let top = pane.rect.y + m.padding_top + row as f32 * m.line_height;
        let p = ctx.palette;
        out.push((
            left,
            top,
            right - left,
            m.line_height,
            ctx.color(p.background.mix(p.foreground, 0.1)),
        ));
        let color = match decor.status {
            BlockStatus::Failed(_) => p.ansi[1],
            _ => p.foreground.mix(p.background, 0.3),
        };
        badges.push(Badge {
            buffer,
            row,
            left,
            right,
            text_left: left + pad,
            top,
            color: color.to_glyphon(),
        });
    }
    badges
}
