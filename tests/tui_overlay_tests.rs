//! TUI popups drawn over Arabic text: the popup is its own layout region, so leftover Arabic
//! cells beside it must not reorder the popup's text (opencode command palette, dialogs).
use cosmic_text::FontSystem;
use twitty::color::Palette;
use twitty::render::{CachedSegment, RowShaper};
use twitty::terminal::Terminal;

const CW: f32 = 9.0;

fn segments(bytes: &[u8]) -> Vec<CachedSegment> {
    let mut term = Terminal::new(60, 3, |_| {});
    term.process_bytes(bytes);
    term.process_bytes(b"\r\n"); // finished line, cursor elsewhere
    let (lines, _) = term.snapshot();
    let mut fs = FontSystem::new();
    let palette = Palette::default();
    let mut shaper = RowShaper {
        font_system: &mut fs,
        palette: &palette,
        font_px: 15.0,
        char_width: CW,
        line_height: 23.0,
        family: None,
    };
    shaper.build_row(&lines[0], None)
}

/// Visual left x of the glyph for column `col`.
fn glyph_x(segs: &[CachedSegment], col: usize) -> f32 {
    let seg = segs
        .iter()
        .find(|s| col >= s.seg_start && col < s.seg_end)
        .expect("segment for column");
    let byte = seg.cell_bytes[col - seg.seg_start];
    seg.buffer
        .layout_runs()
        .find_map(|run| {
            run.glyphs
                .iter()
                .find(|g| byte >= g.start && byte < g.end)
                .map(|g| seg.seg_x + g.x)
        })
        .expect("glyph")
}

#[test]
fn popup_text_next_to_leftover_arabic_stays_in_place() {
    // Cols 0-1: "│ ", cols 2-3: Arabic left from the prompt underneath (its own background),
    // then the popup: 4 padding spaces and "Connect provider" from col 8, on the popup color.
    let segs = segments(
        "│ \x1b[48;5;234mخل\x1b[48;5;237m    Connect provider          \x1b[0m".as_bytes(),
    );
    let x = glyph_x(&segs, 8);
    assert!(
        (x - 8.0 * CW).abs() < 1.0,
        "\"Connect\" drawn at {x}, expected {}",
        8.0 * CW
    );
}

#[test]
fn popup_ending_beside_arabic_keeps_its_text_in_place() {
    // Popup text first, then the popup's right padding, then Arabic from underneath.
    let segs = segments("\x1b[48;5;237m  Stash prompt   \x1b[48;5;234mالحاجة دي\x1b[0m".as_bytes());
    let x = glyph_x(&segs, 2);
    assert!(
        (x - 2.0 * CW).abs() < 1.0,
        "\"Stash\" drawn at {x}, expected {}",
        2.0 * CW
    );
}

#[test]
fn highlighted_word_inside_arabic_keeps_sentence_order() {
    // A word highlighted inside an Arabic sentence is not a separate region: the sentence must
    // still read right to left, so the first word stays rightmost.
    let segs = segments("مرحبا \x1b[43mبك\x1b[0m يا صديقي".as_bytes());
    assert!(
        glyph_x(&segs, 0) > glyph_x(&segs, 6),
        "first word must be right of the highlighted one"
    );
}
