use cosmic_text::{Attrs, Family, FontSystem, Metrics};
use twitty::color::{Palette, Rgba};
use twitty::shaping::shape_row;
use twitty::terminal::Terminal;

#[test]
fn test_cell_locked_segments_for_mixed_code_and_arabic() {
    let mut term = Terminal::new(140, 24, |_| {});
    let text = "الكوميتين اتحسنوا جداً — 7abaf40 نضيف تماماً. بس 43f9bb8 وهو بيصلّح Ctrl+D/E/K\r\n";
    term.process_bytes(text.as_bytes());
    let (lines, cursor) = term.snapshot();

    let mut font_system = FontSystem::new();
    let metrics = Metrics::new(14.5, 23.0);
    let default_attrs = Attrs::new().family(Family::Monospace);
    let palette = Palette::default();
    let default_bg = Rgba::from_rgb8(21, 22, 30);
    let char_width = 9.0f32;
    let padding_left = 8.0f32;

    let cached = shape_row(
        &lines[0],
        0,
        &cursor,
        char_width,
        23.0,
        padding_left,
        metrics,
        &default_attrs,
        &palette,
        &mut font_system,
        default_bg,
    );

    // Segments start on their grid column.
    for seg in &cached.segments {
        let expected_x = padding_left + seg.seg_start as f32 * char_width;
        assert!(
            (seg.seg_x - expected_x).abs() < f32::EPSILON,
            "Segment must start at its grid position"
        );
    }

    // Code tokens inside Arabic text stay intact and left-to-right: every character of
    // each hash sits right of the previous one, with no Arabic torn in between.
    let cells = &lines[0].cells;
    let text: String = cells.iter().map(|c| c.c).collect();
    for hash in ["7abaf40", "43f9bb8", "Ctrl+D/E/K"] {
        let byte = text
            .find(hash)
            .unwrap_or_else(|| panic!("{hash} not on the line"));
        let col = text[..byte].chars().count();
        let seg = cached
            .segments
            .iter()
            .find(|s| s.seg_start <= col && col + hash.len() <= s.seg_end)
            .unwrap_or_else(|| panic!("{hash} must not be split across segments"));
        let xs: Vec<f32> = (col..col + hash.len())
            .map(|c| seg.cell_edges[c - seg.seg_start].0)
            .collect();
        for pair in xs.windows(2) {
            let step = pair[1] - pair[0];
            assert!(
                step > 0.0 && step < char_width * 1.5,
                "{hash} must read left-to-right without gaps, got {xs:?}"
            );
        }
    }
}
