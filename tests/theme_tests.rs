use twitty::color::{theme_names, Palette, Rgba};
use twitty::config::TwittyConfig;

fn config(json: &str) -> TwittyConfig {
    serde_json::from_str(json).expect("valid config json")
}

fn black() -> Rgba {
    Rgba::from_rgb8(0, 0, 0)
}

fn red() -> Rgba {
    Rgba::from_rgb8(255, 0, 0)
}

#[test]
fn every_theme_name_resolves() {
    assert!(!theme_names().is_empty());
    for name in theme_names() {
        assert!(
            Palette::named(name).is_some(),
            "theme {name} does not resolve"
        );
    }
}

#[test]
fn theme_names_are_accepted_in_variant_spellings() {
    assert_eq!(Palette::named("Tokyo Night"), Some(Palette::default()));
    assert_eq!(Palette::named("tokyo_night"), Some(Palette::default()));
    assert_eq!(Palette::named("TOKYO-NIGHT"), Some(Palette::default()));
    assert_eq!(Palette::named("Dracula"), Palette::named("dracula"));
    assert!(Palette::named("no-such-theme").is_none());
}

#[test]
fn default_config_uses_default_palette() {
    assert_eq!(TwittyConfig::default().palette(), Palette::default());
}

#[test]
fn theme_and_color_overrides_apply() {
    let cfg =
        config(r##"{"theme":"dracula","colors":{"background":"#000000","ansi":["#ff0000"]}}"##);
    let palette = cfg.palette();
    let dracula = Palette::named("dracula").unwrap();
    assert_eq!(palette.background, black());
    assert_eq!(palette.ansi[0], red());
    assert_eq!(palette.ansi[1], dracula.ansi[1]);
    assert_eq!(palette.foreground, dracula.foreground);
    assert_eq!(palette.cursor, dracula.cursor);
    assert_eq!(palette.selection, dracula.selection);
}

#[test]
fn short_hex_and_all_fields_apply() {
    let cfg = config(
        r##"{"colors":{"foreground":"#fff","cursor":"#0f0","selection":"#112233",
            "ansi":["#000","#111111","#222222","#333333","#444444","#555555","#666666","#777777",
            "#888888","#999999","#aaaaaa","#bbbbbb","#cccccc","#dddddd","#eeeeee","#ffffff"]}}"##,
    );
    let palette = cfg.palette();
    assert_eq!(palette.foreground, Rgba::from_rgb8(255, 255, 255));
    assert_eq!(palette.cursor, Rgba::from_rgb8(0, 255, 0));
    assert_eq!(palette.selection, Rgba::from_rgb8(0x11, 0x22, 0x33));
    assert_eq!(palette.ansi[1], Rgba::from_rgb8(0x11, 0x11, 0x11));
    assert_eq!(palette.ansi[15], Rgba::from_rgb8(255, 255, 255));
    // Background is not overridden, so it keeps the default theme's value.
    assert_eq!(palette.background, Palette::default().background);
}

#[test]
fn unknown_theme_falls_back_to_default() {
    let cfg = config(r##"{"theme":"no-such-theme"}"##);
    assert_eq!(cfg.palette(), Palette::default());
}

#[test]
fn invalid_hex_is_ignored() {
    let cfg = config(
        r##"{"theme":"nord","colors":{"background":"#zzzzzz","foreground":"#12345","ansi":["red"]}}"##,
    );
    assert_eq!(cfg.palette(), Palette::named("nord").unwrap());
}

#[test]
fn unknown_keys_survive_save() {
    let cfg = config(r##"{"font_size":12.0,"future_key":{"nested":true},"theme":"nord"}"##);
    let saved = serde_json::to_value(&cfg).unwrap();
    assert_eq!(saved["future_key"], serde_json::json!({"nested": true}));
    assert_eq!(saved["theme"], "nord");
    assert!(saved.get("colors").is_none());
}
