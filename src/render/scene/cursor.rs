//! The terminal cursor: its shape, where it goes and its quads.

use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::vte::ansi::CursorShape;

use super::boxdraw::try_render_box_or_block;
use crate::color::Palette;
use crate::render::metrics::CellMetrics;
use crate::render::style::{cell_colors, target_color, Quad};
use crate::terminal::{CellData, LineData};

/// The configured style ("block", "beam"/"bar", "underline") replaces the default block; shapes
/// an app asks for (vim insert mode's beam, for example) are kept.
pub fn effective_cursor_shape(style: &str, shape: CursorShape) -> CursorShape {
    if shape != CursorShape::Block {
        return shape;
    }
    match style {
        "beam" | "bar" => CursorShape::Beam,
        "underline" => CursorShape::Underline,
        _ => CursorShape::Block,
    }
}

/// Left edge of a cursor anchored at `x`. A caret that extends left (after Arabic) sits left
/// of the anchor by its own width.
pub fn cursor_left(x: f32, extends_left: bool, shape: CursorShape, m: &CellMetrics) -> f32 {
    let mut x = x;
    if extends_left {
        x -= match shape {
            CursorShape::Beam => m.stroke(),
            _ => m.char_width,
        };
    }
    x.max(0.0)
}

/// Quads of a cursor at (x, y) that is `w` wide; unfocused cursors are hollow.
pub fn cursor_quads(
    (x, y, w): (f32, f32, f32),
    shape: CursorShape,
    focused: bool,
    m: &CellMetrics,
    color: [f32; 4],
    out: &mut Vec<Quad>,
) {
    let h = m.line_height;
    let t = m.stroke();
    let hollow = |out: &mut Vec<Quad>| {
        out.push((x, y, w, t, color));
        out.push((x, y + h - t, w, t, color));
        out.push((x, y, t, h, color));
        out.push((x + w - t, y, t, h, color));
    };
    match shape {
        CursorShape::Hidden => {}
        _ if !focused => hollow(out),
        CursorShape::Block => out.push((x, y, w, h, color)),
        CursorShape::HollowBlock => hollow(out),
        CursorShape::Beam => out.push((x, y, t, h, color)),
        CursorShape::Underline => out.push((x, y + h - t * 2.0, w, t * 2.0, color)),
    }
}

/// Width of the cursor on `col`: two cells on a double-width character, so a block cursor
/// covers all of the glyph it recolors.
pub fn cursor_cell_width(line: &LineData, col: usize, cw: f32) -> f32 {
    match line.cells.get(col) {
        Some(cell) if cell.flags.contains(CellFlags::WIDE_CHAR) => cw * 2.0,
        _ => cw,
    }
}

/// Box drawing or block element of `cell` in its background color, for drawing on top of a
/// block cursor at `rect` (x, y, w, h). Text under the cursor gets the same recolor when shaped;
/// without this, geometry would vanish beneath the cursor.
pub fn geometry_under_block_cursor(
    palette: &Palette,
    is_srgb: bool,
    cell: &CellData,
    (x, y, w, h): (f32, f32, f32, f32),
) -> Vec<Quad> {
    let mut quads = Vec::new();
    if !cell.flags.contains(CellFlags::HIDDEN) {
        let (_, bg) = cell_colors(palette, cell);
        let color = target_color(bg, is_srgb);
        try_render_box_or_block(cell.c, x, y, w, h, color, &mut quads);
    }
    quads
}
