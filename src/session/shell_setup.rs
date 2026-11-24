//! Automatic shell integration: starting bash, zsh and fish so they report prompts, commands
//! and their exit status (OSC 133), command lines (OSC 633 E) and the working directory
//! (OSC 7), without the user changing their startup files.
//!
//! The scripts (`assets/shell/`) are written to a private per-user directory
//! (`$XDG_RUNTIME_DIR/twitty/shell`, else `~/.cache/twitty/shell`, mode 0700) the first time a
//! shell is started, each atomically. Then:
//!
//! - bash runs with `--rcfile twitty.bash`. That file reads `~/.bashrc` (or, for `bash -l`,
//!   `/etc/profile` and the first of `~/.bash_profile`, `~/.bash_login`, `~/.profile`) and
//!   then adds its hooks to PROMPT_COMMAND and PS0, keeping the user's.
//! - zsh gets `ZDOTDIR` pointing at a directory whose startup files read the user's own from
//!   their ZDOTDIR (or `$HOME`) and give ZDOTDIR back after the last one; the hooks go into
//!   precmd/preexec after the user's `.zshrc`.
//! - fish gets a `vendor_conf.d` directory through `XDG_DATA_DIRS`; the script puts
//!   XDG_DATA_DIRS back first thing and uses fish's prompt and exec events.
//!
//! Only when no explicit command is configured, and only for those three shells started with
//! no arguments (or just `-l`/`--login`/`-i`): anything else runs untouched. The scripts add
//! nothing when the user's own startup files already mark prompts with OSC 133. The shell gets
//! `TWITTY_SHELL_INTEGRATION=1`. Nothing on Windows yet.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::pty::SpawnOptions;

const BASH: &str = include_str!("../../assets/shell/twitty.bash");
const ZSH: &str = include_str!("../../assets/shell/twitty.zsh");
const FISH: &str = include_str!("../../assets/shell/twitty.fish");

/// The zsh startup files placed in Twitty's ZDOTDIR: each hands over to `twitty.zsh`, which
/// reads the user's file of the same name.
const ZSH_STARTUP_FILES: [&str; 4] = [".zshenv", ".zprofile", ".zshrc", ".zlogin"];

/// Every file written, relative to the script directory.
fn files() -> Vec<(PathBuf, String)> {
    let mut files = vec![
        (PathBuf::from("twitty.bash"), BASH.to_string()),
        (PathBuf::from("twitty.zsh"), ZSH.to_string()),
        (
            ["fish", "fish", "vendor_conf.d", "twitty.fish"]
                .iter()
                .collect(),
            FISH.to_string(),
        ),
    ];
    for name in ZSH_STARTUP_FILES {
        let body = format!(
            "# Twitty shell integration: reads your own {name}, see twitty.zsh.\n\
             builtin source -- \"${{ZDOTDIR:h}}/twitty.zsh\" {name}\n"
        );
        files.push((Path::new("zsh").join(name), body));
    }
    files
}

/// A shell Twitty knows how to integrate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
}

impl Shell {
    /// The shell a program path names (`/usr/bin/zsh`, `fish`...).
    pub fn of(program: &str) -> Option<Shell> {
        let name = Path::new(program).file_name()?.to_str()?;
        match name {
            "bash" => Some(Shell::Bash),
            "zsh" => Some(Shell::Zsh),
            "fish" => Some(Shell::Fish),
            _ => None,
        }
    }
}

/// How to start a shell with the integration loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Integration {
    /// Program and arguments.
    pub command: Vec<String>,
    /// Environment to add.
    pub env: Vec<(String, String)>,
}

/// The command and environment that start `program args` with the scripts in `dir` loaded, or
/// `None` when it is not a shell started in a way Twitty can integrate. `env` reads the
/// environment the shell would inherit.
pub fn integrate(
    program: &str,
    args: &[String],
    dir: &Path,
    env: impl Fn(&str) -> Option<String>,
) -> Option<Integration> {
    let shell = Shell::of(program)?;
    let login = match args {
        [] => false,
        [arg] if arg == "-l" || arg == "--login" => true,
        [arg] if arg == "-i" => false,
        _ => return None,
    };
    let dir = dir.to_str()?;
    let mut command = vec![program.to_string()];
    let mut vars = vec![("TWITTY_SHELL_INTEGRATION".to_string(), "1".to_string())];
    let mut var = |key: &str, value: String| vars.push((key.to_string(), value));
    match shell {
        Shell::Bash => {
            // A login bash ignores --rcfile; the script reads the login files itself instead.
            command.extend(["--rcfile".to_string(), format!("{dir}/twitty.bash")]);
            if args.iter().any(|a| a == "-i") {
                command.push("-i".into());
            }
            var("__TWITTY_BASH_RCFILE", "1".into());
            if login {
                var("__TWITTY_BASH_LOGIN", "1".into());
            }
        }
        Shell::Zsh => {
            command.extend(args.iter().cloned());
            if let Some(user) = env("ZDOTDIR") {
                var("__TWITTY_ZSH_ZDOTDIR", user);
            }
            var("__TWITTY_ZSH_BOOTSTRAP", "1".into());
            var("ZDOTDIR", format!("{dir}/zsh"));
        }
        Shell::Fish => {
            command.extend(args.iter().cloned());
            let user = env("XDG_DATA_DIRS").filter(|d| !d.is_empty());
            // Without XDG_DATA_DIRS fish uses these; they must stay on the list.
            let rest = user.clone().unwrap_or("/usr/local/share:/usr/share".into());
            var("__TWITTY_FISH_XDG_DATA_DIRS", user.unwrap_or_default());
            var("XDG_DATA_DIRS", format!("{dir}/fish:{rest}"));
        }
    }
    Some(Integration { command, env: vars })
}

/// Write the scripts into `dir`, creating it (mode 0700) as needed. Each file is replaced
/// atomically and only when its content differs, so shells starting meanwhile (in this or
/// another Twitty) never read half a file.
pub fn install(dir: &Path) -> io::Result<()> {
    create_private_dir(dir)?;
    for (name, body) in files() {
        let path = dir.join(&name);
        if let Some(parent) = path.parent() {
            create_private_dir(parent)?;
        }
        if fs::read(&path).is_ok_and(|old| old == body.as_bytes()) {
            continue;
        }
        let file_name = name
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("script");
        let tmp = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
        let written = fs::File::create(&tmp).and_then(|mut f| {
            f.write_all(body.as_bytes())?;
            f.sync_all()
        });
        if let Err(e) = written.and_then(|()| fs::rename(&tmp, &path)) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    }
    Ok(())
}

/// Create `dir` (and missing parents) readable only by the user; an existing one is made so,
/// and one that is a symlink or belongs to someone else is refused.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() {
        return Err(io::Error::other("not a directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Only the owner can change the mode, so this also refuses someone else's directory.
        if meta.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Where the scripts go: `$XDG_RUNTIME_DIR/twitty/shell`, else `~/.cache/twitty/shell`.
pub fn default_dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute() && d.is_dir());
    let base = match runtime {
        Some(dir) => dir,
        None => {
            let cache = std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .filter(|d| d.is_absolute());
            cache.or_else(|| Some(PathBuf::from(std::env::var_os("HOME")?).join(".cache")))?
        }
    };
    Some(base.join("twitty").join("shell"))
}

/// The script directory, written once per process (and again if it disappeared, e.g. when
/// the runtime directory was cleaned at logout).
fn script_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    let dir = DIR.get_or_init(default_dir).clone()?;
    let ready = dir.join("twitty.bash").is_file();
    if !ready {
        if let Err(e) = install(&dir) {
            tracing::warn!("shell integration unavailable: {}: {e}", dir.display());
            return None;
        }
    }
    Some(dir)
}

/// Load the integration into the shell `options` start, when they start the user's shell
/// (no explicit command) and it is bash, zsh or fish.
pub fn apply(options: &mut SpawnOptions) {
    if cfg!(windows) || !options.command.is_empty() {
        return;
    }
    let (program, args) = crate::pty::default_shell();
    let Some(shell) = Shell::of(&program) else {
        return;
    };
    let Some(dir) = script_dir() else {
        return;
    };
    let env = |key: &str| {
        let from_options = options.env.iter().rev().find(|(k, _)| k == key);
        match from_options {
            Some((_, value)) => Some(value.clone()),
            None => std::env::var(key).ok(),
        }
    };
    if let Some(integration) = integrate(&program, &args, &dir, env) {
        tracing::debug!("shell integration for {shell:?}");
        options.command = integration.command;
        options.env.extend(integration.env);
    }
}
