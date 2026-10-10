use crate::color::{parse_hex, Palette, Rgba};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct TwittyConfig {
    pub font_size: f32,
    /// Window background opacity, 0.2 (very see-through) to 1.0 (solid).
    pub background_opacity: f32,
    /// "block", "beam"/"bar" or "underline": the cursor shape when the app does not ask for one.
    pub cursor_style: String,
    /// Built-in color theme, e.g. "tokyo-night" or "dracula". See `color::theme_names`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Per-field color overrides, applied on top of `theme`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colors: Option<ThemeColors>,
    /// Font family for the terminal grid, e.g. "JetBrains Mono". Unset or not installed: monospace.
    pub font_family: Option<String>,
    /// Keys this version does not know about are kept so saving never drops them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for TwittyConfig {
    fn default() -> Self {
        Self {
            font_size: 14.5,
            background_opacity: 0.78,
            cursor_style: "block".into(),
            theme: None,
            colors: None,
            font_family: None,
            extra: serde_json::Map::new(),
        }
    }
}

/// Color overrides. Every field is optional; values are `#rgb` or `#rrggbb`.
/// `ansi` may list fewer than 16 colors, which overrides only those indices.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ThemeColors {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreground: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ansi: Option<Vec<String>>,
}

/// Parses a color value from the config, warning on stderr and returning `None` if invalid.
fn parse_config_color(value: &str, key: &str) -> Option<Rgba> {
    let color = parse_hex(value);
    if color.is_none() {
        eprintln!("twitty: ignoring invalid color {value:?} for {key}");
    }
    color
}

impl TwittyConfig {
    /// The palette for the configured `theme`, with any `colors` overrides applied.
    pub fn palette(&self) -> Palette {
        let mut palette = match self.theme.as_deref() {
            None => Palette::default(),
            Some(name) => Palette::named(name).unwrap_or_else(|| {
                eprintln!("twitty: unknown theme {name:?}, using tokyo-night");
                Palette::default()
            }),
        };
        let Some(colors) = &self.colors else {
            return palette;
        };
        let fields = [
            (&colors.background, &mut palette.background, "background"),
            (&colors.foreground, &mut palette.foreground, "foreground"),
            (&colors.cursor, &mut palette.cursor, "cursor"),
            (&colors.selection, &mut palette.selection, "selection"),
        ];
        for (value, slot, key) in fields {
            if let Some(color) = value.as_deref().and_then(|v| parse_config_color(v, key)) {
                *slot = color;
            }
        }
        if let Some(ansi) = &colors.ansi {
            if ansi.len() > 16 {
                eprintln!("twitty: ignoring colors.ansi entries after the first 16");
            }
            for (i, value) in ansi.iter().take(16).enumerate() {
                let key = format!("colors.ansi[{i}]");
                if let Some(color) = parse_config_color(value, &key) {
                    palette.ansi[i] = color;
                }
            }
        }
        palette
    }

    /// `%APPDATA%\twitty\config.json` on Windows, `$XDG_CONFIG_HOME/twitty/config.json`
    /// (default `~/.config`) elsewhere.
    fn config_path() -> Option<PathBuf> {
        let non_empty = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty());
        let mut path = if cfg!(windows) {
            PathBuf::from(non_empty("APPDATA")?)
        } else if let Some(xdg) = non_empty("XDG_CONFIG_HOME") {
            PathBuf::from(xdg)
        } else {
            PathBuf::from(non_empty("HOME")?).join(".config")
        };
        path.push("twitty");
        path.push("config.json");
        Some(path)
    }

    pub fn load() -> Self {
        if let Some(path) = Self::config_path() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str::<Self>(&content) {
                    return config;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        if let Some(path) = Self::config_path() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Ok(json) = serde_json::to_string_pretty(self) {
                let _ = fs::write(&path, json);
            }
        }
    }
}
