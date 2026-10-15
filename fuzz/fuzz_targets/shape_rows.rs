//! Arbitrary program output shaped for drawing: bidi marks, isolates, segment splitting and
//! cell extents must never panic, whatever text and colors end up in a row.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::sync::Mutex;
use twitty::color::Palette;
use twitty::render::{build_row_quads, visual_cell_extents, RowShaper};
use twitty::terminal::Terminal;

static FONTS: Mutex<Option<cosmic_text::FontSystem>> = Mutex::new(None);

fuzz_target!(|data: &[u8]| {
    let mut term = Terminal::new(60, 4, |_| {});
    term.process_bytes(data);
    let (lines, cursor) = term.snapshot();
    let mut fonts = FONTS.lock().unwrap();
    let fs = fonts.get_or_insert_with(twitty::render::text::fonts::new_font_system);
    let palette = Palette::default();
    let mut shaper = RowShaper {
        font_system: fs,
        palette: &palette,
        font_px: 15.0,
        char_width: 9.0,
        line_height: 23.0,
        family: None,
    };
    for (r, line) in lines.iter().enumerate() {
        let rc = (r == cursor.row).then_some(twitty::render::RowCursor { col: cursor.col, block: true });
        let segments = shaper.build_row(line, rc);
        let _ = visual_cell_extents(line, &segments, 9.0);
        let _ = build_row_quads(&palette, false, (9.0, 23.0, 15.0), line, &segments);
    }
});
