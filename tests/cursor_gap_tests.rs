//! The caret after Arabic text extends left across the spaces typed after it, but not across a
//! border or other content between the text and the cursor.
use cosmic_text::FontSystem;
use twitty::color::Palette;
use twitty::render::{CachedRow, RowCursor, RowShaper};
use twitty::terminal::Terminal;

const CW: f32 = 9.0;

fn anchor(text: &str, cursor_visible: bool) -> (f32, bool, usize) {
    let (row, col) = shaped(text, cursor_visible);
    let (x, extends_left) = row.cursor_anchor(col, CW);
    (x, extends_left, col)
}

/// Visual x-range of the glyph drawn for cell `col`.
fn glyph_span(row: &CachedRow, col: usize) -> (f32, f32) {
    let seg = row
        .segments
        .iter()
        .find(|s| col >= s.seg_start && col < s.seg_end)
        .expect("segment");
    let target = seg.cell_bytes[col - seg.seg_start];
    seg.buffer
        .layout_runs()
        .find_map(|run| {
            run.glyphs
                .iter()
                .find(|g| target >= g.start && target < g.end)
                .map(|g| (seg.seg_x + g.x, seg.seg_x + g.x + g.w))
        })
        .expect("glyph")
}

fn shaped(text: &str, cursor_visible: bool) -> (CachedRow, usize) {
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
    (row, cursor.col)
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

#[test]
fn caret_after_english_typed_into_arabic_sits_right_of_it() {
    // "❯ مرحبا hello": the phrase reads right to left, so "hello" is drawn left of the Arabic
    // word and the caret follows its last letter, between the two.
    let (row, col) = shaped("❯ مرحبا hello", true);
    assert_eq!(col, 13);
    let (x, extends_left) = row.cursor_anchor(col, CW);
    assert!(!extends_left);
    // Glyph widths depend on the platform's fonts: compare with the glyphs, not the grid.
    let last_letter = glyph_span(&row, 12);
    let arabic_left = (2..7)
        .map(|c| glyph_span(&row, c).0)
        .fold(f32::MAX, f32::min);
    assert!(
        (x - last_letter.1).abs() < 0.5,
        "caret at {x}, \"hello\" ends at {last_letter:?}"
    );
    assert!(
        x <= arabic_left + 0.5,
        "caret at {x}, Arabic starts at {arabic_left}"
    );
}

#[test]
fn caret_after_a_space_typed_after_arabic_sits_on_the_far_left() {
    // "❯ مرحبا hello يا ": the next Arabic word goes left of everything typed so far.
    let (row, col) = shaped("❯ مرحبا hello يا ", true);
    let (x, extends_left) = row.cursor_anchor(col, CW);
    assert!(extends_left);
    let leftmost_text = (2..17)
        .filter(|&c| c != 7 && c != 13)
        .map(|c| glyph_span(&row, c).0)
        .fold(f32::MAX, f32::min);
    assert!(
        x <= leftmost_text + 0.5,
        "caret at {x}, text starts at {leftmost_text}"
    );
}
