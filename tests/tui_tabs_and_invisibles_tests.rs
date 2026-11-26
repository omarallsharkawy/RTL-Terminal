//! Regressions for TUI rendering artifacts: tab placeholders (`alacritty_terminal` stores `\t`
//! in the grid so copy preserves literal tabs), invisible zero-width formatters, NBSP padding
//! and combining/variation-selector marks in LTR runs.
use alacritty_terminal::index::Side;
use cosmic_text::{fontdb, FontSystem};
use twitty::color::Palette;
use twitty::render::text::fonts::new_font_system;
use twitty::render::{build_row_quads, measure_char_width, CachedSegment, RowShaper};
use twitty::terminal::Terminal;

const NASKH: &[u8] = include_bytes!("../assets/fonts/NotoNaskhArabic-Regular.ttf");
const MONO: &[u8] = include_bytes!("golden/fonts/NotoSansMono-Subset.ttf");

fn subset_font_system() -> FontSystem {
    let mut db = fontdb::Database::new();
    db.load_font_data(MONO.to_vec());
    db.load_font_data(NASKH.to_vec());
    db.set_monospace_family("Noto Sans Mono");
    db.set_sans_serif_family("Noto Sans Mono");
    db.set_serif_family("Noto Sans Mono");
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

fn shape_with(bytes: &[u8], cols: usize, fs: &mut FontSystem) -> (Vec<CachedSegment>, f32) {
    let mut term = Terminal::new(cols, 3, |_| {});
    term.process_bytes(bytes);
    term.process_bytes(b"\r\n");
    let (lines, _) = term.snapshot();
    let palette = Palette::default();
    let cw = measure_char_width(fs, None, 15.0, 23.0);
    let mut shaper = RowShaper {
        font_system: fs,
        palette: &palette,
        font_px: 15.0,
        char_width: cw,
        line_height: 23.0,
        family: None,
    };
    (shaper.build_row(&lines[0], None), cw)
}

fn glyph_x(segs: &[CachedSegment], col: usize) -> f32 {
    let seg = segs
        .iter()
        .find(|s| col >= s.seg_start && col < s.seg_end)
        .unwrap_or_else(|| panic!("no segment for column {col}"));
    let byte = seg.cell_bytes[col - seg.seg_start];
    seg.buffer
        .layout_runs()
        .find_map(|run| {
            run.glyphs
                .iter()
                .find(|g| g.w > 0.0 && byte >= g.start && byte < g.end)
                .map(|g| seg.seg_x + g.x)
        })
        .unwrap_or_else(|| panic!("no glyph for column {col}"))
}

fn assert_no_notdef(segs: &[CachedSegment]) {
    for seg in segs {
        for run in seg.buffer.layout_runs() {
            for g in run.glyphs {
                assert_ne!(
                    g.glyph_id,
                    0,
                    "rendered .notdef glyph for {:?} at seg {}..{}",
                    &run.text[g.start..g.end],
                    seg.seg_start,
                    seg.seg_end
                );
            }
        }
    }
}

#[test]
fn tabs_in_tui_menu_render_without_notdef_and_split_columns() {
    // Reproduces TUI menus (agy, Claude Code, Codex, Ink/curses) that use `\t` to jump across
    // blank cells between a command name and its description: `alacritty_terminal` leaves `\t`
    // in cols 8, 16, 24. Those must not draw `.notdef` boxes or merge the two columns.
    for mut fs in [new_font_system(), subset_font_system()] {
        let (segs, cw) = shape_with(
            b"> /model\t\t\t   Set a model, or run a single ...",
            80,
            &mut fs,
        );
        assert_no_notdef(&segs);
        assert_eq!(segs.len(), 2, "two columns separated by a wide tab gap");
        assert_eq!((segs[0].seg_start, segs[0].seg_end), (0, 8));
        assert_eq!(segs[1].seg_start, 35);
        assert!((glyph_x(&segs, 35) - 35.0 * cw).abs() < 0.5);
    }
}

#[test]
fn short_tab_stays_grid_aligned_and_copy_preserves_literal_tab() {
    let mut term = Terminal::new(40, 3, |_| {});
    term.process_bytes(b"abc\tdef\r\n");
    // Selection still reads the raw grid, so copying preserves the literal tab.
    term.start_selection(0, 0, Side::Left, 1);
    term.update_selection(10, 0, Side::Right);
    assert_eq!(term.selection_text().as_deref(), Some("abc\tdef"));

    for mut fs in [new_font_system(), subset_font_system()] {
        let (segs, cw) = shape_with(b"abc\tdef", 40, &mut fs);
        assert_no_notdef(&segs);
        assert_eq!(segs.len(), 1);
        assert!((glyph_x(&segs, 8) - 8.0 * cw).abs() < 0.5);
    }
}

#[test]
fn tabs_in_arabic_rows_do_not_bloat_spacing_or_merge_distant_columns() {
    let mut fs = new_font_system();

    // Short tab (< 8 blank cells): stays within its 12-cell grid without cosmic-text expanding
    // `\t` to 8 extra spaces on top of the grid's spaces.
    let (segs, cw) = shape_with("مرحبا\tعالم".as_bytes(), 40, &mut fs);
    assert_no_notdef(&segs);
    assert_eq!(segs.len(), 1);
    let max_x = segs[0]
        .buffer
        .layout_runs()
        .flat_map(|r| r.glyphs.iter())
        .map(|g| segs[0].seg_x + g.x + g.w)
        .fold(0.0_f32, f32::max);
    assert!(
        max_x <= 12.0 * cw + 1.0,
        "short tab bloated RTL row to {max_x}px (grid is {}px)",
        12.0 * cw
    );

    // Wide tab gap (>= 8 blank cells): splits into two independent columns so the right
    // column stays on the right instead of swapping with the left column.
    let (segs, cw) = shape_with("مرحبا\t\t\tعالم".as_bytes(), 40, &mut fs);
    assert_no_notdef(&segs);
    assert_eq!(segs.len(), 2, "wide tab gap must split RTL columns");
    assert_eq!((segs[0].seg_start, segs[0].seg_end), (0, 5));
    assert_eq!((segs[1].seg_start, segs[1].seg_end), (24, 28));
    assert!((segs[1].seg_x - 24.0 * cw).abs() < 0.5);

    // Selected Arabic+tab row has contiguous background quads with no gaps or overlaps.
    let mut term = Terminal::new(40, 3, |_| {});
    term.process_bytes("مرحبا\tعالم\r\n".as_bytes());
    term.start_selection(0, 0, Side::Left, 1);
    term.update_selection(11, 0, Side::Right);
    let (lines, _) = term.snapshot();
    let palette = Palette::default();
    let mut shaper = RowShaper {
        font_system: &mut fs,
        palette: &palette,
        font_px: 15.0,
        char_width: cw,
        line_height: 23.0,
        family: None,
    };
    let segs = shaper.build_row(&lines[0], None);
    let sel = palette.selection.to_array();
    let mut q: Vec<_> = build_row_quads(&palette, false, (cw, 23.0, 15.0), &lines[0], &segs)
        .into_iter()
        .filter(|q| q.4 == sel && q.2 > 0.0)
        .collect();
    q.sort_by(|a, b| a.0.total_cmp(&b.0));
    for pair in q.windows(2) {
        let gap = pair[1].0 - (pair[0].0 + pair[0].2);
        assert!(gap.abs() < 0.01, "gap {gap} in selected RTL+tab row");
    }
}

#[test]
fn invisible_format_marks_do_not_shatter_runs_or_break_blank_cells() {
    let mut term = Terminal::new(40, 3, |_| {});
    term.process_bytes(" \u{200B} \u{FEFF} \u{2060} \u{2066}a\u{200B}b\u{FEFF}c".as_bytes());
    let (lines, _) = term.snapshot();
    for col in 0..4 {
        assert!(
            lines[0].cells[col].is_blank(),
            "col {col} with only ignored marks must stay blank"
        );
    }

    let mut fs = new_font_system();
    let (segs, cw) = shape_with(
        "\u{FEFF}hello\u{200B} world\u{2060}!".as_bytes(),
        40,
        &mut fs,
    );
    assert_no_notdef(&segs);
    assert_eq!(segs.len(), 1, "ignored marks must not shatter an ASCII run");
    assert!(!segs[0].has_rtl, "U+FEFF BOM must not trigger RTL");
    assert!((glyph_x(&segs, 6) - 6.0 * cw).abs() < 0.5);
}

#[test]
fn nbsp_padding_splits_columns_and_popup_regions() {
    let mut fs = new_font_system();
    let (segs, cw) = shape_with(
        "left\u{00A0}\u{00A0}\u{00A0}\u{00A0}\u{00A0}\u{00A0}\u{00A0}\u{00A0}right".as_bytes(),
        40,
        &mut fs,
    );
    assert_no_notdef(&segs);
    assert_eq!(segs.len(), 2);
    assert!((glyph_x(&segs, 12) - 12.0 * cw).abs() < 0.5);

    // Popup padded with NBSP next to leftover Arabic keeps popup text in place.
    let (segs, cw) = shape_with(
        "│ \x1b[48;5;234mخل\x1b[48;5;237m\u{00A0}\u{00A0}Connect\x1b[0m".as_bytes(),
        40,
        &mut fs,
    );
    assert!((glyph_x(&segs, 6) - 6.0 * cw).abs() < 1.0);
}

#[test]
fn combining_marks_in_ltr_run_keep_grid_alignment() {
    let mut fs = new_font_system();
    let (segs, cw) = shape_with("cafe\u{0301} ok".as_bytes(), 40, &mut fs);
    assert_no_notdef(&segs);
    assert_eq!(segs.len(), 1, "combining accent must not shatter LTR run");
    assert!((glyph_x(&segs, 5) - 5.0 * cw).abs() < 0.5);
}
