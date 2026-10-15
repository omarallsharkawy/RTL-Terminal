//! Regressions from the renderer audit: hidden text, text color mode, wide cursor and the
//! preedit covering the text under it.
use alacritty_terminal::term::cell::Flags as CellFlags;
use glyphon::{ColorMode, TextBounds};
use twitty::color::Palette;
use twitty::ime::bounds_outside;
use twitty::render::{cell_colors, cursor_cell_width, text_color_mode};
use twitty::terminal::{CellData, Terminal};

fn hidden_cell() -> CellData {
    let mut cell = CellData::blank();
    cell.c = 'p';
    cell.flags = CellFlags::HIDDEN;
    cell
}

#[test]
fn hidden_text_stays_hidden_when_selected_or_matched() {
    let palette = Palette::default();
    let mut selected = hidden_cell();
    selected.selected = true;
    let (fg, bg) = cell_colors(&palette, &selected);
    assert_eq!(fg, bg, "selected hidden text must stay invisible");

    for m in [1, 2] {
        let mut matched = hidden_cell();
        matched.search_match = m;
        let (fg, bg) = cell_colors(&palette, &matched);
        assert_eq!(fg, bg, "search-matched hidden text must stay invisible");
    }
}

#[test]
fn glyph_colors_match_quad_colors_on_a_non_srgb_surface() {
    // Quads write sRGB values as-is to a non-sRGB surface; glyphs must not be linearized there,
    // or text comes out darker than the palette (and than the block cursor it is drawn on).
    assert_eq!(text_color_mode(false), ColorMode::Web);
    assert_eq!(text_color_mode(true), ColorMode::Accurate);
}

#[test]
fn block_cursor_covers_both_cells_of_a_wide_char() {
    let mut term = Terminal::new(20, 2, |_| {});
    term.process_bytes("a中b".as_bytes());
    let (lines, _) = term.snapshot();
    let line = &lines[0];
    assert_eq!(cursor_cell_width(line, 0, 9.0), 9.0);
    assert_eq!(cursor_cell_width(line, 1, 9.0), 18.0);
    assert_eq!(cursor_cell_width(line, 3, 9.0), 9.0);
    // Past the end of the row.
    assert_eq!(cursor_cell_width(line, 99, 9.0), 9.0);
}

#[test]
fn preedit_hides_the_text_under_it() {
    let pane = TextBounds {
        left: 0,
        top: 0,
        right: 200,
        bottom: 100,
    };
    let [left, right] = bounds_outside(pane, 40.4, 80.6);
    assert_eq!((left.left, left.right), (0, 40));
    assert_eq!((right.left, right.right), (81, 200));
    assert_eq!(
        (left.top, left.bottom, right.top, right.bottom),
        (0, 100, 0, 100)
    );
    // A hole wider than the pane leaves nothing on either side.
    let [left, right] = bounds_outside(pane, -10.0, 250.0);
    assert!(left.right <= left.left && right.right <= right.left);
}
