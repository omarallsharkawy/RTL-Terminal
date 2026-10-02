use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TwittyConfig {
    pub font_size: f32,
    pub background_opacity: f32,
    pub cursor_style: String,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub scrollback_lines: Option<usize>,
}

impl Default for TwittyConfig {
    fn default() -> Self {
        Self {
            font_size: 14.5,
            background_opacity: 0.92,
            cursor_style: "beam".to_string(),
            font_family: None,
            scrollback_lines: None,
        }
    }
}

impl TwittyConfig {
    pub fn config_path() -> Option<PathBuf> {
        #[cfg(target_os = "windows")]
        {
            if let Ok(appdata) = std::env::var("APPDATA") {
                let mut path = PathBuf::from(appdata);
                path.push("twitty");
                path.push("config.json");
                return Some(path);
            }
            if let Ok(userprofile) = std::env::var("USERPROFILE") {
                let mut path = PathBuf::from(userprofile);
                path.push("AppData");
                path.push("Roaming");
                path.push("twitty");
                path.push("config.json");
                return Some(path);
            }
        }

        if let Ok(config_home) = std::env::var("XDG_CONFIG_HOME") {
            let mut path = PathBuf::from(config_home);
            path.push("twitty");
            path.push("config.json");
            return Some(path);
        }

        let home = std::env::var("HOME").ok()?;
        let mut path = PathBuf::from(home);
        path.push(".config");
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
