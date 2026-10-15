//! The caret after Arabic text extends left across the spaces typed after it, but not across a
//! border or other content between the text and the cursor.
use cosmic_text::FontSystem;
use twitty::color::Palette;
use twitty::renderer::{CachedRow, RowCursor, RowShaper};
use twitty::terminal::Terminal;

const CW: f32 = 9.0;

fn anchor(text: &str, cursor_visible: bool) -> (f32, bool, usize) {
    let mut term = Terminal::new(30, 3, |_| {});
    term.process_bytes(text.as_bytes());
    let (lines, cursor) = term.snapshot();
    let mut font_system = FontSystem::new();
    let palette = Palette::default();
    let mut shaper = RowShaper {
        font_system: &mut font_system,
        palette: &palette,
        font_px: 15.0,
        char_width: CW,
        line_height: 23.0,
        family: None,
    };
    let row_cursor = cursor_visible.then_some(RowCursor {
        col: cursor.col,
        block: false,
    });
    let segments = shaper.build_row(&lines[0], row_cursor);
    let row = CachedRow {
        blank_cells: lines[0].cells.iter().map(|c| c.is_blank()).collect(),
        segments,
        ..Default::default()
    };
    let (x, extends_left) = row.cursor_anchor(cursor.col, CW);
    (x, extends_left, cursor.col)
}

#[test]
fn caret_after_a_border_stays_on_its_cell() {
    // "مرحبا │ " with the cursor after the border: the caret belongs at its own cell, not
    // left of the Arabic word on the other side of the border.
    for visible in [true, false] {
        let (x, extends_left, col) = anchor("مرحبا │ ", visible);
        assert_eq!(col, 8);
        assert!(!extends_left, "caret must not jump across the border");
        assert!((x - col as f32 * CW).abs() < 0.5, "caret at {x}");
    }
}

#[test]
fn hidden_cursor_caret_still_follows_spaces_after_arabic() {
    // With the cursor hidden the row is shaped without it (no segment extension); the caret
    // (used for the IME preedit) still sits left of the Arabic word.
    let (x, extends_left, _) = anchor("> هلا  ", false);
    assert!(extends_left);
    assert!(x < 2.0 * CW + 0.5, "caret at {x}");
}
