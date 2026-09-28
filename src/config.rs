use std::fs;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TwittyConfig {
    pub font_size: f32,
    pub background_opacity: f32,
}

impl Default for TwittyConfig {
    fn default() -> Self {
        Self {
            font_size: 14.5,
            background_opacity: 0.92,
        }
    }
}

impl TwittyConfig {
    fn config_path() -> Option<PathBuf> {
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
