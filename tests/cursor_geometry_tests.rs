//! Box drawing and block elements under a block cursor are redrawn in the cell background on
//! top of the cursor, like text glyphs, instead of disappearing beneath it.
use alacritty_terminal::term::cell::Flags as CellFlags;
use twitty::color::Palette;
use twitty::renderer::{cell_colors, geometry_under_block_cursor};
use twitty::terminal::CellData;

fn cell(c: char) -> CellData {
    let mut cell = CellData::blank();
    cell.c = c;
    cell
}

#[test]
fn box_char_under_block_cursor_is_redrawn_in_the_background_color() {
    let palette = Palette::default();
    let rect = (10.0, 20.0, 9.0, 22.0);
    let border = cell('│');
    let quads = geometry_under_block_cursor(&palette, false, &border, rect);
    assert!(!quads.is_empty(), "│ must stay visible on the cursor");
    let (_, bg) = cell_colors(&palette, &border);
    assert!(quads.iter().all(|q| q.4 == bg.to_array()));

    // Text is recolored while shaping; nothing to add for it.
    assert!(geometry_under_block_cursor(&palette, false, &cell('a'), rect).is_empty());

    let mut hidden = cell('█');
    hidden.flags = CellFlags::HIDDEN;
    assert!(geometry_under_block_cursor(&palette, false, &hidden, rect).is_empty());
}
