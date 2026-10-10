//! Backgrounds that follow RTL glyphs, trailing neutrals after Arabic, and Arabic-led output.
use alacritty_terminal::index::Side;
use cosmic_text::FontSystem;
use twitty::color::Palette;
use twitty::renderer::{build_row_quads, CachedRow, CachedSegment, RowCursor, RowShaper};
use twitty::terminal::{LineData, Terminal};

const CW: f32 = 9.0;
const LH: f32 = 23.0;

type Quad = (f32, f32, f32, f32, [f32; 4]);

struct Shaped {
    term: Terminal,
    line: LineData,
    row: CachedRow,
    palette: Palette,
}

/// Print `text` on row 0. With `cursor_on_row` the cursor stays right after it, otherwise it
/// moves to the next row (the line is finished output).
fn shaped(text: &str, cursor_on_row: bool) -> Shaped {
    let mut term = Terminal::new(40, 4, |_| {});
    term.process_bytes(text.as_bytes());
    if !cursor_on_row {
        term.process_bytes(b"\r\n");
    }
    reshape(term)
}

fn reshape(term: Terminal) -> Shaped {
    let (lines, cursor) = term.snapshot();
    let mut font_system = FontSystem::new();
    let palette = Palette::default();
    let mut shaper = RowShaper {
        font_system: &mut font_system,
        palette: &palette,
        font_px: 15.0,
        char_width: CW,
        line_height: LH,
        family: None,
    };
    let row_cursor = (cursor.row == 0).then_some(RowCursor {
        col: cursor.col,
        block: false,
    });
    let segments = shaper.build_row(&lines[0], row_cursor);
    Shaped {
        term,
        line: lines[0].clone(),
        row: CachedRow {
            segments,
            ..Default::default()
        },
        palette,
    }
}

fn glyph_span(seg: &CachedSegment, col: usize) -> (f32, f32) {
    let target = seg.cell_bytes[col - seg.seg_start];
    seg.buffer
        .layout_runs()
        .find_map(|run| {
            run.glyphs
                .iter()
                .find(|g| target >= g.start && target < g.end)
                .map(|g| (seg.seg_x + g.x, seg.seg_x + g.x + g.w))
        })
        .expect("glyph for column")
}

/// Visual x-range of the glyphs holding columns `from..=to`.
fn span(row: &CachedRow, from: usize, to: usize) -> (f32, f32) {
    let seg = row
        .segments
        .iter()
        .find(|s| from >= s.seg_start && from < s.seg_end)
        .expect("segment");
    (from..=to)
        .map(|c| glyph_span(seg, c))
        .fold((f32::MAX, f32::MIN), |(l, r), (a, b)| (l.min(a), r.max(b)))
}

fn bg_quads(s: &Shaped) -> Vec<Quad> {
    let quads = build_row_quads(&s.palette, false, (CW, LH, 15.0), &s.line, &s.row.segments);
    let sel = s.palette.selection.to_array();
    let mut q: Vec<_> = quads.into_iter().filter(|q| q.4 == sel).collect();
    q.sort_by(|a, b| a.0.total_cmp(&b.0));
    q
}

#[test]
fn selection_follows_the_arabic_word_glyphs() {
    let mut s = shaped("قال مرحبا بك", false);
    s.term.start_selection(5, 0, Side::Left, 2);
    assert_eq!(s.term.selection_text().as_deref(), Some("مرحبا"));
    let s = reshape(s.term);
    let q = bg_quads(&s);
    let left = q.iter().map(|q| q.0).fold(f32::MAX, f32::min);
    let right = q.iter().map(|q| q.0 + q.2).fold(f32::MIN, f32::max);
    let (gl, gr) = span(&s.row, 4, 8);
    assert!((left - gl).abs() <= 1.0, "bg left {left} vs glyphs {gl}");
    assert!((right - gr).abs() <= 1.0, "bg right {right} vs glyphs {gr}");
}

#[test]
fn rtl_cell_backgrounds_have_no_holes_or_overlaps() {
    for (text, last) in [("مرحبا بك", 7), ("لا ّ قال 7abaf40 وهو", 18)] {
        let mut s = shaped(text, false);
        s.term.start_selection(0, 0, Side::Left, 1);
        s.term.update_selection(last, 0, Side::Right);
        let s = reshape(s.term);
        let q = bg_quads(&s);
        assert!(!q.is_empty());
        for pair in q.windows(2) {
            let gap = pair[1].0 - (pair[0].0 + pair[0].2);
            let spans: Vec<_> = q.iter().map(|q| (q.0, q.2)).collect();
            assert!(gap.abs() < 0.01, "gap {gap} in {text}: {spans:?}");
        }
    }
}

#[test]
fn colored_rtl_line_fills_its_cells_to_the_grid_edge() {
    // A TUI highlight or selection over a whole Arabic line must not leave an unpainted hole
    // between the (narrower) glyphs and the end of the line's cells.
    for (text, last) in [
        ("بس فيه حاجة صغيرة:", 17),
        ("بقى مفصول عن الخلفية #15161e", 27),
    ] {
        let mut s = shaped(text, false);
        s.term.start_selection(0, 0, Side::Left, 1);
        s.term.update_selection(last, 0, Side::Right);
        let s = reshape(s.term);
        let q = bg_quads(&s);
        let left = q.iter().map(|q| q.0).fold(f32::MAX, f32::min);
        let right = q.iter().map(|q| q.0 + q.2).fold(f32::MIN, f32::max);
        assert!(left <= 0.5, "{text}: starts at {left}");
        assert!(
            right >= (last + 1) as f32 * CW - 0.5,
            "{text}: painted up to {right}, cells end at {}",
            (last + 1) as f32 * CW
        );
    }
}

#[test]
fn trailing_symbol_stays_on_the_arabic_side() {
    for cursor_on_row in [false, true] {
        for (text, last) in [("تم الحفظ ✓", 9), ("مرحبا!", 5), ("جاهزة.", 5)] {
            let s = shaped(text, cursor_on_row);
            let neutral = span(&s.row, last, last).0;
            let first_letter = span(&s.row, 0, 0).0;
            assert!(
                neutral < first_letter,
                "{text:?} (cursor on row: {cursor_on_row}): {neutral} vs {first_letter}"
            );
        }
    }
}

#[test]
fn leading_symbol_stays_on_the_left() {
    let s = shaped("✓ تم", false);
    assert!(span(&s.row, 0, 0).0 < span(&s.row, 2, 3).0);
}

#[test]
fn arabic_led_output_reads_right_to_left_but_typing_does_not() {
    let s = shaped("مرحبا world 123", false);
    let arabic = span(&s.row, 0, 4);
    let world = span(&s.row, 6, 10);
    let num = span(&s.row, 12, 14);
    assert!(world.1 <= arabic.0 + 0.5, "world {world:?} vs {arabic:?}");
    assert!(
        num.0 >= world.1 - 0.5,
        "\"world 123\" must read left to right: {world:?} {num:?}"
    );
    assert!(s.row.segments[0].seg_x == 0.0);

    let s = shaped("مرحبا world", true);
    assert!(span(&s.row, 6, 10).0 >= span(&s.row, 0, 4).1 - 0.5);
}
