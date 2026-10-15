use crate::color::{parse_hex, Palette, Rgba};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
    /// Blink the cursor, unless the running program asks for a steady one (DECSCUSR).
    pub cursor_blink: bool,
    /// Keys this version does not know about are kept so saving never drops them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
    /// The file exists but could not be parsed: never overwrite it with defaults.
    #[serde(skip)]
    keep_file: bool,
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
            cursor_blink: true,
            extra: serde_json::Map::new(),
            keep_file: false,
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

static WARNINGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Report a configuration problem: printed to stderr and kept for `take_warnings`, so the
/// app can also show it in the terminal (a Windows GUI app has no visible stderr).
pub fn warn(message: String) {
    eprintln!("{message}");
    if let Ok(mut warnings) = WARNINGS.lock() {
        warnings.push(message);
    }
}

/// Problems reported since the last call, oldest first.
pub fn take_warnings() -> Vec<String> {
    WARNINGS
        .lock()
        .map(|mut w| std::mem::take(&mut *w))
        .unwrap_or_default()
}

/// Parses a color value from the config, warning on stderr and returning `None` if invalid.
fn parse_config_color(value: &str, key: &str) -> Option<Rgba> {
    let color = parse_hex(value);
    if color.is_none() {
        warn(format!(
            "twitty: ignoring invalid color {value:?} for {key}"
        ));
    }
    color
}

impl TwittyConfig {
    /// The palette for the configured `theme`, with any `colors` overrides applied.
    pub fn palette(&self) -> Palette {
        let mut palette = match self.theme.as_deref() {
            None => Palette::default(),
            Some(name) => Palette::named(name).unwrap_or_else(|| {
                warn(format!("twitty: unknown theme {name:?}, using tokyo-night"));
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
                warn("twitty: ignoring colors.ansi entries after the first 16".into());
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
        Self::config_path().map_or_else(Self::default, |path| Self::load_from(&path))
    }

    /// Read `path`; a missing file gives the defaults. A file that fails to parse also gives
    /// the defaults, with a warning, and is left untouched by later saves.
    pub fn load_from(path: &Path) -> Self {
        let Ok(content) = fs::read_to_string(path) else {
            return Self::default();
        };
        let mut config = match serde_json::from_str::<Self>(&content) {
            Ok(config) => config,
            Err(err) => {
                warn(format!(
                    "twitty: ignoring {} ({err}); it will not be overwritten",
                    path.display()
                ));
                Self {
                    keep_file: true,
                    ..Self::default()
                }
            }
        };
        config.sanitize();
        config
    }

    /// Bring hand-edited values into the ranges the UI allows.
    fn sanitize(&mut self) {
        let defaults = Self::default();
        self.font_size = if self.font_size.is_finite() {
            self.font_size.clamp(8.0, 48.0)
        } else {
            defaults.font_size
        };
        self.background_opacity = if self.background_opacity.is_finite() {
            self.background_opacity.clamp(0.2, 1.0)
        } else {
            defaults.background_opacity
        };
    }

    pub fn save(&self) {
        if let Some(path) = Self::config_path() {
            if let Err(err) = self.save_to(&path) {
                warn(format!("twitty: could not save {}: {err}", path.display()));
            }
        }
    }

    /// Write the config to `path` through a temporary file, so a crash or a full disk never
    /// leaves a truncated config behind. Does nothing when the existing file failed to parse.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if self.keep_file {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)
    }
}
