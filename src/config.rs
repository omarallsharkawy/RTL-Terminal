use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TwittyConfig {
    pub font_size: f32,
    pub background_opacity: f32,
    pub cursor_style: String,
    pub font_family: Option<String>,
    pub scrollback_lines: Option<usize>,
    pub osc52: Option<String>,
}

impl Default for TwittyConfig {
    fn default() -> Self {
        Self {
            font_size: 14.5,
            background_opacity: 0.78,
            cursor_style: "beam".to_string(),
            font_family: None,
            scrollback_lines: None,
            osc52: None,
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
                match serde_json::from_str::<Self>(&content) {
                    Ok(config) => return config,
                    Err(e) => eprintln!("Warning: Failed to parse config at {:?}: {}", path, e),
                }
            }
        }
        Self::default()
    }

    pub fn update_font_size(new_size: f32) {
        if let Some(path) = Self::config_path() {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let mut val: serde_json::Value = if let Ok(content) = fs::read_to_string(&path) {
                serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
            } else {
                serde_json::json!({})
            };
            if let Some(obj) = val.as_object_mut() {
                obj.insert("font_size".to_string(), serde_json::json!(new_size));
            }
            if let Ok(json) = serde_json::to_string_pretty(&val) {
                let _ = fs::write(&path, json);
            }
        }
    }

    #[allow(dead_code)]
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
