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

    // Must be split into distinct scoped segments so code and hashes are cell-locked
    assert!(
        cached.segments.len() >= 4,
        "Must isolate code tokens and Arabic runs into scoped segments"
    );

    // For each segment, seg_x MUST strictly equal padding_left + seg_start * char_width
    for seg in &cached.segments {
        let expected_x = padding_left + seg.seg_start as f32 * char_width;
        assert!(
            (seg.seg_x - expected_x).abs() < f32::EPSILON,
            "Segment must be cell-locked at its grid start position"
        );
    }

    // Verify that commit hash '43f9bb8' is contained in its own LTR segment without Arabic tearing
    let hash_seg = cached.segments.iter().find(|s| {
        let slice: String = lines[0].cells[s.seg_start..s.seg_end]
            .iter()
            .map(|c| c.c)
            .collect();
        slice.contains("43f9bb8")
    });
    assert!(hash_seg.is_some(), "Must have a segment containing 43f9bb8");
    let hash_seg = hash_seg.unwrap();
    assert!(
        !hash_seg.has_rtl,
        "43f9bb8 must be shaped as an LTR segment"
    );
}
