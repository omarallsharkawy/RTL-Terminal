use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};
use twitty::terminal::Terminal;

#[test]
fn test_terminal_rtl_detection() {
    let mut term = Terminal::new(80, 24, |_| {});

    term.process_bytes(b"hello world\n");
    let (lines, _) = term.snapshot();
    assert!(
        !lines[0].has_rtl,
        "English text should not be detected as RTL"
    );

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
    assert!(
        !arabic_glyphs.is_empty(),
        "Should contain RTL glyphs for Arabic"
    );

    let min_x_glyph = arabic_glyphs
        .iter()
        .min_by(|a, b| a.x.partial_cmp(&b.x).unwrap())
        .unwrap();
    let max_x_glyph = arabic_glyphs
        .iter()
        .max_by(|a, b| a.x.partial_cmp(&b.x).unwrap())
        .unwrap();

    assert!(
        min_x_glyph.start > max_x_glyph.start,
        "Arabic text must be visually reordered RTL: left-most glyph (min x) should be later in string than right-most glyph (max x)"
    );
}

#[test]
fn test_prompt_with_arabic_does_not_flip() {
    let mut font_system = FontSystem::new();
    let metrics = Metrics::new(14.5, 23.0);
    let mut buf = Buffer::new_empty(metrics);

    let raw_text = "~ ❯ الزايك";
    let ltr_text = format!("‎{}", raw_text);
    buf.set_text(
        &ltr_text,
        &Attrs::new().family(Family::Monospace),
        Shaping::Advanced,
        None,
    );
    buf.shape_until_scroll(&mut font_system, false);

    let runs: Vec<_> = buf.layout_runs().collect();
    assert_eq!(runs.len(), 1);

    let glyphs = &runs[0].glyphs;
    let tilde = glyphs
        .iter()
        .find(|g| &ltr_text[g.start..g.end] == "~")
        .unwrap();
    let chevron = glyphs
        .iter()
        .find(|g| &ltr_text[g.start..g.end] == "❯")
        .unwrap();
    let arabic = glyphs.iter().find(|g| g.level.is_rtl()).unwrap();

    assert!(tilde.x < chevron.x, "Tilde must be to the left of chevron");
    assert!(
        chevron.x < arabic.x,
        "Prompt chevron must be to the left of Arabic user input, never flipped!"
    );
}

#[test]
fn test_mixed_arabic_with_alphanumeric_hashes() {
    let mut term = Terminal::new(120, 24, |_| {});
    let text = "الكوميتين اتحسنوا جداً — 7abaf40 نضيف تماماً. بس 43f9bb8 وهو بيصلّح Ctrl+D/E/K\r\n";
    term.process_bytes(text.as_bytes());
    let (lines, cursor) = term.snapshot();

    let mut font_system = FontSystem::new();
    let metrics = Metrics::new(14.5, 23.0);
    let default_attrs = Attrs::new().family(Family::Monospace);
    let palette = twitty::color::Palette::default();
    let default_bg = twitty::color::Rgba::from_rgb8(21, 22, 30);

    let cached = twitty::shaping::shape_row(
        &lines[0],
        0,
        &cursor,
        9.0,
        23.0,
        12.0,
        metrics,
        &default_attrs,
        &palette,
        &mut font_system,
        default_bg,
    );

    assert!(!cached.segments.is_empty());
    let seg = &cached.segments[0];
    for run in seg.buffer.layout_runs() {
        let mut glyphs: Vec<_> = run.glyphs.iter().collect();
        glyphs.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
        let g_7 = glyphs
            .iter()
            .find(|g| g.start > 0 && &run.text[g.start..g.end] == "7")
            .unwrap();
        let g_a = glyphs
            .iter()
            .find(|g| g.start > 0 && &run.text[g.start..g.end] == "a")
            .unwrap();
        assert!(
            (g_7.x - g_a.x).abs() <= 15.0,
            "Digit '7' and letter 'a' in hash must stay together visually, diff was {}",
            (g_7.x - g_a.x).abs()
        );
    }
}
