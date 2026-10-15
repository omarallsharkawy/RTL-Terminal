//! The quads of one terminal row: backgrounds, box geometry and text decorations.

use alacritty_terminal::term::cell::Flags as CellFlags;

use super::boxdraw::try_render_box_or_block;
use crate::color::{Palette, Rgba};
use crate::render::row::visual_cell_extents;
use crate::render::style::{cell_colors, target_color, Quad};
use crate::render::text::segment::CachedSegment;
use crate::terminal::LineData;

/// Backgrounds, box geometry and decorations of a row. Cells inside RTL segments follow the
/// glyphs that hold them; everything else sits on the grid. `metrics` is (cell width, line
/// height, font px).
pub fn build_row_quads(
    palette: &Palette,
    is_srgb: bool,
    metrics: (f32, f32, f32),
    line: &LineData,
    segments: &[CachedSegment],
) -> Vec<Quad> {
    let (cw, lh, font_px) = metrics;
    let target = |rgba: Rgba| target_color(rgba, is_srgb);
    let thickness = (font_px / 14.0).round().max(1.0);
    let underline_y = ((lh + font_px) * 0.5).round().min(lh - thickness);
    let strike_y = (lh * 0.5).round();

    let visual = visual_cell_extents(line, segments, cw);

    let mut quads = Vec::new();
    for (c, cell) in line.cells.iter().enumerate() {
        let (fg, bg) = cell_colors(palette, cell);
        let (x, w) = visual[c].map_or((c as f32 * cw, cw), |(l, r)| (l, r - l));
        if bg != palette.background && w > 0.0 {
            quads.push((x, 0.0, w, lh, target(bg)));
        }
        let fg_color = target(fg);
        if !cell.flags.contains(CellFlags::HIDDEN) {
            try_render_box_or_block(cell.c, x, 0.0, w, lh, fg_color, &mut quads);
        }
        if w <= 0.0 {
            continue;
        }
        if cell.flags.contains(CellFlags::DOUBLE_UNDERLINE) {
            quads.push((x, underline_y - thickness * 2.0, w, thickness, fg_color));
            quads.push((x, underline_y, w, thickness, fg_color));
        } else if cell.link_hover || cell.flags.intersects(CellFlags::ALL_UNDERLINES) {
            quads.push((x, underline_y, w, thickness, fg_color));
        }
        if cell.flags.contains(CellFlags::STRIKEOUT) {
            quads.push((x, strike_y, w, thickness, fg_color));
        }
    }
    quads
}
