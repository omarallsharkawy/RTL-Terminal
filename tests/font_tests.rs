use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping};
use twitty::config::TwittyConfig;
use twitty::render::measure_char_width;
use twitty::render::text::fonts::{grid_family, new_font_system};

#[test]
fn embedded_arabic_font_is_registered() {
    let fs = new_font_system();
    assert!(fs.db().faces().any(|f| f
        .families
        .iter()
        .any(|(n, _)| n.contains("Noto Naskh Arabic"))));
}

#[test]
fn arabic_shapes_without_notdef() {
    let mut fs = new_font_system();
    let mut buf = Buffer::new_empty(Metrics::new(16.0, 24.0));
    buf.set_text(
        "مرحبا",
        &Attrs::new().family(Family::Monospace),
        Shaping::Advanced,
        None,
    );
    buf.shape_until_scroll(&mut fs, false);
    let mut count = 0;
    for run in buf.layout_runs() {
        for g in run.glyphs {
            assert!(fs.db().face(g.font_id).is_some(), "glyph font is in the db");
            assert_ne!(g.glyph_id, 0, "glyph is .notdef");
            count += 1;
        }
    }
    assert!(count >= 5, "shaped {count} glyphs");
}

#[test]
fn font_family_deserializes_and_unknown_keys_survive() {
    let cfg: TwittyConfig =
        serde_json::from_str(r#"{"font_family":"JetBrains Mono","future_key":42}"#).unwrap();
    assert_eq!(cfg.font_family.as_deref(), Some("JetBrains Mono"));
    let out = serde_json::to_string(&cfg).unwrap();
    assert!(out.contains("future_key"), "{out}");
    assert!(out.contains("JetBrains Mono"), "{out}");
    assert!(TwittyConfig::default().font_family.is_none());
}

#[test]
fn unknown_family_falls_back_without_panic() {
    let mut fs = new_font_system();
    assert!(matches!(grid_family(None), Family::Monospace));
    let w = measure_char_width(&mut fs, Some("No Such Font 123"), 16.0, 24.0);
    assert!(w > 0.0);
}
