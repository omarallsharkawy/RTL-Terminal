use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping};

#[test]
fn test_arabic_font_fallback_resolution() {
    let mut font_system = FontSystem::new();
    let metrics = Metrics::new(14.5, 23.0);
    let mut buffer = Buffer::new_empty(metrics);

    // Primary family: Monospace (Latin only), with fallback chains for Arabic and Symbols
    let text = "Git branch: main ❯ تحديث الكود العربي";
    buffer.set_text(
        text,
        &Attrs::new().family(Family::Monospace),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut font_system, false);

    let runs: Vec<_> = buffer.layout_runs().collect();
    assert!(!runs.is_empty(), "Must produce layout runs");

    let glyphs: Vec<_> = runs[0].glyphs.iter().collect();
    assert!(!glyphs.is_empty(), "Must produce positioned glyphs");

    // Verify that Arabic glyphs have valid non-zero glyph IDs (not tofu/missing glyph id 0)
    let arabic_glyphs: Vec<_> = glyphs.iter().filter(|g| g.level.is_rtl()).collect();
    assert!(
        !arabic_glyphs.is_empty(),
        "Arabic characters must produce RTL glyphs"
    );

    for g in arabic_glyphs {
        assert!(
            g.glyph_id > 0,
            "Glyph ID must be resolved via font fallback, got 0 (tofu) for glyph in Arabic word"
        );
    }
}

#[test]
fn test_symbol_glyph_resolution() {
    let mut font_system = FontSystem::new();
    let metrics = Metrics::new(14.5, 23.0);
    let mut buffer = Buffer::new_empty(metrics);

    // Unicode miscellaneous technical symbols (e.g. ⏵ used in CLI mode badges)
    buffer.set_text(
        "\u{23f5}\u{23f5} auto mode on",
        &Attrs::new().family(Family::Monospace),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(&mut font_system, false);

    let runs: Vec<_> = buffer.layout_runs().collect();
    assert!(!runs.is_empty(), "Must produce layout runs for symbols");
    for glyph in runs[0].glyphs.iter().filter(|g| g.start >= 6) {
        assert!(
            glyph.glyph_id > 0,
            "Latin text glyphs must be resolved, got 0 for glyph at {}",
            glyph.start
        );
    }
}

#[test]
fn test_embedded_arabic_font_loading() {
    let mut font_system = FontSystem::new();
    let count_before = font_system.db().faces().count();
    static EMBEDDED_ARABIC_FONT: &[u8] =
        include_bytes!("../assets/fonts/NotoNaskhArabic-Regular.ttf");
    font_system
        .db_mut()
        .load_font_data(EMBEDDED_ARABIC_FONT.to_vec());
    let count_after = font_system.db().faces().count();
    assert!(
        count_after > count_before,
        "Embedded font data must load successfully into fontdb"
    );
}
