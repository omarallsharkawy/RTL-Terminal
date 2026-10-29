//! Cell colors and the colored rectangles everything but text is drawn with.

use alacritty_terminal::term::cell::Flags as CellFlags;

use crate::color::{Palette, Rgba};
use crate::terminal::CellData;

/// A filled rectangle: x, y, width, height (physical pixels) and its color in surface space.
pub type Quad = (f32, f32, f32, f32, [f32; 4]);

/// `rgba` as the surface expects it: linear on an sRGB surface, as-is otherwise.
pub(crate) fn target_color(rgba: Rgba, is_srgb: bool) -> [f32; 4] {
    if is_srgb {
        rgba.to_linear()
    } else {
        rgba.to_array()
    }
}

/// Resolve a cell's effective (foreground, background) honoring INVERSE, DIM and HIDDEN.
pub fn cell_colors(palette: &Palette, cell: &CellData) -> (Rgba, Rgba) {
    let mut fg = palette.resolve(cell.fg, false);
    let mut bg = palette.resolve(cell.bg, true);
    if cell.flags.contains(CellFlags::INVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.flags.contains(CellFlags::DIM) {
        fg = fg.mix(bg, 0.34);
    }
    if cell.search_match > 0 {
        bg = if cell.search_match > 1 {
            palette.search_focus
        } else {
            palette.search_match
        };
        fg = palette.background;
    }
    if cell.selected {
        bg = palette.selection;
    }
    // Last, so selecting or searching never reveals concealed text (passwords, `\e[8m`).
    if cell.flags.contains(CellFlags::HIDDEN) {
        fg = bg;
    }
    (fg, bg)
}
