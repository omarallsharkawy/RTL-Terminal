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
    /// Blink the cursor, unless the running program asks for a steady one (DECSCUSR).
    pub cursor_blink: bool,
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
            cursor_blink: true,
            extra: serde_json::Map::new(),
        }
    }
}

impl TwittyConfig {
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
