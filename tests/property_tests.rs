//! Property tests: random mixes of Arabic (with harakat), Latin words, numbers, punctuation,
//! symbols, CJK and emoji must keep the bidi layout invariants, and arbitrary bytes from a
//! program must never crash the terminal or the shaper.
use alacritty_terminal::index::Side;
use cosmic_text::FontSystem;
use proptest::prelude::*;
use twitty::color::Palette;
use twitty::render::{build_row_quads, visual_cell_extents, CachedRow, RowShaper};
use twitty::terminal::Terminal;

const CW: f32 = 9.0;
const LH: f32 = 23.0;
const COLS: usize = 48;

fn token() -> impl Strategy<Value = &'static str> {
    prop::sample::select(vec![
        "مرحبا",
        "بالعالم",
        "قال",
        "الحاجة",
        "دي",
        "مُحَمَّد",
        "كِتَاب",
        "تم",
        "لا",
        "؟",
        "،",
        "hello",
        "world",
        "7abaf40",
        "v3.0.0",
        "2026",
        "123",
        "ok",
        "(x)",
        "[y]",
        "✓",
        "!",
        ".",
        ":",
        "-",
        "/",
        "#15161e",
        "中文",
        "😀",
        " ",
        "  ",
    ])
}

fn sentence() -> impl Strategy<Value = String> {
    prop::collection::vec(token(), 1..10).prop_map(|t| {
        let s = t.join(" ");
        // Keep it on one row of the test terminal (wide chars take two cells).
        s.chars().take(COLS / 2).collect()
    })
}

struct Shaped {
    term: Terminal,
    row: CachedRow,
    line: twitty::terminal::LineData,
}

fn shape(term: Terminal, fs: &mut FontSystem, palette: &Palette) -> Shaped {
    let (lines, _) = term.snapshot();
    let mut shaper = RowShaper {
        font_system: fs,
        palette,
        font_px: 15.0,
        char_width: CW,
        line_height: LH,
        family: None,
    };
    let segments = shaper.build_row(&lines[0], None);
    let row = CachedRow {
        visual: visual_cell_extents(&lines[0], &segments, CW),
        segments,
        ..Default::default()
    };
    Shaped {
        term,
        row,
        line: lines[0].clone(),
    }
}

fn finished(text: &str) -> Terminal {
    let mut term = Terminal::new(COLS, 3, |_| {});
    term.process_bytes(text.as_bytes());
    term.process_bytes(b"\r\n");
    term
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    #[test]
    fn selected_row_backgrounds_have_no_gaps_or_overlaps(text in sentence()) {
        let mut fs = FontSystem::new();
        let palette = Palette::default();
        let mut term = finished(&text);
        let used = text.chars().map(|c| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0)).sum::<usize>();
        prop_assume!(used > 0);
        term.start_selection(0, 0, Side::Left, 1);
        term.update_selection(used.min(COLS) - 1, 0, Side::Right);
        let s = shape(term, &mut fs, &palette);
        let sel = palette.selection.to_array();
        let mut q: Vec<_> = build_row_quads(&palette, false, (CW, LH, 15.0), &s.line, &s.row.segments)
            .into_iter()
            .filter(|q| q.4 == sel && q.2 > 0.0)
            .collect();
        q.sort_by(|a, b| a.0.total_cmp(&b.0));
        for pair in q.windows(2) {
            let gap = pair[1].0 - (pair[0].0 + pair[0].2);
            prop_assert!(gap.abs() < 0.01, "gap {} in {:?}", gap, text);
        }
    }

    #[test]
    fn every_cell_is_hit_at_the_center_of_where_it_is_drawn(text in sentence()) {
        let mut fs = FontSystem::new();
        let palette = Palette::default();
        let s = shape(finished(&text), &mut fs, &palette);
        for (col, extent) in s.row.visual.iter().enumerate() {
            let Some((l, r)) = *extent else { continue };
            if r - l < 1.0 || s.line.cells[col].is_spacer() || s.line.cells[col].is_blank() {
                continue;
            }
            let (hit, _) = s.row.hit_test((l + r) / 2.0, CW, COLS);
            prop_assert_eq!(hit, col, "{:?}: cell {} drawn at {}..{}", text, col, l, r);
        }
    }

    #[test]
    fn dragging_between_two_cells_selects_contiguous_text(text in sentence(), a in 0usize..COLS, b in 0usize..COLS) {
        let mut fs = FontSystem::new();
        let palette = Palette::default();
        let s = shape(finished(&text), &mut fs, &palette);
        let x = |c: usize| s.row.visual.get(c).copied().flatten().map_or(c as f32 * CW + CW / 2.0, |(l, r)| (l + r) / 2.0);
        let (c0, s0) = s.row.hit_test(x(a), CW, COLS);
        let (c1, s1) = s.row.hit_test(x(b), CW, COLS);
        let mut term = s.term;
        term.start_selection(c0, 0, s0, 1);
        term.update_selection(c1, 0, s1);
        if let Some(sel) = term.selection_text() {
            let row: String = text.clone();
            prop_assert!(row.contains(sel.trim_end()), "{:?} is not a piece of {:?}", sel, row);
        }
    }

    #[test]
    fn arbitrary_program_output_never_crashes(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        let mut fs = FontSystem::new();
        let palette = Palette::default();
        let mut term = Terminal::new(30, 6, |_| {});
        term.process_bytes(&bytes);
        term.process_bytes("مرحبا 7abaf40 ✓".as_bytes());
        let (lines, _) = term.snapshot();
        let mut shaper = RowShaper { font_system: &mut fs, palette: &palette, font_px: 15.0, char_width: CW, line_height: LH, family: None };
        for line in &lines {
            let segments = shaper.build_row(line, None);
            let _ = build_row_quads(&palette, false, (CW, LH, 15.0), line, &segments);
        }
    }
}
