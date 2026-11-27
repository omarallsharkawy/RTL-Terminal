//! Window-free state, first-run detection, done-flag persistence, geometry, hit testing,
//! and keyboard/pointer navigation for Twitty's first-run Welcome wizard.
//!
//! Shown once on first launch (when neither `config.json` nor `$XDG_STATE_HOME/twitty/first_run_done`
//! exists yet) and re-openable anytime from the command palette ("Welcome / الترحيب").
//! Drawing lives in `render::scene::welcome`; app hooks live in `app::welcome`.

use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use twitty_core::en;

use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::ai::{agent_display_name, detect_shell_name, resolve_ai_agent, AI_KNOWN_AGENTS};
use crate::commands::{self, Command};
use crate::config::TwittyConfig;
use crate::layout::Rect;
use crate::pty::Shell;
use crate::ui::chrome::{UiLanguage, UiLanguagePref};
use crate::ui::settings::{clamp_font_size, MAX_FONT_SIZE, MIN_FONT_SIZE};
use twitty_style::ui_style::{SPACE_12, SPACE_16, SPACE_24, SPACE_4, SPACE_8};

/// File name of the first-run completion flag inside Twitty's state directory.
pub const FIRST_RUN_FLAG_NAME: &str = "first_run_done";

/// Maximum size accepted when verifying an existing `first_run_done` flag file.
const MAX_FLAG_BYTES: u64 = 4096;

/// Default translucent background opacity used when toggling transparency on from solid (`1.0`).
pub const DEFAULT_TRANSPARENT_OPACITY: f32 = 0.78;

/// Font size increment/decrement step in logical points on Step 2 (Look).
pub const FONT_SIZE_STEP: f32 = 0.5;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

// ---- First-run detection & atomic 0600 flag persistence ----

/// Compute the `first_run_done` flag path from `XDG_STATE_HOME` and `HOME`
/// (ignoring empty or relative environment paths).
pub fn flag_path_from(xdg_state_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    let absolute = |v: Option<&OsStr>| {
        v.filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let base = if let Some(xdg) = absolute(xdg_state_home) {
        xdg
    } else {
        absolute(home)?.join(".local").join("state")
    };
    Some(base.join("twitty").join(FIRST_RUN_FLAG_NAME))
}

/// Default path of the `first_run_done` flag for the current user.
pub fn default_flag_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let non_empty = |key: &str| std::env::var_os(key).filter(|v| !v.is_empty());
        let base = non_empty("LOCALAPPDATA").or_else(|| non_empty("APPDATA"))?;
        Some(PathBuf::from(base).join("twitty").join(FIRST_RUN_FLAG_NAME))
    }
    #[cfg(not(windows))]
    {
        let xdg = std::env::var_os("XDG_STATE_HOME");
        let home = std::env::var_os("HOME");
        flag_path_from(xdg.as_deref(), home.as_deref())
    }
}

/// Whether a valid, user-owned regular `first_run_done` flag file exists at `flag_path`.
/// Refuses symlinks, directories, oversized files, and group/world-writable files on Unix.
pub fn is_done_at(flag_path: &Path) -> bool {
    if let Some(parent) = flag_path.parent().filter(|p| !p.as_os_str().is_empty()) {
        match fs::symlink_metadata(parent) {
            Ok(meta) if meta.file_type().is_dir() => {}
            _ => return false,
        }
    }
    let Ok(meta) = fs::symlink_metadata(flag_path) else {
        return false;
    };
    if meta.file_type().is_symlink() || !meta.file_type().is_file() || meta.len() > MAX_FLAG_BYTES {
        return false;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let Ok(file) = options.open(flag_path) else {
        return false;
    };
    let Ok(fd_meta) = file.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(uid) = twitty_control::paths::current_uid() else {
            return false;
        };
        if fd_meta.uid() != uid || fd_meta.mode() & 0o6002 != 0 {
            return false;
        }
    }
    fd_meta.file_type().is_file() && fd_meta.len() <= MAX_FLAG_BYTES
}

/// Atomically persist the `first_run_done` flag at `flag_path` with `0600` permissions inside
/// a `0700` state directory. Refuses symlinked parents or targets.
pub fn mark_done_at(flag_path: &Path) -> io::Result<()> {
    let parent = flag_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("first-run flag path {} has no parent", flag_path.display()),
            )
        })?;
    ensure_state_dir(parent)?;

    match fs::symlink_metadata(flag_path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.file_type().is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing to write non-regular or symlinked {}",
                    flag_path.display()
                ),
            ));
        }
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }

    let pid = std::process::id();
    let (tmp_path, mut tmp_file) = create_temp_flag_file(parent, pid)?;
    let write_result = (|| -> io::Result<()> {
        tmp_file.write_all(b"done\n")?;
        tmp_file.sync_all()?;
        Ok(())
    })();
    drop(tmp_file);
    if let Err(err) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = fs::rename(&tmp_path, flag_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

/// Persist the `first_run_done` flag in the default state directory.
pub fn mark_done_default() -> io::Result<()> {
    let path = default_flag_path().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "could not determine Twitty state directory for first_run_done",
        )
    })?;
    mark_done_at(&path)
}

/// Whether the welcome screen should open automatically at startup:
/// true only when a state flag path is available, `first_run_done` is missing, and no config
/// file exists yet.
pub fn should_show_first_run(config_path: Option<&Path>, flag_path: Option<&Path>) -> bool {
    let Some(flag_path) = flag_path else {
        return false;
    };
    if is_done_at(flag_path) {
        return false;
    }
    if config_path.is_some_and(|p| fs::metadata(p).is_ok_and(|m| m.is_file())) {
        return false;
    }
    true
}

/// Convenience wrapper checking [`TwittyConfig::config_path`] and [`default_flag_path`].
pub fn should_show_first_run_default() -> bool {
    let config_path = TwittyConfig::config_path();
    let flag_path = default_flag_path();
    should_show_first_run(config_path.as_deref(), flag_path.as_deref())
}

fn ensure_state_dir(parent: &Path) -> io::Result<()> {
    match fs::symlink_metadata(parent) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "state directory {} is not a real directory",
                        parent.display()
                    ),
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if let Some(grandparent) = parent.parent() {
                if !grandparent.as_os_str().is_empty() {
                    fs::create_dir_all(grandparent)?;
                }
            }
            #[cfg_attr(not(unix), allow(unused_mut))]
            let mut builder = DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(parent) {
                Ok(()) => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
                    }
                    Ok(())
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => ensure_state_dir(parent),
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err),
    }
}

fn create_temp_flag_file(parent: &Path, pid: u32) -> io::Result<(PathBuf, fs::File)> {
    for _ in 0..64 {
        let seq = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = parent.join(format!(".{FIRST_RUN_FLAG_NAME}.tmp.{pid}.{seq}"));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&tmp_path) {
            Ok(file) => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
                }
                return Ok((tmp_path, file));
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate temporary file for first_run_done flag",
    ))
}

// ---- Curated wizard content ----

/// Curated theme swatches displayed on Step 2 (Look).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeSwatchSpec {
    /// Built-in theme key accepted by `Palette::named` and `config.theme`.
    pub id: &'static str,
    /// English; `label` translates it.
    pub name: &'static str,
}

impl ThemeSwatchSpec {
    pub fn label(self, language: UiLanguage) -> &'static str {
        language.t(self.name)
    }
}

/// Five featured themes: Twitty Dark, Twitty Light, plus three distinct built-ins.
pub const WELCOME_THEMES: &[ThemeSwatchSpec] = &[
    ThemeSwatchSpec {
        id: "twitty-dark",
        name: en!("Twitty Dark"),
    },
    ThemeSwatchSpec {
        id: "twitty-light",
        name: en!("Twitty Light"),
    },
    ThemeSwatchSpec {
        id: "tokyo-night",
        name: en!("Tokyo Night"),
    },
    ThemeSwatchSpec {
        id: "catppuccin-mocha",
        name: en!("Catppuccin"),
    },
    ThemeSwatchSpec {
        id: "nord",
        name: en!("Nord"),
    },
];

/// Curated command names shown on Step 5 (Shortcuts cheat sheet).
pub const WELCOME_SHORTCUT_COMMANDS: &[&str] = &[
    "palette.open",
    "composer.open",
    "settings.open",
    "split.right",
    "split.down",
    "search.all",
    "project.open",
    "review.toggle",
    "agent.ask",
    "sidebar.toggle",
];

/// One entry in the Step 5 shortcuts cheat sheet, pulled from the command table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutEntry {
    pub name: &'static str,
    pub title_en: &'static str,
    pub title_ar: &'static str,
    pub chord: Option<String>,
}

impl ShortcutEntry {
    pub fn title(&self, language: UiLanguage) -> &'static str {
        language.text(self.title_en, self.title_ar)
    }
}

/// Build the shortcuts cheat sheet from `commands`, keeping the curated order and
/// effective shortcut chords from the command table.
pub fn build_shortcut_entries(commands: &[Command]) -> Vec<ShortcutEntry> {
    WELCOME_SHORTCUT_COMMANDS
        .iter()
        .filter_map(|&name| {
            let cmd = commands.iter().find(|c| c.name == name)?;
            Some(ShortcutEntry {
                name: cmd.name,
                title_en: cmd.title_en,
                title_ar: cmd.title_ar,
                chord: cmd.shortcut.map(str::to_owned),
            })
        })
        .collect()
}

/// Detected shell & OSC 133 integration status shown on Step 3 (Shell).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellStatus {
    /// Cleaned shell basename (e.g. `"zsh"`, `"bash"`, `"fish"`).
    pub name: String,
    /// True when Twitty has automatic shell integration scripts for this shell.
    pub supported: bool,
    /// True when at least one OSC 133 mark has been observed in this session.
    pub marks_seen: bool,
}

impl ShellStatus {
    pub fn detect(pane_program: Option<&str>, marks_seen: bool) -> Self {
        let name = detect_shell_name(pane_program);
        let supported = Shell::of(&name).is_some();
        Self {
            name,
            supported,
            marks_seen,
        }
    }
}

/// One known coding agent CLI shown on Step 4 (AI and agents).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetectedAgent {
    pub name: &'static str,
    pub installed: bool,
    /// True when this agent is the one Twitty will invoke.
    pub active: bool,
}

/// Detected coding agents and whichever agent will be used by built-in AI features.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentsInfo {
    pub agents: Vec<DetectedAgent>,
    /// Display name of the agent that will actually be used (if any).
    pub selected: Option<String>,
    /// True when `selected` came from an explicit `ai_agent` or `agent_command` setting.
    pub from_config: bool,
}

impl AgentsInfo {
    /// Probe `AI_KNOWN_AGENTS` on `path` and determine which agent will be used given
    /// `ai_agent` and `agent_command` config overrides.
    pub fn detect(
        ai_agent: Option<&str>,
        agent_command: Option<&str>,
        path: Option<&OsStr>,
    ) -> Self {
        let dirs: Vec<PathBuf> = path
            .map(|p| std::env::split_paths(p).collect())
            .unwrap_or_default();
        let resolved = resolve_ai_agent(true, ai_agent, agent_command, path).ok();
        let from_config = [ai_agent, agent_command]
            .into_iter()
            .flatten()
            .any(|s| !s.trim().is_empty());
        let selected = resolved.as_deref().map(agent_display_name);
        let selected_stem = selected
            .as_deref()
            .and_then(|s| Path::new(s).file_stem())
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase);

        let agents = AI_KNOWN_AGENTS
            .iter()
            .map(|&name| {
                let installed = dirs
                    .iter()
                    .any(|dir| is_executable(&dir.join(exe_name(name))));
                let active = selected_stem.as_deref() == Some(name);
                DetectedAgent {
                    name,
                    installed,
                    active,
                }
            })
            .collect();

        Self {
            agents,
            selected,
            from_config,
        }
    }

    /// Detect using the current process `PATH`.
    pub fn detect_env(config: &TwittyConfig) -> Self {
        let path = std::env::var_os("PATH");
        Self::detect(
            config.ai_agent.as_deref(),
            config.agent_command.as_deref(),
            path.as_deref(),
        )
    }

    pub fn installed_count(&self) -> usize {
        self.agents.iter().filter(|a| a.installed).count()
    }
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

// ---- Wizard steps, state, live apply & revert ----

/// The five pages of the Welcome wizard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WelcomeStep {
    #[default]
    Welcome,
    Look,
    Shell,
    Ai,
    Shortcuts,
}

impl WelcomeStep {
    pub const ALL: [Self; 5] = [
        Self::Welcome,
        Self::Look,
        Self::Shell,
        Self::Ai,
        Self::Shortcuts,
    ];

    pub fn index(self) -> usize {
        match self {
            Self::Welcome => 0,
            Self::Look => 1,
            Self::Shell => 2,
            Self::Ai => 3,
            Self::Shortcuts => 4,
        }
    }

    pub fn from_index(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    pub fn is_first(self) -> bool {
        self == Self::Welcome
    }

    pub fn is_last(self) -> bool {
        self == Self::Shortcuts
    }

    pub fn next(self) -> Option<Self> {
        Self::ALL.get(self.index() + 1).copied()
    }

    pub fn prev(self) -> Option<Self> {
        self.index().checked_sub(1).map(Self::from_index)
    }

    pub fn title_en(self) -> &'static str {
        match self {
            Self::Welcome => en!("Welcome to Twitty"),
            Self::Look => en!("Look & Theme"),
            Self::Shell => en!("Shell Integration"),
            Self::Ai => en!("AI & Coding Agents"),
            Self::Shortcuts => en!("Essential Shortcuts"),
        }
    }

    pub fn title_ar(self) -> &'static str {
        UiLanguage::Arabic.t(self.title_en())
    }

    pub fn title(self, language: UiLanguage) -> &'static str {
        language.t(self.title_en())
    }

    pub fn subtitle_en(self) -> &'static str {
        match self {
            Self::Welcome => en!("An Arabic-first native terminal: the grid stays logical while drawing handles RTL and Arabic shaping."),
            Self::Look => en!("Pick a theme, font size, and transparency — changes preview live."),
            Self::Shell => en!("Enables command blocks, the smart composer, and finish notifications."),
            Self::Ai => en!("Built-in AI runs through your local coding agent CLI in read-only mode — no API keys or network calls in Twitty."),
            Self::Shortcuts => en!("The fastest ways to move around Twitty. Press Ctrl+Shift+P anytime for every command."),
        }
    }

    pub fn subtitle_ar(self) -> &'static str {
        UiLanguage::Arabic.t(self.subtitle_en())
    }

    pub fn subtitle(self, language: UiLanguage) -> &'static str {
        language.t(self.subtitle_en())
    }

    /// Number of focusable controls inside the body of this step.
    pub fn control_count(self) -> usize {
        match self {
            Self::Welcome => 1,   // language segmented selector
            Self::Look => 3,      // theme swatches, font size, transparency toggle
            Self::Shell => 1,     // shell integration toggle
            Self::Ai => 1,        // ai toggle
            Self::Shortcuts => 0, // read-only cheat sheet
        }
    }
}

/// Snapshot of config values taken when the Welcome wizard opens, enabling exact revert on
/// Cancel / Esc / Skip.
#[derive(Clone, Debug, PartialEq)]
pub struct WelcomeSnapshot {
    pub ui_language: String,
    pub theme: Option<String>,
    pub font_size: f32,
    pub background_opacity: f32,
    pub shell_integration: bool,
    pub ai: bool,
}

impl WelcomeSnapshot {
    pub fn capture(config: &TwittyConfig) -> Self {
        Self {
            ui_language: config.ui_language.clone(),
            theme: config.theme.clone(),
            font_size: config.font_size,
            background_opacity: config.background_opacity,
            shell_integration: config.shell_integration,
            ai: config.ai,
        }
    }
}

/// Which live window/renderer properties changed and must be pushed immediately.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WelcomeApply {
    pub language: bool,
    pub palette: bool,
    pub font_size: bool,
    pub opacity: bool,
}

impl WelcomeApply {
    pub fn any(self) -> bool {
        self.language || self.palette || self.font_size || self.opacity
    }
}

/// Result of handling an input event on the Welcome wizard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WelcomeOutcome {
    pub close: bool,
    pub redraw: bool,
    pub apply: WelcomeApply,
    /// Save `config` via `TwittyConfig::save()` (true on Finish).
    pub save_config: bool,
    /// Persist `first_run_done` flag in the state dir (true on Finish, Skip, Esc, Close).
    pub mark_done: bool,
}

/// Window-free state of the first-run Welcome wizard.
#[derive(Clone, Debug)]
pub struct WelcomeState {
    pub step: WelcomeStep,
    /// Focused control index within the active step's body.
    pub focused_control: usize,
    /// Effective UI language currently shown by the card (English LTR or Arabic RTL).
    pub language: UiLanguage,
    /// System locale fallback used when `config.ui_language == "auto"`.
    pub env_language: UiLanguage,
    /// Initial config captured on open so Cancel/Esc/Skip can revert live changes.
    pub snapshot: WelcomeSnapshot,
    /// Remembered translucent opacity (< 1.0) so toggling transparency off->on restores it.
    saved_transparent_opacity: f32,
    /// Detected shell and OSC 133 integration status for Step 3.
    pub shell: ShellStatus,
    /// Detected coding agents for Step 4.
    pub agents: AgentsInfo,
    /// Effective shortcut entries for Step 5.
    pub shortcuts: Vec<ShortcutEntry>,
    /// Currently hovered hit target.
    pub hover: Option<WelcomeHit>,
}

impl WelcomeState {
    pub fn new(
        config: &TwittyConfig,
        env_language: UiLanguage,
        shell: ShellStatus,
        agents: AgentsInfo,
        commands: &[Command],
    ) -> Self {
        let language = config.resolve_ui_language(env_language);
        let saved_transparent_opacity = if config.background_opacity < 0.99 {
            config.background_opacity
        } else {
            DEFAULT_TRANSPARENT_OPACITY
        };
        Self {
            step: WelcomeStep::Welcome,
            focused_control: 0,
            language,
            env_language,
            snapshot: WelcomeSnapshot::capture(config),
            saved_transparent_opacity,
            shell,
            agents,
            shortcuts: build_shortcut_entries(commands),
            hover: None,
        }
    }

    /// Construct with the live command table (`commands::all()`).
    pub fn with_default_commands(
        config: &TwittyConfig,
        env_language: UiLanguage,
        shell: ShellStatus,
        agents: AgentsInfo,
    ) -> Self {
        let all = commands::all();
        Self::new(config, env_language, shell, agents, &all)
    }

    pub fn mirrored(&self) -> bool {
        self.language == UiLanguage::Arabic
    }

    /// Whether window transparency is currently considered enabled (`background_opacity < 0.99`).
    pub fn transparency_enabled(config: &TwittyConfig) -> bool {
        config.background_opacity < 0.99
    }

    /// Active theme index among [`WELCOME_THEMES`], if the current theme is one of them.
    pub fn active_theme_index(config: &TwittyConfig) -> Option<usize> {
        let name = config.theme.as_deref().unwrap_or(WELCOME_THEMES[0].id);
        WELCOME_THEMES
            .iter()
            .position(|t| t.id.eq_ignore_ascii_case(name))
    }

    /// Update live OSC 133 status when a shell mark arrives while the wizard is open.
    pub fn set_shell_marks_seen(&mut self, seen: bool) -> bool {
        if self.shell.marks_seen != seen {
            self.shell.marks_seen = seen;
            true
        } else {
            false
        }
    }

    /// Jump to `step`, resetting per-step focus.
    pub fn go_to(&mut self, step: WelcomeStep) -> WelcomeOutcome {
        if self.step != step {
            self.step = step;
            self.focused_control = 0;
            WelcomeOutcome {
                redraw: true,
                ..Default::default()
            }
        } else {
            WelcomeOutcome::default()
        }
    }

    /// Set interface language preference (`"en"`, `"ar"`, or `"auto"`) and apply live.
    pub fn set_language_pref(
        &mut self,
        pref: UiLanguagePref,
        config: &mut TwittyConfig,
    ) -> WelcomeOutcome {
        let next_str = pref.as_str();
        let next_lang = pref.resolve_with(self.env_language);
        let mut outcome = WelcomeOutcome::default();
        if config.ui_language != next_str {
            config.ui_language = next_str.to_owned();
            outcome.redraw = true;
        }
        if self.language != next_lang {
            self.language = next_lang;
            outcome.apply.language = true;
            outcome.redraw = true;
        }
        outcome
    }

    /// Select one of [`WELCOME_THEMES`] by index and apply live.
    pub fn select_theme(&mut self, index: usize, config: &mut TwittyConfig) -> WelcomeOutcome {
        let Some(spec) = WELCOME_THEMES.get(index) else {
            return WelcomeOutcome::default();
        };
        let next = Some(spec.id.to_owned());
        if config.theme == next {
            return WelcomeOutcome::default();
        }
        config.theme = next;
        WelcomeOutcome {
            redraw: true,
            apply: WelcomeApply {
                palette: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Adjust font size by `delta_steps * FONT_SIZE_STEP` and apply live.
    pub fn step_font_size(
        &mut self,
        delta_steps: i32,
        config: &mut TwittyConfig,
    ) -> WelcomeOutcome {
        let raw = config.font_size + delta_steps as f32 * FONT_SIZE_STEP;
        let snapped = (clamp_font_size(raw) * 2.0).round() / 2.0;
        let next = snapped.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE);
        if (config.font_size - next).abs() < 1e-4 {
            return WelcomeOutcome::default();
        }
        config.font_size = next;
        WelcomeOutcome {
            redraw: true,
            apply: WelcomeApply {
                font_size: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Toggle window transparency on (`saved_transparent_opacity`) or off (`1.0`) and apply live.
    pub fn toggle_transparency(&mut self, config: &mut TwittyConfig) -> WelcomeOutcome {
        let currently_on = Self::transparency_enabled(config);
        let next = if currently_on {
            self.saved_transparent_opacity = config.background_opacity;
            1.0
        } else {
            self.saved_transparent_opacity.clamp(0.2, 0.98)
        };
        if (config.background_opacity - next).abs() < 1e-4 {
            return WelcomeOutcome::default();
        }
        config.background_opacity = next;
        WelcomeOutcome {
            redraw: true,
            apply: WelcomeApply {
                opacity: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Toggle `config.shell_integration`.
    pub fn toggle_shell_integration(&mut self, config: &mut TwittyConfig) -> WelcomeOutcome {
        config.shell_integration = !config.shell_integration;
        WelcomeOutcome {
            redraw: true,
            ..Default::default()
        }
    }

    /// Toggle `config.ai`.
    pub fn toggle_ai(&mut self, config: &mut TwittyConfig) -> WelcomeOutcome {
        config.ai = !config.ai;
        WelcomeOutcome {
            redraw: true,
            ..Default::default()
        }
    }

    /// Complete the wizard: keep all live changes, save config atomically, and mark done.
    pub fn finish(&mut self) -> WelcomeOutcome {
        WelcomeOutcome {
            close: true,
            redraw: true,
            apply: WelcomeApply::default(),
            save_config: true,
            mark_done: true,
        }
    }

    /// Cancel / Skip / Esc: revert all live-applied changes to the opening snapshot,
    /// mark `first_run_done` so the wizard does not auto-open again, and close.
    pub fn cancel(&mut self, config: &mut TwittyConfig) -> WelcomeOutcome {
        let mut apply = WelcomeApply::default();

        if config.ui_language != self.snapshot.ui_language {
            config.ui_language = self.snapshot.ui_language.clone();
        }
        let orig_lang = config.resolve_ui_language(self.env_language);
        if self.language != orig_lang {
            self.language = orig_lang;
            apply.language = true;
        }
        if config.theme != self.snapshot.theme {
            config.theme = self.snapshot.theme.clone();
            apply.palette = true;
        }
        if (config.font_size - self.snapshot.font_size).abs() > 1e-4 {
            config.font_size = self.snapshot.font_size;
            apply.font_size = true;
        }
        if (config.background_opacity - self.snapshot.background_opacity).abs() > 1e-4 {
            config.background_opacity = self.snapshot.background_opacity;
            apply.opacity = true;
        }
        config.shell_integration = self.snapshot.shell_integration;
        config.ai = self.snapshot.ai;

        WelcomeOutcome {
            close: true,
            redraw: true,
            apply,
            save_config: false,
            mark_done: true,
        }
    }

    /// Handle one keyboard press while the Welcome overlay is open.
    pub fn on_key(
        &mut self,
        key: &Key,
        modifiers: ModifiersState,
        config: &mut TwittyConfig,
    ) -> WelcomeOutcome {
        match key {
            Key::Named(NamedKey::Escape) => return self.cancel(config),
            Key::Named(NamedKey::Enter) => {
                return if let Some(next) = self.step.next() {
                    self.go_to(next)
                } else {
                    self.finish()
                };
            }
            Key::Named(NamedKey::Backspace) => {
                if let Some(prev) = self.step.prev() {
                    return self.go_to(prev);
                }
                return WelcomeOutcome::default();
            }
            Key::Named(NamedKey::PageDown) => {
                if let Some(next) = self.step.next() {
                    return self.go_to(next);
                }
                return WelcomeOutcome::default();
            }
            Key::Named(NamedKey::PageUp) => {
                if let Some(prev) = self.step.prev() {
                    return self.go_to(prev);
                }
                return WelcomeOutcome::default();
            }
            Key::Named(NamedKey::Tab) => {
                let count = self.step.control_count();
                if count > 1 {
                    self.focused_control = if modifiers.shift_key() {
                        (self.focused_control + count - 1) % count
                    } else {
                        (self.focused_control + 1) % count
                    };
                    return WelcomeOutcome {
                        redraw: true,
                        ..Default::default()
                    };
                }
                return WelcomeOutcome::default();
            }
            Key::Named(NamedKey::ArrowDown) => {
                let count = self.step.control_count();
                if count > 1 {
                    self.focused_control = (self.focused_control + 1) % count;
                    return WelcomeOutcome {
                        redraw: true,
                        ..Default::default()
                    };
                }
                return WelcomeOutcome::default();
            }
            Key::Named(NamedKey::ArrowUp) => {
                let count = self.step.control_count();
                if count > 1 {
                    self.focused_control = (self.focused_control + count - 1) % count;
                    return WelcomeOutcome {
                        redraw: true,
                        ..Default::default()
                    };
                }
                return WelcomeOutcome::default();
            }
            Key::Named(NamedKey::Space) => {
                return self.activate_focused_control(config);
            }
            Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowRight) => {
                let is_right = matches!(key, Key::Named(NamedKey::ArrowRight));
                let delta: i32 = if is_right { 1 } else { -1 };
                if modifiers.alt_key() {
                    let step_delta = if self.mirrored() { -delta } else { delta };
                    return if step_delta > 0 {
                        self.step
                            .next()
                            .map_or_else(Default::default, |s| self.go_to(s))
                    } else {
                        self.step
                            .prev()
                            .map_or_else(Default::default, |s| self.go_to(s))
                    };
                }
                return self.adjust_horizontal(delta, config);
            }
            Key::Character(s) if !modifiers.control_key() && !modifiers.alt_key() => {
                let ch = s.as_str();
                match ch {
                    "1" => return self.go_to(WelcomeStep::Welcome),
                    "2" => return self.go_to(WelcomeStep::Look),
                    "3" => return self.go_to(WelcomeStep::Shell),
                    "4" => return self.go_to(WelcomeStep::Ai),
                    "5" => return self.go_to(WelcomeStep::Shortcuts),
                    "+" | "=" if self.step == WelcomeStep::Look => {
                        return self.step_font_size(1, config);
                    }
                    "-" if self.step == WelcomeStep::Look => {
                        return self.step_font_size(-1, config);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        WelcomeOutcome::default()
    }

    fn activate_focused_control(&mut self, config: &mut TwittyConfig) -> WelcomeOutcome {
        match (self.step, self.focused_control) {
            (WelcomeStep::Welcome, 0) => {
                let cur = config.ui_language_pref();
                let idx = UiLanguagePref::ALL
                    .iter()
                    .position(|&p| p == cur)
                    .unwrap_or(0);
                let next = UiLanguagePref::ALL[(idx + 1) % UiLanguagePref::ALL.len()];
                self.set_language_pref(next, config)
            }
            (WelcomeStep::Look, 0) => {
                let idx = Self::active_theme_index(config).unwrap_or(0);
                self.select_theme((idx + 1) % WELCOME_THEMES.len(), config)
            }
            (WelcomeStep::Look, 2) => self.toggle_transparency(config),
            (WelcomeStep::Shell, 0) => self.toggle_shell_integration(config),
            (WelcomeStep::Ai, 0) => self.toggle_ai(config),
            _ => WelcomeOutcome::default(),
        }
    }

    fn adjust_horizontal(&mut self, raw_delta: i32, config: &mut TwittyConfig) -> WelcomeOutcome {
        // Mirror visual left/right movement for horizontally arranged items when RTL.
        let visual_delta = if self.mirrored() {
            -raw_delta
        } else {
            raw_delta
        };
        match (self.step, self.focused_control) {
            (WelcomeStep::Welcome, 0) => {
                let n = UiLanguagePref::ALL.len();
                let cur = config.ui_language_pref();
                let idx = UiLanguagePref::ALL
                    .iter()
                    .position(|&p| p == cur)
                    .unwrap_or(0);
                let next = if visual_delta > 0 {
                    (idx + 1) % n
                } else {
                    (idx + n - 1) % n
                };
                self.set_language_pref(UiLanguagePref::ALL[next], config)
            }
            (WelcomeStep::Look, 0) => {
                let n = WELCOME_THEMES.len();
                let idx = Self::active_theme_index(config).unwrap_or(0);
                let next = if visual_delta > 0 {
                    (idx + 1) % n
                } else {
                    (idx + n - 1) % n
                };
                self.select_theme(next, config)
            }
            (WelcomeStep::Look, 1) => {
                // Font size: right increases, left decreases (independent of text direction).
                self.step_font_size(raw_delta, config)
            }
            (WelcomeStep::Look, 2) => self.toggle_transparency(config),
            (WelcomeStep::Shell, 0) => self.toggle_shell_integration(config),
            (WelcomeStep::Ai, 0) => self.toggle_ai(config),
            (WelcomeStep::Shortcuts, _) => {
                if visual_delta > 0 {
                    self.step
                        .next()
                        .map_or_else(Default::default, |s| self.go_to(s))
                } else {
                    self.step
                        .prev()
                        .map_or_else(Default::default, |s| self.go_to(s))
                }
            }
            _ => WelcomeOutcome::default(),
        }
    }

    pub fn on_mouse_move(
        &mut self,
        x: f64,
        y: f64,
        surface: (f32, f32),
        scale: f32,
    ) -> WelcomeOutcome {
        let hit = WelcomeLayout::build(surface, scale, self).hit(x, y);
        let next = if hit == WelcomeHit::Outside || hit == WelcomeHit::Inside {
            None
        } else {
            Some(hit)
        };
        if self.hover != next {
            self.hover = next;
            WelcomeOutcome {
                redraw: true,
                ..Default::default()
            }
        } else {
            WelcomeOutcome::default()
        }
    }

    pub fn on_mouse_leave(&mut self) -> WelcomeOutcome {
        if self.hover.take().is_some() {
            WelcomeOutcome {
                redraw: true,
                ..Default::default()
            }
        } else {
            WelcomeOutcome::default()
        }
    }

    pub fn on_mouse_down(
        &mut self,
        x: f64,
        y: f64,
        config: &mut TwittyConfig,
        surface: (f32, f32),
        scale: f32,
    ) -> WelcomeOutcome {
        let hit = WelcomeLayout::build(surface, scale, self).hit(x, y);
        match hit {
            WelcomeHit::Outside | WelcomeHit::Close | WelcomeHit::Skip => self.cancel(config),
            WelcomeHit::Inside => WelcomeOutcome::default(),
            WelcomeHit::Back => self
                .step
                .prev()
                .map_or_else(Default::default, |s| self.go_to(s)),
            WelcomeHit::NextOrFinish => {
                if let Some(next) = self.step.next() {
                    self.go_to(next)
                } else {
                    self.finish()
                }
            }
            WelcomeHit::StepDot(step) => self.go_to(step),
            WelcomeHit::LanguageOption(pref) => {
                self.focused_control = 0;
                self.set_language_pref(pref, config)
            }
            WelcomeHit::ThemeSwatch(index) => {
                self.focused_control = 0;
                self.select_theme(index, config)
            }
            WelcomeHit::FontSizeMinus => {
                self.focused_control = 1;
                self.step_font_size(-1, config)
            }
            WelcomeHit::FontSizePlus => {
                self.focused_control = 1;
                self.step_font_size(1, config)
            }
            WelcomeHit::TransparencyToggle => {
                self.focused_control = 2;
                self.toggle_transparency(config)
            }
            WelcomeHit::ShellIntegrationToggle => {
                self.focused_control = 0;
                self.toggle_shell_integration(config)
            }
            WelcomeHit::AiToggle => {
                self.focused_control = 0;
                self.toggle_ai(config)
            }
        }
    }
}

// ---- Shared physical-pixel layout & hit testing ----

/// Pointer hit target inside (or outside) the Welcome wizard card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WelcomeHit {
    Outside,
    Inside,
    Close,
    Skip,
    Back,
    NextOrFinish,
    StepDot(WelcomeStep),
    LanguageOption(UiLanguagePref),
    ThemeSwatch(usize),
    FontSizeMinus,
    FontSizePlus,
    TransparencyToggle,
    ShellIntegrationToggle,
    AiToggle,
}

impl WelcomeHit {
    pub fn is_interactive(self) -> bool {
        !matches!(self, Self::Outside | Self::Inside)
    }
}

/// Physical-pixel geometry shared identically between `render::scene::welcome` and pointer hit testing.
#[derive(Clone, Debug)]
pub struct WelcomeLayout {
    pub step: WelcomeStep,
    pub card: Rect,
    pub mirrored: bool,
    pub header: Rect,
    pub header_title: Rect,
    pub step_counter: Rect,
    pub close_button: Rect,
    pub body: Rect,
    pub subtitle: Rect,
    pub footer: Rect,
    pub skip_button: Rect,
    pub dots: [(WelcomeStep, Rect); 5],
    pub back_button: Option<Rect>,
    pub next_button: Rect,
    // Step 1: Welcome
    pub hero_box: Rect,
    pub language_label: Rect,
    pub language_container: Rect,
    pub language_options: [(UiLanguagePref, Rect); 3],
    pub language_hint: Rect,
    // Step 2: Look
    pub theme_label: Rect,
    pub theme_swatches: Vec<Rect>,
    pub font_row: Rect,
    pub font_label: Rect,
    pub font_minus: Rect,
    pub font_value: Rect,
    pub font_plus: Rect,
    pub transparency_row: Rect,
    pub transparency_label: Rect,
    pub transparency_track: Rect,
    // Step 3: Shell
    pub shell_card: Rect,
    pub shell_toggle_row: Rect,
    pub shell_toggle_label: Rect,
    pub shell_toggle_track: Rect,
    // Step 4: AI & agents
    pub agent_cards: Vec<Rect>,
    pub agent_summary: Rect,
    pub ai_toggle_row: Rect,
    pub ai_toggle_label: Rect,
    pub ai_toggle_track: Rect,
    // Step 5: Shortcuts
    pub shortcut_rows: Vec<(Rect, Rect, Rect)>, // (row, title_rect, chord_rect)
}

impl WelcomeLayout {
    pub fn build(surface: (f32, f32), scale: f32, state: &WelcomeState) -> Self {
        let (sw, sh) = surface;
        let lang = state.language;
        let mirrored = state.mirrored();
        let px = |v: f32| (v * scale).round();

        let margin = px(SPACE_24);
        let card_w = px(620.0).min((sw - 2.0 * margin).max(0.0));
        let card_h = px(480.0).min((sh - 2.0 * margin).max(0.0));
        let card = Rect::new(
            ((sw - card_w) * 0.5).round(),
            ((sh - card_h) * 0.5).round(),
            card_w,
            card_h,
        );

        let pad_x = px(SPACE_24).min(card.w * 0.1);
        let header_h = px(52.0).min(card.h * 0.25);
        let footer_h = px(58.0).min(card.h * 0.25);
        let header = Rect::new(card.x, card.y, card.w, header_h);
        let footer = Rect::new(card.x, card.bottom() - footer_h, card.w, footer_h);
        let body = Rect::new(
            card.x + pad_x,
            header.bottom() + px(SPACE_16),
            (card.w - 2.0 * pad_x).max(0.0),
            (footer.y - header.bottom() - 2.0 * px(SPACE_16)).max(0.0),
        );

        // Header controls (laid out LTR, then mirrored for Arabic).
        let close_side = px(28.0).min(header_h - px(8.0)).max(0.0);
        let close_ltr = Rect::new(
            header.right() - pad_x - close_side,
            header.y + (header_h - close_side) * 0.5,
            close_side,
            close_side,
        );
        let counter_w = px(44.0);
        let counter_ltr = Rect::new(
            close_ltr.x - px(SPACE_8) - counter_w,
            header.y,
            counter_w,
            header_h,
        );
        let title_ltr = Rect::new(
            header.x + pad_x,
            header.y,
            (counter_ltr.x - px(SPACE_8) - (header.x + pad_x)).max(0.0),
            header_h,
        );
        let close_button = lang.mirror(close_ltr, header);
        let step_counter = lang.mirror(counter_ltr, header);
        let header_title = lang.mirror(title_ltr, header);

        // Footer: Skip on start, 5 dots centered, Back + Next/Finish on end.
        let btn_h = px(32.0).min(footer_h - px(SPACE_12)).max(0.0);
        let btn_y = footer.y + (footer_h - btn_h) * 0.5;
        let skip_w = px(82.0).min(footer.w * 0.22);
        let next_w = px(100.0).min(footer.w * 0.24);
        let back_w = px(82.0).min(footer.w * 0.20);
        let btn_gap = px(SPACE_8);

        let skip_ltr = Rect::new(footer.x + pad_x, btn_y, skip_w, btn_h);
        let next_ltr = Rect::new(footer.right() - pad_x - next_w, btn_y, next_w, btn_h);
        let back_ltr = (!state.step.is_first())
            .then(|| Rect::new(next_ltr.x - btn_gap - back_w, btn_y, back_w, btn_h));

        let skip_button = lang.mirror(skip_ltr, footer);
        let next_button = lang.mirror(next_ltr, footer);
        let back_button = back_ltr.map(|r| lang.mirror(r, footer));

        // Centered step dots (with generous clickable hit target).
        let dot_hit = px(18.0);
        let dot_gap = px(6.0);
        let total_dots_w = 5.0 * dot_hit + 4.0 * dot_gap;
        let dots_start_x = (footer.x + (footer.w - total_dots_w) * 0.5).round();
        let dot_y = footer.y + (footer_h - dot_hit) * 0.5;
        let mut dots = [(WelcomeStep::Welcome, Rect::new(0.0, 0.0, 0.0, 0.0)); 5];
        for (i, step) in WelcomeStep::ALL.into_iter().enumerate() {
            let r = Rect::new(
                dots_start_x + i as f32 * (dot_hit + dot_gap),
                dot_y,
                dot_hit,
                dot_hit,
            );
            dots[i] = (step, lang.mirror(r, footer));
        }

        // Subtitle at the top of body (2 lines max).
        let subtitle_h = px(38.0).min(body.h);
        let subtitle = Rect::new(body.x, body.y, body.w, subtitle_h);
        let content_y = subtitle.bottom() + px(SPACE_12);
        let content_h = (body.bottom() - content_y).max(0.0);

        // ---- Step 1: Welcome ----
        let hero_h = px(112.0).min(content_h * 0.45);
        let hero_box = Rect::new(body.x, content_y, body.w, hero_h);
        let lang_label_y = hero_box.bottom() + px(SPACE_24);
        let language_label = Rect::new(body.x, lang_label_y, body.w, px(20.0));
        let seg_h = px(40.0);
        let language_container =
            Rect::new(body.x, language_label.bottom() + px(SPACE_8), body.w, seg_h);
        let seg_pad = px(SPACE_4);
        let seg_inner_w = (language_container.w - 2.0 * seg_pad).max(0.0);
        let seg_gap = px(SPACE_4);
        let each_w = ((seg_inner_w - 2.0 * seg_gap) / 3.0).max(0.0);
        let mut language_options = [(UiLanguagePref::English, Rect::new(0.0, 0.0, 0.0, 0.0)); 3];
        for (i, pref) in UiLanguagePref::ALL.into_iter().enumerate() {
            let r = Rect::new(
                language_container.x + seg_pad + i as f32 * (each_w + seg_gap),
                language_container.y + seg_pad,
                each_w,
                (seg_h - 2.0 * seg_pad).max(0.0),
            );
            language_options[i] = (pref, lang.mirror(r, language_container));
        }
        let language_hint = Rect::new(
            body.x,
            language_container.bottom() + px(SPACE_8),
            body.w,
            px(20.0),
        );

        // ---- Step 2: Look ----
        let theme_label = Rect::new(body.x, content_y, body.w, px(20.0));
        let swatch_y = theme_label.bottom() + px(SPACE_8);
        let swatch_h = px(94.0).min(content_h * 0.42);
        let swatch_gap = px(10.0);
        let n_swatches = WELCOME_THEMES.len();
        let swatch_w = ((body.w - swatch_gap * (n_swatches.saturating_sub(1) as f32))
            / n_swatches as f32)
            .max(0.0);
        let theme_swatches: Vec<Rect> = (0..n_swatches)
            .map(|i| {
                let r = Rect::new(
                    body.x + i as f32 * (swatch_w + swatch_gap),
                    swatch_y,
                    swatch_w,
                    swatch_h,
                );
                lang.mirror(r, body)
            })
            .collect();

        let row_h = px(48.0);
        let row_gap = px(SPACE_12);
        let font_row = Rect::new(body.x, swatch_y + swatch_h + px(SPACE_16), body.w, row_h);
        let (font_label, font_minus, font_value, font_plus) =
            layout_stepper_row(font_row, scale, lang);

        let transparency_row = Rect::new(body.x, font_row.bottom() + row_gap, body.w, row_h);
        let (transparency_label, transparency_track) =
            layout_toggle_row(transparency_row, scale, lang);

        // ---- Step 3: Shell ----
        let shell_card_h = px(112.0).min(content_h * 0.5);
        let shell_card = Rect::new(body.x, content_y, body.w, shell_card_h);
        let shell_toggle_row =
            Rect::new(body.x, shell_card.bottom() + px(SPACE_16), body.w, px(52.0));
        let (shell_toggle_label, shell_toggle_track) =
            layout_toggle_row(shell_toggle_row, scale, lang);

        // ---- Step 4: AI & agents ----
        let agent_cols = if body.w >= px(360.0) { 2 } else { 1 };
        let agent_gap_x = px(10.0);
        let agent_gap_y = px(SPACE_8);
        let agent_card_h = px(46.0);
        let agent_card_w =
            ((body.w - agent_gap_x * (agent_cols - 1) as f32) / agent_cols as f32).max(0.0);
        let mut agent_cards = Vec::with_capacity(state.agents.agents.len());
        for i in 0..state.agents.agents.len() {
            let col = i % agent_cols;
            let row = i / agent_cols;
            let r = Rect::new(
                body.x + col as f32 * (agent_card_w + agent_gap_x),
                content_y + row as f32 * (agent_card_h + agent_gap_y),
                agent_card_w,
                agent_card_h,
            );
            agent_cards.push(lang.mirror(r, body));
        }
        let agent_rows = state.agents.agents.len().div_ceil(agent_cols);
        let agents_bottom = content_y
            + agent_rows as f32 * agent_card_h
            + agent_rows.saturating_sub(1) as f32 * agent_gap_y;
        let agent_summary = Rect::new(body.x, agents_bottom + px(SPACE_12), body.w, px(24.0));
        let ai_toggle_row = Rect::new(
            body.x,
            agent_summary.bottom() + px(SPACE_12),
            body.w,
            px(52.0),
        );
        let (ai_toggle_label, ai_toggle_track) = layout_toggle_row(ai_toggle_row, scale, lang);

        // ---- Step 5: Shortcuts ----
        let sc_cols = if body.w >= px(420.0) { 2 } else { 1 };
        let sc_gap_x = px(SPACE_12);
        let sc_gap_y = px(6.0);
        let sc_row_h = px(38.0);
        let sc_col_w = ((body.w - sc_gap_x * (sc_cols - 1) as f32) / sc_cols as f32).max(0.0);
        let mut shortcut_rows = Vec::with_capacity(state.shortcuts.len());
        let rows_per_col = state.shortcuts.len().div_ceil(sc_cols).max(1);
        for i in 0..state.shortcuts.len() {
            let col = i / rows_per_col;
            let row = i % rows_per_col;
            let r_ltr = Rect::new(
                body.x + col as f32 * (sc_col_w + sc_gap_x),
                content_y + row as f32 * (sc_row_h + sc_gap_y),
                sc_col_w,
                sc_row_h,
            );
            let r = lang.mirror(r_ltr, body);
            let pad = px(10.0);
            let chord_w = px(108.0).min(r.w * 0.44);
            let chord_ltr = Rect::new(
                r.right() - pad - chord_w,
                r.y + px(7.0),
                chord_w,
                r.h - px(14.0),
            );
            let title_ltr = Rect::new(
                r.x + pad,
                r.y,
                (chord_ltr.x - px(SPACE_8) - (r.x + pad)).max(0.0),
                r.h,
            );
            shortcut_rows.push((r, lang.mirror(title_ltr, r), lang.mirror(chord_ltr, r)));
        }

        Self {
            step: state.step,
            card,
            mirrored,
            header,
            header_title,
            step_counter,
            close_button,
            body,
            subtitle,
            footer,
            skip_button,
            dots,
            back_button,
            next_button,
            hero_box,
            language_label,
            language_container,
            language_options,
            language_hint,
            theme_label,
            theme_swatches,
            font_row,
            font_label,
            font_minus,
            font_value,
            font_plus,
            transparency_row,
            transparency_label,
            transparency_track,
            shell_card,
            shell_toggle_row,
            shell_toggle_label,
            shell_toggle_track,
            agent_cards,
            agent_summary,
            ai_toggle_row,
            ai_toggle_label,
            ai_toggle_track,
            shortcut_rows,
        }
    }

    /// Hit-test physical pixel `(x, y)` against the card chrome and the active step's controls.
    pub fn hit(&self, x: f64, y: f64) -> WelcomeHit {
        self.hit_for_step(self.step, x, y)
    }

    /// Step-aware hit testing (tests both chrome and `step`'s controls).
    pub fn hit_for_step(&self, step: WelcomeStep, x: f64, y: f64) -> WelcomeHit {
        if !self.card.contains(x, y) {
            return WelcomeHit::Outside;
        }
        if self.close_button.contains(x, y) {
            return WelcomeHit::Close;
        }
        if self.skip_button.contains(x, y) {
            return WelcomeHit::Skip;
        }
        if self.back_button.is_some_and(|r| r.contains(x, y)) {
            return WelcomeHit::Back;
        }
        if self.next_button.contains(x, y) {
            return WelcomeHit::NextOrFinish;
        }
        for &(dot_step, rect) in &self.dots {
            if rect.contains(x, y) {
                return WelcomeHit::StepDot(dot_step);
            }
        }
        match step {
            WelcomeStep::Welcome => {
                for &(pref, rect) in &self.language_options {
                    if rect.contains(x, y) {
                        return WelcomeHit::LanguageOption(pref);
                    }
                }
            }
            WelcomeStep::Look => {
                for (i, rect) in self.theme_swatches.iter().enumerate() {
                    if rect.contains(x, y) {
                        return WelcomeHit::ThemeSwatch(i);
                    }
                }
                if self.font_minus.contains(x, y) {
                    return WelcomeHit::FontSizeMinus;
                }
                if self.font_plus.contains(x, y) {
                    return WelcomeHit::FontSizePlus;
                }
                if self.transparency_row.contains(x, y) {
                    return WelcomeHit::TransparencyToggle;
                }
            }
            WelcomeStep::Shell => {
                if self.shell_toggle_row.contains(x, y) {
                    return WelcomeHit::ShellIntegrationToggle;
                }
            }
            WelcomeStep::Ai => {
                if self.ai_toggle_row.contains(x, y) {
                    return WelcomeHit::AiToggle;
                }
            }
            WelcomeStep::Shortcuts => {}
        }
        WelcomeHit::Inside
    }
}

impl WelcomeState {
    /// Step-aware hit test helper.
    pub fn hit_at(&self, x: f64, y: f64, surface: (f32, f32), scale: f32) -> WelcomeHit {
        WelcomeLayout::build(surface, scale, self).hit_for_step(self.step, x, y)
    }
}

fn layout_toggle_row(row: Rect, scale: f32, lang: UiLanguage) -> (Rect, Rect) {
    let px = |v: f32| (v * scale).round();
    let pad = px(SPACE_12);
    let track_w = px(40.0).min(row.w * 0.25);
    let track_h = px(22.0).min(row.h - px(SPACE_8)).max(0.0);
    let track_ltr = Rect::new(
        row.right() - pad - track_w,
        row.y + (row.h - track_h) * 0.5,
        track_w,
        track_h,
    );
    let label_ltr = Rect::new(
        row.x + pad,
        row.y,
        (track_ltr.x - px(SPACE_12) - (row.x + pad)).max(0.0),
        row.h,
    );
    (lang.mirror(label_ltr, row), lang.mirror(track_ltr, row))
}

fn layout_stepper_row(row: Rect, scale: f32, lang: UiLanguage) -> (Rect, Rect, Rect, Rect) {
    let px = |v: f32| (v * scale).round();
    let pad = px(SPACE_12);
    let btn_side = px(28.0).min(row.h - px(SPACE_8)).max(0.0);
    let val_w = px(62.0);
    let gap = px(SPACE_8);
    let btn_y = row.y + (row.h - btn_side) * 0.5;

    let plus_ltr = Rect::new(row.right() - pad - btn_side, btn_y, btn_side, btn_side);
    let value_ltr = Rect::new(plus_ltr.x - gap - val_w, row.y, val_w, row.h);
    let minus_ltr = Rect::new(value_ltr.x - gap - btn_side, btn_y, btn_side, btn_side);
    let label_ltr = Rect::new(
        row.x + pad,
        row.y,
        (minus_ltr.x - px(SPACE_12) - (row.x + pad)).max(0.0),
        row.h,
    );

    // Mirror the label vs control group, while keeping minus/value/plus visually intuitive
    // (minus on left, plus on right inside the stepper group).
    let group_x = minus_ltr.x;
    let group_w = plus_ltr.right() - minus_ltr.x;
    let group = lang.mirror(Rect::new(group_x, row.y, group_w, row.h), row);
    let minus = Rect::new(group.x, btn_y, btn_side, btn_side);
    let value = Rect::new(minus.right() + gap, row.y, val_w, row.h);
    let plus = Rect::new(value.right() + gap, btn_y, btn_side, btn_side);
    (lang.mirror(label_ltr, row), minus, value, plus)
}
