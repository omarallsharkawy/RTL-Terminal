//! Cell geometry without a window: grid size, pixel to cell, tab bar.
use alacritty_terminal::index::Side;
use twitty::layout::Rect;
use twitty::render::CellMetrics;

fn metrics() -> CellMetrics {
    CellMetrics {
        font_size: 14.0,
        scale: 1.0,
        char_width: 9.0,
        line_height: 22.0,
        padding_left: 8.0,
        padding_top: 6.0,
    }
}

#[test]
fn grid_size_fits_cells_inside_the_padding() {
    let rect = Rect::new(
        0.0,
        0.0,
        8.0 * 2.0 + 9.0 * 80.0 + 5.0,
        6.0 * 2.0 + 22.0 * 24.0 + 3.0,
    );
    assert_eq!(metrics().grid_size(rect), (80, 24));
    // A tiny pane gets the smallest grid that fits, never one larger than the pane.
    assert_eq!(metrics().grid_size(Rect::new(0.0, 0.0, 1.0, 1.0)), (2, 1));
    let small = Rect::new(0.0, 0.0, 8.0 * 2.0 + 9.0 * 5.0, 6.0 * 2.0 + 22.0 * 2.0);
    assert_eq!(metrics().grid_size(small), (5, 2));
}

#[test]
fn pixels_map_to_cells_and_sides() {
    let m = metrics();
    let rect = Rect::new(100.0, 50.0, 800.0, 600.0);
    assert_eq!(
        m.pixel_to_cell(rect, 108.0 + 9.0 * 3.0 + 1.0, 56.0 + 22.0 * 2.0 + 1.0),
        (3, 2)
    );
    // Left of the padding clamps to the first cell.
    assert_eq!(m.pixel_to_cell(rect, 0.0, 0.0), (0, 0));
    assert_eq!(m.pixel_side(rect, 108.0 + 9.0 * 3.0 + 2.0), Side::Left);
    assert_eq!(m.pixel_side(rect, 108.0 + 9.0 * 3.0 + 7.0), Side::Right);
}

#[test]
fn tab_bar_appears_with_two_tabs() {
    let m = metrics();
    assert_eq!(m.tab_bar_height(1), 0.0);
    assert_eq!(m.tab_bar_height(2), 22.0);
    let content = m.content_rect(2, (800.0, 600.0));
    assert_eq!((content.y, content.h), (22.0, 578.0));
}
