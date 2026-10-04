use twitty::config::TwittyConfig;

#[test]
fn test_config_defaults() {
    let config = TwittyConfig::default();
    assert_eq!(config.font_size, 14.5);
    assert_eq!(config.background_opacity, 0.78);
    assert_eq!(config.cursor_style, "beam");
    assert_eq!(config.font_family, None);
    assert_eq!(config.scrollback_lines, None);
}

#[test]
fn test_config_json_roundtrip() {
    let custom = TwittyConfig {
        font_size: 16.0,
        background_opacity: 0.85,
        cursor_style: "block".to_string(),
        font_family: Some("JetBrainsMono Nerd Font".to_string()),
        scrollback_lines: Some(25000),
    };

    let json = serde_json::to_string(&custom).expect("Serialization failed");
    assert!(json.contains("\"font_size\":16.0") || json.contains("\"font_size\": 16.0"));
    assert!(
        json.contains("\"cursor_style\":\"block\"") || json.contains("\"cursor_style\": \"block\"")
    );

    let deserialized: TwittyConfig = serde_json::from_str(&json).expect("Deserialization failed");
    assert_eq!(deserialized.font_size, 16.0);
    assert_eq!(deserialized.background_opacity, 0.85);
    assert_eq!(deserialized.cursor_style, "block");
    assert_eq!(
        deserialized.font_family,
        Some("JetBrainsMono Nerd Font".to_string())
    );
    assert_eq!(deserialized.scrollback_lines, Some(25000));
}

#[test]
fn test_config_path_resolution() {
    // Verify config_path does not panic
    let path = TwittyConfig::config_path();
    if let Some(p) = path {
        assert!(p.ends_with("config.json"));
        assert!(p.to_string_lossy().contains("twitty"));
    }
}
