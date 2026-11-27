//! Declarative command completion specifications for the composer.
//!
//! Specifications are stored in an embedded JSON document (`specs_data.json`) parsed on first use
//! via `serde_json` (no extra dependency) and exposed via [`default_registry`]. External/test
//! callers can also parse custom specs with [`parse_specs`].

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::ui::chrome::UiLanguage;

/// Embedded built-in completion specs.
const BUILTIN_SPECS_JSON: &str = include_str!("specs_data.json");

static DEFAULT_REGISTRY: OnceLock<SpecRegistry> = OnceLock::new();

/// Expected value or positional argument kind for completion and dynamic generators.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArgKind {
    /// Any filesystem path (file or directory).
    Path,
    /// Regular file path.
    File,
    /// Directory path.
    Dir,
    /// Inner command (used by wrappers such as `sudo` to delegate completion).
    Command,
    /// Git local/remote branch names.
    GitBranch,
    /// Git configured remotes.
    GitRemote,
    /// Git tags.
    GitTag,
    /// Git refs (branches and tags).
    GitRef,
    /// Modified or untracked files in a git worktree.
    GitChangedFile,
    /// Scripts defined in `package.json`.
    NpmScript,
    /// Targets defined in `Makefile`.
    MakeTarget,
    /// Cargo targets (binaries, examples, tests, benches) from `Cargo.toml`.
    CargoTarget,
    /// Cargo feature flags from `Cargo.toml`.
    CargoFeature,
    /// Running or stopped Docker containers.
    DockerContainer,
    /// Local Docker images.
    DockerImage,
    /// Systemd unit names.
    SystemctlUnit,
    /// SSH host aliases from `~/.ssh/config`.
    SshHost,
    /// Fixed list of known values (e.g., resource types, output formats).
    Values(Vec<String>),
    /// Arbitrary freeform text (no structured value candidates).
    Any,
}

/// Specification for one short and/or long command-line option.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct OptionSpec {
    #[serde(default)]
    pub short: Option<String>,
    #[serde(default)]
    pub long: Option<String>,
    #[serde(default)]
    pub takes_value: bool,
    #[serde(default)]
    pub value: Option<ArgKind>,
    pub desc: String,
    #[serde(default)]
    pub desc_ar: Option<String>,
}

impl OptionSpec {
    /// True when this option consumes the following argument (or `=value`).
    pub fn expects_value(&self) -> bool {
        self.takes_value || self.value.is_some()
    }

    /// Expected argument kind for this option's value, if it takes one.
    pub fn value_kind(&self) -> Option<&ArgKind> {
        if let Some(kind) = &self.value {
            Some(kind)
        } else if self.takes_value {
            Some(&ArgKind::Any)
        } else {
            None
        }
    }

    /// True if `flag` matches either the short or long spelling of this option.
    pub fn matches_flag(&self, flag: &str) -> bool {
        self.short.as_deref() == Some(flag) || self.long.as_deref() == Some(flag)
    }

    /// Preferred display label combining short and long forms (e.g. `"-m, --message"`).
    pub fn display_label(&self) -> String {
        match (&self.short, &self.long) {
            (Some(s), Some(l)) => format!("{s}, {l}"),
            (None, Some(l)) => l.clone(),
            (Some(s), None) => s.clone(),
            (None, None) => String::new(),
        }
    }

    /// Canonical flag string inserted when accepting this option for `typed_prefix`.
    pub fn insert_flag(&self, typed_prefix: &str) -> &str {
        if typed_prefix.starts_with("--") {
            if let Some(l) = &self.long {
                return l.as_str();
            }
        }
        if typed_prefix.starts_with('-')
            && !typed_prefix.starts_with("--")
            && typed_prefix.len() > 1
        {
            if let Some(s) = &self.short {
                if s.starts_with(typed_prefix) {
                    return s.as_str();
                }
            }
        }
        self.long.as_deref().or(self.short.as_deref()).unwrap_or("")
    }

    pub fn description(&self, lang: UiLanguage) -> &str {
        match lang {
            UiLanguage::Arabic => self.desc_ar.as_deref().unwrap_or(&self.desc),
            UiLanguage::English => &self.desc,
        }
    }
}

/// Specification for a top-level command or nested subcommand.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct CommandSpec {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub desc: String,
    #[serde(default)]
    pub desc_ar: Option<String>,
    #[serde(default)]
    pub options: Vec<OptionSpec>,
    #[serde(default)]
    pub subcommands: Vec<CommandSpec>,
    #[serde(default)]
    pub args: Vec<ArgKind>,
}

impl CommandSpec {
    /// Look up a direct subcommand by name or alias.
    pub fn find_subcommand(&self, name: &str) -> Option<&CommandSpec> {
        self.subcommands
            .iter()
            .find(|sub| sub.name == name || sub.aliases.iter().any(|a| a == name))
    }

    /// Look up an option by exact flag (`-m` or `--message`).
    pub fn find_option(&self, flag: &str) -> Option<&OptionSpec> {
        self.options.iter().find(|opt| opt.matches_flag(flag))
    }

    /// True if this command delegates to an inner command (e.g. `sudo`).
    pub fn delegates_command(&self) -> bool {
        self.args.iter().any(|a| matches!(a, ArgKind::Command))
    }

    /// Positional argument kind for the `index`-th positional argument (0-based).
    /// Repeats the final declared argument kind when `index >= self.args.len()`.
    pub fn positional_arg(&self, index: usize) -> Option<&ArgKind> {
        self.args.get(index).or_else(|| self.args.last())
    }

    pub fn description(&self, lang: UiLanguage) -> &str {
        match lang {
            UiLanguage::Arabic => self.desc_ar.as_deref().unwrap_or(&self.desc),
            UiLanguage::English => &self.desc,
        }
    }
}

#[derive(Deserialize)]
struct SpecFile {
    commands: Vec<CommandSpec>,
}

/// Registry of parsed command specifications with fast lookup by name or alias.
#[derive(Clone, Debug)]
pub struct SpecRegistry {
    commands: Vec<CommandSpec>,
    index: HashMap<String, usize>,
}

impl SpecRegistry {
    /// Build a validated registry from a list of command specs.
    pub fn from_commands(commands: Vec<CommandSpec>) -> Result<Self, String> {
        let mut index = HashMap::new();
        for (i, cmd) in commands.iter().enumerate() {
            validate_command(cmd)?;
            index.insert(cmd.name.clone(), i);
            for alias in &cmd.aliases {
                if alias.is_empty() {
                    return Err(format!("empty alias in command `{}`", cmd.name));
                }
                index.insert(alias.clone(), i);
            }
        }
        Ok(Self { commands, index })
    }

    /// Look up a top-level command spec by name or alias.
    pub fn get(&self, name: &str) -> Option<&CommandSpec> {
        self.index.get(name).map(|&i| &self.commands[i])
    }

    /// All registered top-level command specs.
    pub fn commands(&self) -> &[CommandSpec] {
        &self.commands
    }

    /// True if `name` is a known command or alias in the registry.
    pub fn contains(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }
}

fn validate_command(cmd: &CommandSpec) -> Result<(), String> {
    if cmd.name.trim().is_empty() {
        return Err("command spec has empty name".into());
    }
    for opt in &cmd.options {
        if opt.short.is_none() && opt.long.is_none() {
            return Err(format!(
                "option in `{}` must specify `short` or `long`",
                cmd.name
            ));
        }
    }
    for sub in &cmd.subcommands {
        validate_command(sub)?;
    }
    Ok(())
}

/// Parse a declarative JSON spec document into a [`SpecRegistry`].
pub fn parse_specs(json: &str) -> Result<SpecRegistry, String> {
    let file: SpecFile =
        serde_json::from_str(json).map_err(|e| format!("invalid completion spec JSON: {e}"))?;
    SpecRegistry::from_commands(file.commands)
}

/// Return the built-in completion spec registry (parsed once on first use).
pub fn default_registry() -> &'static SpecRegistry {
    DEFAULT_REGISTRY.get_or_init(|| {
        parse_specs(BUILTIN_SPECS_JSON).expect("built-in completion specs JSON must be valid")
    })
}
