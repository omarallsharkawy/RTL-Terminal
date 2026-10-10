//! Mouse selection, transparency config, and the Arabic caret/space/hash fixes.
use alacritty_terminal::index::Side;
use alacritty_terminal::vte::ansi::CursorShape;
use cosmic_text::FontSystem;
use twitty::color::Palette;
use twitty::config::TwittyConfig;
use twitty::renderer::{cell_colors, CachedRow, RowCursor, RowShaper};
use twitty::terminal::{CursorState, Terminal};

const CW: f32 = 9.0;

fn term_with(text: &str) -> Terminal {
    let mut term = Terminal::new(40, 4, |_| {});
    term.process_bytes(text.as_bytes());
    term
}

#[test]
fn drag_selects_characters() {
    let mut term = term_with("hello world");
    term.start_selection(0, 0, Side::Left, 1);
    term.update_selection(4, 0, Side::Right);
    assert_eq!(term.selection_text().as_deref(), Some("hello"));

    let (lines, _) = term.snapshot();
    assert!(lines[0].cells[..5].iter().all(|c| c.selected));
    assert!(!lines[0].cells[5].selected);

    term.clear_selection();
    assert!(!term.has_selection());
    let (lines, _) = term.snapshot();
    assert!(lines[0].cells.iter().all(|c| !c.selected));
}

#[test]
fn single_click_selects_nothing() {
    let mut term = term_with("hello");
    term.start_selection(2, 0, Side::Left, 1);
    assert_eq!(term.selection_text(), None);
}

#[test]
fn double_click_selects_word_and_triple_selects_line() {
    let mut term = term_with("git commit 7abaf40");
    term.start_selection(13, 0, Side::Left, 2);
    assert_eq!(term.selection_text().as_deref(), Some("7abaf40"));

    term.start_selection(1, 0, Side::Left, 3);
    assert_eq!(
        term.selection_text().as_deref().map(str::trim_end),
        Some("git commit 7abaf40")
    );
}

#[test]
fn arabic_word_is_copied_in_logical_order() {
    let mut term = term_with("قال مرحبا بك");
    term.start_selection(5, 0, Side::Left, 2);
    assert_eq!(term.selection_text().as_deref(), Some("مرحبا"));
}

#[test]
fn selected_cells_get_the_selection_background() {
    let mut term = term_with("hi");
    term.start_selection(0, 0, Side::Left, 1);
    term.update_selection(1, 0, Side::Right);
    let (lines, _) = term.snapshot();
    let palette = Palette::default();
    assert_eq!(
        cell_colors(&palette, &lines[0].cells[0]).1,
        palette.selection
    );
    assert_eq!(
        cell_colors(&palette, &lines[0].cells[2]).1,
        palette.background
    );
}

#[test]
fn config_has_transparency_and_keeps_unknown_keys() {
    let config: TwittyConfig =
        serde_json::from_str(r#"{"font_size": 13.0, "font_family": "Iosevka"}"#).unwrap();
    assert_eq!(config.font_size, 13.0);
    assert!((config.background_opacity - 0.78).abs() < 1e-6);
    assert_eq!(config.cursor_style, "block");

    let saved = serde_json::to_string(&config).unwrap();
    assert!(
        saved.contains("Iosevka"),
        "saving must not drop keys: {saved}"
    );
}

fn shaped(text: &str) -> (CachedRow, CursorState) {
    let term = term_with(text);
    let (lines, cursor) = term.snapshot();
    let mut font_system = FontSystem::new();
    let palette = Palette::default();
    let mut shaper = RowShaper {
        font_system: &mut font_system,
        palette: &palette,
        font_px: 15.0,
        char_width: CW,
        line_height: 23.0,
    };
    let row_cursor = RowCursor {
        col: cursor.col,
        block: false,
    };
    let segments = shaper.build_row(&lines[0], Some(row_cursor));
    let row = CachedRow {
        segments,
        ..Default::default()
    };
    (
        row,
        CursorState {
            shape: CursorShape::Beam,
            ..cursor
        },
    )
}

/// Left edge (absolute x) of the glyph that holds `col`.
fn glyph_left(row: &CachedRow, col: usize) -> f32 {
    let seg = row
        .segments
        .iter()
        .find(|s| col >= s.seg_start && col < s.seg_end)
        .expect("segment for column");
    let target = seg.cell_bytes[col - seg.seg_start];
    seg.buffer
        .layout_runs()
        .find_map(|run| {
            run.glyphs
                .iter()
                .find(|g| target >= g.start && target < g.end)
                .map(|g| seg.seg_x + g.x)
        })
        .expect("glyph for column")
}

#[test]
fn caret_sits_left_of_arabic_and_right_of_latin() {
    let (row, cursor) = shaped("> هلا");
    let (x, extends_left) = row.cursor_anchor(cursor.col, CW);
    // "> " is two cells; the Arabic word starts right after, so its left edge is at 2 cells.
    assert!(extends_left, "caret after Arabic must extend to the left");
    assert!(
        (x - 2.0 * CW).abs() < CW * 0.6,
        "caret at {x}, expected near the word's left edge"
    );

    let (row, cursor) = shaped("> hello");
    let (x, extends_left) = row.cursor_anchor(cursor.col, CW);
    assert!(!extends_left);
    assert!((x - 7.0 * CW).abs() < 0.5, "Latin caret at {x}");
}

#[test]
fn space_typed_after_arabic_stays_on_its_left() {
    let (row, cursor) = shaped("> هلا ");
    let first_letter = glyph_left(&row, 2);
    let space = glyph_left(&row, 5);
    assert!(
        space < first_letter,
        "space ({space}) must sit left of the word ({first_letter})"
    );
    let (x, extends_left) = row.cursor_anchor(cursor.col, CW);
    assert!(extends_left && x <= first_letter + 0.5, "caret at {x}");
}

#[test]
fn hashes_inside_arabic_are_not_torn() {
    let (row, _) = shaped("بس 43f9bb8 وهو");
    // Columns 3..10 hold "43f9bb8": they must read left to right, one cell apart.
    let xs: Vec<f32> = (3..10).map(|c| glyph_left(&row, c)).collect();
    for pair in xs.windows(2) {
        let step = pair[1] - pair[0];
        assert!(
            step > 0.0 && step < CW * 1.5,
            "hash glyphs out of order or split: {xs:?}"
        );
    }
}
