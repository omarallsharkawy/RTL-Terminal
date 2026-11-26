use crate::color::{parse_hex, Palette, Rgba};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

/// Maximum `config.json` size read from disk (2 MiB).
pub const MAX_CONFIG_BYTES: usize = 2 * 1024 * 1024;

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
    /// Proportional chrome font. Unset uses the bundled Latin and Arabic UI fonts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_font_family: Option<String>,
    /// Chrome accent as `#rgb` or `#rrggbb`. Unset uses the theme cursor color.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_accent: Option<String>,
    pub top_bar: bool,
    pub pane_headers: bool,
    /// Skip chrome animations and show a steady working indicator.
    pub reduce_motion: bool,
    /// Render OSC 133 command blocks on the main screen as rounded cards with a hover toolbar.
    pub block_cards: bool,
    /// Blink the cursor, unless the running program asks for a steady one (DECSCUSR).
    pub cursor_blink: bool,
    /// Where the workspace sidebar sits: "right", "left" or "hidden".
    #[serde(deserialize_with = "deserialize_sidebar")]
    pub sidebar: String,
    /// Sidebar width in logical pixels.
    pub sidebar_width: f32,
    /// Show a desktop notification when a program in an unfocused pane asks for attention
    /// (OSC 9/99/777).
    pub desktop_notifications: bool,
    /// Listen on a local socket so `twitty ctl` and agents can drive this window.
    pub control_socket: bool,
    /// Expose cmux-compatible environment variables (`CMUX_SOCKET_PATH`, `CMUX_WORKSPACE_ID`,
    /// `CMUX_SURFACE_ID`) when the control socket is enabled. Defaults to true.
    pub cmux_compat: bool,
    /// Prepend Twitty's `claude` wrapper to `PATH` inside Twitty shells so Claude Code reports
    /// working / needs-input / done status and notifications over the control socket. Defaults
    /// to true.
    pub claude_hooks: bool,
    /// Command composer mode: "off", "on-demand" (default) or "always".
    #[serde(deserialize_with = "deserialize_composer")]
    pub composer: String,
    /// Load Twitty's shell integration (OSC 133 prompt marks, OSC 7 cwd) into bash, zsh and fish.
    pub shell_integration: bool,
    /// Restore the tabs and splits from the last window closed normally. Defaults to true.
    pub restore_session: bool,
    /// Command run by "Ask an agent", e.g. "claude" or "codex". Unset: the first one installed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_command: Option<String>,
    /// Master switch for built-in AI features (composer command generation, failed-block explain/fix).
    pub ai: bool,
    /// Coding agent CLI override for built-in AI (e.g. "claude", "codex", "gemini", "opencode").
    /// Unset: falls back to `agent_command`, then auto-detects on `PATH`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_agent: Option<String>,
    /// Timeout in seconds for non-interactive built-in AI requests (default 60, clamped 5..=300).
    pub ai_timeout_secs: u64,
    /// Directories scanned (1-2 levels deep) for git repositories in the project switcher.
    #[serde(default = "crate::projects::scan::default_project_roots")]
    pub project_roots: Vec<String>,
    /// Custom commands ("workflows") listed in the command palette (always trusted).
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "crate::workflows::deserialize_config_commands"
    )]
    pub commands: Vec<crate::workflows::WorkflowSpec>,
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
            ui_font_family: None,
            ui_accent: None,
            top_bar: true,
            pane_headers: true,
            reduce_motion: false,
            block_cards: true,
            cursor_blink: true,
            sidebar: "right".into(),
            sidebar_width: crate::layout::DEFAULT_SIDEBAR_WIDTH,
            desktop_notifications: true,
            control_socket: true,
            cmux_compat: true,
            claude_hooks: true,
            composer: crate::composer::ComposerMode::OnDemand.as_str().into(),
            shell_integration: true,
            restore_session: true,
            agent_command: None,
            ai: true,
            ai_agent: None,
            ai_timeout_secs: 60,
            project_roots: crate::projects::scan::default_project_roots(),
            commands: Vec::new(),
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
/// Control and bidi-override characters are stripped so paths/errors cannot inject escapes.
pub(crate) fn warn(message: String) {
    let clean = crate::shell_integration::clean_text(&message, 512);
    if clean.is_empty() {
        return;
    }
    tracing::warn!("{clean}");
    if let Ok(mut warnings) = WARNINGS.lock() {
        warnings.push(clean);
    }
}

/// Problems reported since the last call, oldest first.
pub fn take_warnings() -> Vec<String> {
    WARNINGS
        .lock()
        .map(|mut w| std::mem::take(&mut *w))
        .unwrap_or_default()
}

fn deserialize_sidebar<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    if let serde_json::Value::String(value) = value {
        return Ok(value);
    }
    warn("twitty: invalid sidebar value, using right".into());
    Ok(crate::layout::SidebarPosition::Right.as_str().to_owned())
}

fn deserialize_composer<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    if let serde_json::Value::String(value) = value {
        return Ok(value);
    }
    warn("twitty: invalid composer value, using on-demand".into());
    Ok(crate::composer::ComposerMode::OnDemand.as_str().to_owned())
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
    /// Parsed command composer policy (`off`, `on-demand`, `always`).
    pub fn composer_mode(&self) -> crate::composer::ComposerMode {
        self.composer.parse().unwrap_or_default()
    }

    pub(crate) fn ui_accent_color(&self) -> Option<Rgba> {
        self.ui_accent
            .as_deref()
            .and_then(|value| parse_config_color(value, "ui_accent"))
    }

    /// The palette for the configured `theme`, with any `colors` overrides applied.
    pub fn palette(&self) -> Palette {
        let mut palette = match self.theme.as_deref() {
            None => Palette::default(),
            Some(name) => Palette::named(name).unwrap_or_else(|| {
                warn(format!("twitty: unknown theme {name:?}, using twitty-dark"));
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
    /// (default `~/.config`) elsewhere. Relative environment paths are ignored.
    pub fn config_path() -> Option<PathBuf> {
        let absolute_dir = |key: &str| {
            std::env::var_os(key)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        };
        let mut path = if cfg!(windows) {
            absolute_dir("APPDATA")?
        } else if let Some(xdg) = absolute_dir("XDG_CONFIG_HOME") {
            xdg
        } else {
            absolute_dir("HOME")?.join(".config")
        };
        path.push("twitty");
        path.push("config.json");
        Some(path)
    }

    pub(crate) fn load() -> Self {
        Self::config_path().map_or_else(Self::default, |path| Self::load_from(&path))
    }

    /// Read `path`; a missing file gives the defaults. A file that fails to parse, is not a
    /// regular file, or exceeds [`MAX_CONFIG_BYTES`] also gives the defaults, with a warning,
    /// and is left untouched by later saves.
    pub fn load_from(path: &Path) -> Self {
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // Avoid blocking startup if a FIFO is placed at the config path; symlinks are
            // still followed so dotfiles links work.
            options.custom_flags(libc::O_NONBLOCK);
        }
        let file = match options.open(path) {
            Ok(f) => f,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                warn(format!(
                    "twitty: could not open {} ({err}); it will not be overwritten",
                    path.display()
                ));
                return Self {
                    keep_file: true,
                    ..Self::default()
                };
            }
        };
        let Ok(meta) = file.metadata() else {
            return Self {
                keep_file: true,
                ..Self::default()
            };
        };
        if !meta.file_type().is_file() {
            warn(format!(
                "twitty: ignoring {} (not a regular file); it will not be overwritten",
                path.display()
            ));
            return Self {
                keep_file: true,
                ..Self::default()
            };
        }
        let mut bytes = Vec::new();
        if let Err(err) = file
            .take(MAX_CONFIG_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
        {
            warn(format!(
                "twitty: could not read {} ({err}); it will not be overwritten",
                path.display()
            ));
            return Self {
                keep_file: true,
                ..Self::default()
            };
        }
        if bytes.len() > MAX_CONFIG_BYTES {
            warn(format!(
                "twitty: ignoring {} (exceeds {MAX_CONFIG_BYTES} bytes); it will not be overwritten",
                path.display()
            ));
            return Self {
                keep_file: true,
                ..Self::default()
            };
        }
        let content = match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(err) => {
                warn(format!(
                    "twitty: ignoring {} (not valid UTF-8: {err}); it will not be overwritten",
                    path.display()
                ));
                return Self {
                    keep_file: true,
                    ..Self::default()
                };
            }
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
        self.sidebar_width = crate::layout::clamp_sidebar_width(self.sidebar_width);
        if self
            .sidebar
            .parse::<crate::layout::SidebarPosition>()
            .is_err()
        {
            warn(format!(
                "twitty: invalid sidebar {:?}, using right",
                self.sidebar
            ));
            self.sidebar = defaults.sidebar;
        }
        if self
            .composer
            .parse::<crate::composer::ComposerMode>()
            .is_err()
        {
            warn(format!(
                "twitty: invalid composer {:?}, using on-demand",
                self.composer
            ));
            self.composer = defaults.composer;
        }
        self.ai_timeout_secs = if self.ai_timeout_secs == 0 {
            defaults.ai_timeout_secs
        } else {
            self.ai_timeout_secs.clamp(5, 300)
        };
    }

    pub(crate) fn save(&self) {
        if let Some(path) = Self::config_path() {
            if let Err(err) = self.save_to(&path) {
                warn(format!("twitty: could not save {}: {err}", path.display()));
            }
        }
    }

    /// Write the config to `path` through a unique temporary file, so a crash, concurrent
    /// window close, or full disk never leaves a truncated config behind. Does nothing when
    /// the existing file failed to parse. A symlinked config (dotfiles) is written through
    /// the link, which stays a link.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if self.keep_file {
            return Ok(());
        }
        let resolved = fs::canonicalize(path).ok();
        let path = resolved.as_deref().unwrap_or(path);
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let (tmp, mut file) = create_temp_config(parent)?;
        let result = (|| -> io::Result<()> {
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&tmp, path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }
}

fn create_temp_config(dir: &Path) -> io::Result<(PathBuf, fs::File)> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    for _ in 0..64 {
        let seq = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".config-{pid}-{seq}.json.tmp"));
        match options.open(&tmp) {
            Ok(file) => return Ok((tmp, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::other("could not create temporary config file"))
}
