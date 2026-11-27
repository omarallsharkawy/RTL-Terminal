//! Custom commands ("workflows"): user commands from `config.json` and project commands from
//! `.twitty/commands.json`, with per-file cryptographic trust and window-free execution helpers.

pub mod discover;
pub mod parse;
pub mod sha256;
pub mod trust;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use winit::keyboard::{Key, NamedKey};

use crate::commands;
use crate::input::encode_paste;
use crate::pty::default_shell;
use crate::shell_integration::clean_text;
use crate::ui::palette::{PaletteEntry, PaletteId};
use crate::ui::palette::{PaletteItem, PaletteView};

pub use discover::{discover_project_file, discover_project_file_default};
pub use parse::{
    deserialize_config_commands, parse_entries_slice, parse_entry, parse_project_bytes,
};
pub use sha256::{sha256, sha256_hex};
pub use trust::{TrustEntry, TrustStore, MAX_TRUST_FILE_BYTES, TRUST_STORE_VERSION};

/// Maximum custom commands accepted from one source (`config.json` or `.twitty/commands.json`).
pub const MAX_COMMANDS_PER_SOURCE: usize = 100;
/// Maximum cleaned length of `name` and `name_ar`, in characters.
pub const MAX_NAME_CHARS: usize = 120;
/// Maximum cleaned length of `command`, in characters.
pub const MAX_COMMAND_CHARS: usize = 4096;
/// Maximum cleaned length of `description`, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 240;
/// Maximum cleaned length of `cwd`, in characters.
pub const MAX_CWD_CHARS: usize = 4096;
/// Maximum `.twitty/commands.json` size in bytes (64 KiB).
pub const MAX_PROJECT_FILE_BYTES: usize = 64 * 1024;
/// Maximum parent directories walked when looking for `.twitty/commands.json`.
pub const MAX_DISCOVERY_LEVELS: usize = 32;
/// Maximum entries kept in `trust.json` (oldest dropped first).
pub const MAX_TRUST_ENTRIES: usize = 500;

/// How a custom command executes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommandMode {
    /// Paste the command into the focused pane without a newline so the user can review it.
    #[default]
    Insert,
    /// Open a split (or tab) and run `$SHELL -c <command>`.
    Run,
}

impl CommandMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Run => "run",
        }
    }
}

/// One custom command definition (shared by `config.json` and `.twitty/commands.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name_ar: String,
    pub command: String,
    #[serde(default)]
    pub mode: CommandMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Where a custom command came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkflowSource {
    User,
    Project {
        /// Canonical project root containing `.twitty/commands.json`.
        root: PathBuf,
        /// Cleaned directory basename shown as a small tag in the palette.
        tag: String,
        /// SHA-256 hex digest of `.twitty/commands.json`.
        sha256: String,
    },
}

impl WorkflowSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project { .. } => "project",
        }
    }
}

/// A parsed, discovered project `.twitty/commands.json` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectFile {
    pub root: PathBuf,
    pub tag: String,
    pub sha256: String,
    pub commands: Vec<WorkflowSpec>,
}

/// One resolved custom command with its origin and trust status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workflow {
    pub spec: WorkflowSpec,
    pub source: WorkflowSource,
    pub trusted: bool,
}

/// Combined user + discovered project commands for one pane context.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkflowSet {
    workflows: Vec<Workflow>,
    project: Option<ProjectFile>,
}

/// Why `command.run` over the control socket refused a custom command name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlRunError {
    NotFound(String),
    Untrusted(String),
}

impl fmt::Display for ControlRunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(name) => write!(f, "unknown command: {name}"),
            Self::Untrusted(name) => write!(
                f,
                "refusing to run untrusted project command {name:?}; trust it in the command palette first"
            ),
        }
    }
}

impl WorkflowSet {
    /// Load user commands and any discovered project commands for `pane_cwd` using the default
    /// trust store.
    pub fn load(user_commands: &[WorkflowSpec], pane_cwd: Option<&Path>) -> Self {
        let project = pane_cwd.and_then(discover_project_file_default);
        let trust = if project.is_some() {
            TrustStore::load_default()
        } else {
            TrustStore::default()
        };
        Self::from_parts(user_commands, project, &trust)
    }

    /// Build a [`WorkflowSet`] from explicit components (window-free, testable without touching
    /// `$HOME` or `$XDG_STATE_HOME`).
    pub fn from_parts(
        user_commands: &[WorkflowSpec],
        project: Option<ProjectFile>,
        trust: &TrustStore,
    ) -> Self {
        let mut workflows = Vec::new();
        for spec in user_commands.iter().take(MAX_COMMANDS_PER_SOURCE) {
            workflows.push(Workflow {
                spec: spec.clone(),
                source: WorkflowSource::User,
                trusted: true,
            });
        }
        if let Some(ref proj) = project {
            let trusted = trust.is_trusted(&proj.root, &proj.sha256);
            let source = WorkflowSource::Project {
                root: proj.root.clone(),
                tag: proj.tag.clone(),
                sha256: proj.sha256.clone(),
            };
            for spec in proj.commands.iter().take(MAX_COMMANDS_PER_SOURCE) {
                workflows.push(Workflow {
                    spec: spec.clone(),
                    source: source.clone(),
                    trusted,
                });
            }
        }
        Self { workflows, project }
    }

    pub fn workflows(&self) -> &[Workflow] {
        &self.workflows
    }

    pub fn get(&self, index: usize) -> Option<&Workflow> {
        self.workflows.get(index)
    }

    pub fn project(&self) -> Option<&ProjectFile> {
        self.project.as_ref()
    }

    /// Mark the discovered project file as trusted in memory (after persisting to `TrustStore`).
    pub fn mark_project_trusted(&mut self) {
        for wf in &mut self.workflows {
            if matches!(wf.source, WorkflowSource::Project { .. }) {
                wf.trusted = true;
            }
        }
    }

    /// Built-in commands followed by custom commands, ready for the generic command palette.
    pub fn palette_entries(&self) -> Vec<PaletteEntry> {
        self.palette_entries_with(crate::input::keybindings::KeyBindings::defaults())
    }

    /// Built-in commands (showing effective shortcuts from `bindings`) followed by custom commands.
    pub fn palette_entries_with(
        &self,
        bindings: &crate::input::keybindings::KeyBindings,
    ) -> Vec<PaletteEntry> {
        let mut entries = PaletteEntry::builtins_with(bindings);
        for (idx, wf) in self.workflows.iter().enumerate() {
            let shortcut = match &wf.source {
                WorkflowSource::User => None,
                WorkflowSource::Project { tag, .. } => Some(if wf.trusted {
                    format!("[{tag}]")
                } else {
                    format!("⚠ [{tag}]")
                }),
            };
            let mut search_key = wf.spec.command.clone();
            if let Some(desc) = &wf.spec.description {
                search_key.push(' ');
                search_key.push_str(desc);
            }
            if let WorkflowSource::Project { tag, .. } = &wf.source {
                search_key.push(' ');
                search_key.push_str(tag);
            }
            entries.push(PaletteEntry {
                id: PaletteId::Custom(idx),
                name: search_key,
                title_en: wf.spec.name.clone(),
                title_ar: wf.spec.name_ar.clone(),
                shortcut,
            });
        }
        entries
    }

    /// JSON list returned by the `command.list` control socket method: built-ins plus custom
    /// commands, each with `name`, `source` (`"builtin"`, `"user"`, or `"project"`), and `trusted`.
    pub fn control_list_json(&self) -> Vec<Value> {
        self.control_list_json_with(crate::input::keybindings::KeyBindings::defaults())
    }

    /// Like [`Self::control_list_json`], reporting effective shortcuts from `bindings`.
    pub fn control_list_json_with(
        &self,
        bindings: &crate::input::keybindings::KeyBindings,
    ) -> Vec<Value> {
        let mut out: Vec<Value> = commands::all()
            .into_iter()
            .map(|c| {
                json!({
                    "name": c.name,
                    "title_en": c.title_en,
                    "title_ar": c.title_ar,
                    "shortcut": bindings.display_shortcut(c.name),
                    "source": "builtin",
                    "trusted": true,
                })
            })
            .collect();

        for wf in &self.workflows {
            let project = match &wf.source {
                WorkflowSource::User => Value::Null,
                WorkflowSource::Project { tag, .. } => Value::String(tag.clone()),
            };
            out.push(json!({
                "name": wf.spec.name,
                "title_en": wf.spec.name,
                "title_ar": wf.spec.name_ar,
                "shortcut": Value::Null,
                "source": wf.source.as_str(),
                "trusted": wf.trusted,
                "mode": wf.spec.mode.as_str(),
                "project": project,
            }));
        }
        out
    }

    /// Look up a custom command for `command.run` over the control socket.
    ///
    /// User commands are checked before project commands so an untrusted repo cannot shadow a
    /// user command. Untrusted project commands are always refused.
    pub fn resolve_control_run(&self, name: &str) -> Result<Workflow, ControlRunError> {
        // User commands first (always trusted).
        if let Some(wf) = self
            .workflows
            .iter()
            .find(|wf| matches!(wf.source, WorkflowSource::User) && wf.spec.name == name)
        {
            return Ok(wf.clone());
        }
        // Project commands: require trust.
        if let Some(wf) = self
            .workflows
            .iter()
            .find(|wf| matches!(wf.source, WorkflowSource::Project { .. }) && wf.spec.name == name)
        {
            if wf.trusted {
                return Ok(wf.clone());
            }
            return Err(ControlRunError::Untrusted(name.to_owned()));
        }
        Err(ControlRunError::NotFound(name.to_owned()))
    }
}

/// Encode a command for `insert` mode: never emits a newline, and wraps in bracketed paste when
/// `bracketed` is true.
pub fn encode_insert(command: &str, bracketed: bool) -> Vec<u8> {
    let single_line: String = command
        .chars()
        .filter(|&c| c != '\r' && c != '\n' && c != '\0')
        .collect();
    encode_paste(&single_line, bracketed)
}

/// Resolve the working directory for `run` mode.
///
/// For project commands with an entry `cwd`, resolves relative to the project root and refuses
/// any path that is absolute, missing, not a directory, or escapes the canonical project root
/// (including via `..` or symlinks). When `cwd` is omitted, falls back to `pane_cwd`.
pub fn resolve_run_cwd(
    workflow: &Workflow,
    pane_cwd: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let Some(raw_cwd) = workflow.spec.cwd.as_deref() else {
        return Ok(pane_cwd.map(Path::to_path_buf));
    };
    match &workflow.source {
        WorkflowSource::Project { root, .. } => resolve_project_cwd(root, raw_cwd).map(Some),
        WorkflowSource::User => {
            let candidate = Path::new(raw_cwd);
            let target = if candidate.is_absolute() {
                candidate.to_path_buf()
            } else if let Some(base) = pane_cwd {
                base.join(candidate)
            } else {
                return Err(format!(
                    "cannot resolve relative cwd {raw_cwd:?} without a pane directory"
                ));
            };
            if !target.is_dir() {
                let shown = clean_text(&target.to_string_lossy(), 256);
                return Err(format!(
                    "workflow cwd is not an existing directory: {shown}"
                ));
            }
            Ok(Some(target))
        }
    }
}

/// Resolve `relative` inside `project_root`, refusing absolute paths or any canonicalised target
/// outside `project_root`.
pub fn resolve_project_cwd(project_root: &Path, relative: &str) -> Result<PathBuf, String> {
    use std::path::Component;

    let rel = Path::new(relative);
    let shown_rel = clean_text(relative, 256);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| matches!(c, Component::Prefix(_) | Component::RootDir))
    {
        return Err(format!(
            "refusing workflow cwd {shown_rel:?}: must be relative to project root"
        ));
    }
    let canon_root = fs::canonicalize(project_root).map_err(|err| {
        let shown_root = clean_text(&project_root.to_string_lossy(), 256);
        format!("could not resolve project root {shown_root}: {err}")
    })?;
    // Push components one by one so Windows verbatim `\\?\` paths never receive unnormalized
    // `/` separators or `.` / `..` segments that Win32 rejects on verbatim paths.
    let mut joined = canon_root.clone();
    for comp in rel.components() {
        match comp {
            Component::Normal(part) => joined.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                joined = fs::canonicalize(&joined).map_err(|err| {
                    format!("workflow cwd {shown_rel:?} does not exist inside project root: {err}")
                })?;
                joined.pop();
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(format!(
                    "refusing workflow cwd {shown_rel:?}: must be relative to project root"
                ));
            }
        }
    }
    let canon_target = fs::canonicalize(&joined).map_err(|err| {
        format!("workflow cwd {shown_rel:?} does not exist inside project root: {err}")
    })?;
    if !canon_target.starts_with(&canon_root) {
        let shown_root = clean_text(&canon_root.to_string_lossy(), 256);
        return Err(format!(
            "refusing workflow cwd {shown_rel:?}: escapes project root {shown_root}"
        ));
    }
    if !canon_target.is_dir() {
        return Err(format!("workflow cwd {shown_rel:?} is not a directory"));
    }
    Ok(canon_target)
}

/// Build the argv to execute `command` with the user's shell (`$SHELL -c <command>`).
pub fn run_shell_argv(command: &str) -> Vec<String> {
    let (shell, extra_args) = default_shell();
    run_shell_argv_for(&shell, &extra_args, command)
}

/// Pure helper for [`run_shell_argv`], testable without mutating environment variables.
pub fn run_shell_argv_for(shell: &str, extra_args: &[String], command: &str) -> Vec<String> {
    let mut argv = Vec::with_capacity(extra_args.len() + 3);
    argv.push(shell.to_owned());
    argv.extend(extra_args.iter().cloned());
    let flag = shell_command_flag(shell);
    argv.push(flag.to_owned());
    argv.push(command.to_owned());
    argv
}

fn shell_command_flag(shell: &str) -> &'static str {
    let basename = Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(shell)
        .to_ascii_lowercase();
    if basename == "cmd.exe" || basename == "cmd" {
        "/C"
    } else if basename == "pwsh.exe"
        || basename == "pwsh"
        || basename == "powershell.exe"
        || basename == "powershell"
    {
        "-Command"
    } else {
        "-c"
    }
}

/// Action returned by [`TrustConfirmState::key`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmCmd {
    None,
    Trust,
    Cancel,
}

/// Key state for the palette trust confirmation prompt.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustConfirmState {
    /// Scroll offset into the project's command preview list.
    pub scroll: usize,
}

impl TrustConfirmState {
    pub fn key(&mut self, key: &Key, command_count: usize) -> ConfirmCmd {
        match key {
            Key::Named(NamedKey::Enter) => ConfirmCmd::Trust,
            Key::Named(NamedKey::Escape) => ConfirmCmd::Cancel,
            Key::Named(NamedKey::ArrowDown) => {
                if self.scroll + 1 < command_count {
                    self.scroll += 1;
                }
                ConfirmCmd::None
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.scroll = self.scroll.saturating_sub(1);
                ConfirmCmd::None
            }
            _ => ConfirmCmd::None,
        }
    }
}

/// Build the [`PaletteView`] for confirming trust of an untrusted project file, showing the
/// bilingual confirmation item ("Trust commands in `<dir>`? / تثق في أوامر `<dir>`؟") followed by
/// the file's full command list.
pub fn trust_confirm_view(project: &ProjectFile, scroll: usize) -> PaletteView {
    let dir_display = clean_text(&project.root.to_string_lossy(), 96);
    let query = format!("⚠ {dir_display} — Enter = trust · Esc = cancel");

    let mut items = Vec::with_capacity(1 + project.commands.len());
    items.push(PaletteItem {
        title_en: format!("Trust commands in {}?", project.tag),
        title_ar: format!("تثق في أوامر {}؟", project.tag),
        shortcut: Some("Enter / Esc".into()),
        dimmed: false,
    });

    let start = scroll.min(project.commands.len());
    for cmd in &project.commands[start..] {
        let mode_tag = cmd.mode.as_str();
        let cwd_part = match &cmd.cwd {
            Some(c) => format!(" ({c})"),
            None => String::new(),
        };
        items.push(PaletteItem {
            title_en: format!("[{mode_tag}] {}{cwd_part}:", cmd.name),
            title_ar: cmd.name_ar.clone(),
            ..PaletteItem::default()
        });
        // The whole command, never cut: runs of spaces are collapsed (so padding cannot push
        // the end out of sight) and long commands continue on the next rows.
        for chunk in confirm_rows(&cmd.command) {
            items.push(PaletteItem {
                title_en: format!("    {chunk}"),
                ..PaletteItem::default()
            });
        }
    }

    PaletteView {
        query,
        items,
        selected: 0,
        placeholder: None,
    }
}

/// Characters of a command shown per row in the trust confirmation.
pub const CONFIRM_ROW_CHARS: usize = 64;

/// `command` as shown for trust: whitespace runs collapsed to one space, cut into rows of at
/// most [`CONFIRM_ROW_CHARS`] characters, nothing dropped.
pub fn confirm_rows(command: &str) -> Vec<String> {
    let flat = command.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = flat.chars().collect();
    chars
        .chunks(CONFIRM_ROW_CHARS)
        .map(|c| c.iter().collect())
        .collect()
}
