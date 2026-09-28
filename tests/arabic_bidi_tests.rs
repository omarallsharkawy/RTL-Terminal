use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};
use twitty::terminal::Terminal;

#[test]
fn test_terminal_rtl_detection() {
    let mut term = Terminal::new(80, 24);
    
    term.process_bytes(b"hello world\n");
    let (lines, _) = term.snapshot();
    assert!(!lines[0].has_rtl, "English text should not be detected as RTL");

    let arabic_text = "مرحبا بالعالم\n";
    term.process_bytes(arabic_text.as_bytes());
    let (lines, _) = term.snapshot();
    assert!(lines[1].has_rtl, "Arabic text must be detected as RTL");
}

#[test]
fn test_cosmic_arabic_shaping_and_bidi() {
    let mut font_system = FontSystem::new();
    let metrics = Metrics::new(16.0, 22.0);
    let mut buffer = Buffer::new_empty(metrics);

    let text = "$ echo \"مرحبا\"";
    buffer.set_text(
        text,
        &Attrs::new().family(Family::Monospace),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut font_system, false);

    let runs: Vec<_> = buffer.layout_runs().collect();
    assert_eq!(runs.len(), 1, "Should have 1 layout run");

    let run = &runs[0];
    let glyphs = &run.glyphs;

    let arabic_glyphs: Vec<_> = glyphs.iter().filter(|g| g.level.is_rtl()).collect();
    assert!(!arabic_glyphs.is_empty(), "Should contain RTL glyphs for Arabic");

    let min_x_glyph = arabic_glyphs.iter().min_by(|a, b| a.x.partial_cmp(&b.x).unwrap()).unwrap();
    let max_x_glyph = arabic_glyphs.iter().max_by(|a, b| a.x.partial_cmp(&b.x).unwrap()).unwrap();

    assert!(
        min_x_glyph.start > max_x_glyph.start,
        "Arabic text must be visually reordered RTL: left-most glyph (min x) should be later in string than right-most glyph (max x)"
    );
}
