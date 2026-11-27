//! Automatic shell integration: starting bash, zsh and fish so they report prompts, commands
//! and their exit status (OSC 133), command lines (OSC 633 E) and the working directory
//! (OSC 7), without the user changing their startup files.
//!
//! The scripts (`assets/shell/`) are written to a private per-user directory
//! (`$XDG_RUNTIME_DIR/twitty/shell/<digest>`, else `~/.cache/twitty/shell/<digest>`, mode 0700,
//! where `<digest>` names this build's scripts) the first time a shell is started, each
//! atomically. Then:
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

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use crate::pty::SpawnOptions;

const BASH: &str = include_str!("../../assets/shell/twitty.bash");
const ZSH: &str = include_str!("../../assets/shell/twitty.zsh");
const FISH: &str = include_str!("../../assets/shell/twitty.fish");
const CLAUDE_WRAPPER: &str = include_str!("../../assets/bin/claude");

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

/// Read a regular (non-symlink) file up to `expected.len() + 1` bytes and return whether its
/// contents match `expected` and its permissions match `expected_mode` (on Unix).
fn matches_private_file(path: &Path, expected: &[u8], expected_mode: u32) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.file_type().is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o7777 != expected_mode {
            return false;
        }
    }
    #[cfg(not(unix))]
    let _ = expected_mode;

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let Ok(file) = options.open(path) else {
        return false;
    };
    if !file.metadata().is_ok_and(|m| m.file_type().is_file()) {
        return false;
    }
    let mut existing = Vec::with_capacity(expected.len());
    file.take(expected.len() as u64 + 1)
        .read_to_end(&mut existing)
        .is_ok()
        && existing == expected
}

/// Atomically write `body` to `path` with mode `mode` (on Unix), using an exclusive temporary
/// file in the same directory so concurrent spawns never collide and the file never exists with
/// wider permissions.
fn write_private_file(path: &Path, label: &str, body: &[u8], mode: u32) -> io::Result<()> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let pid = std::process::id();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    let _ = mode;

    let mut created = None;
    for _ in 0..64 {
        let seq = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_file_name(format!(".{label}.{pid}.{seq}.tmp"));
        match options.open(&tmp) {
            Ok(file) => {
                created = Some((tmp, file));
                break;
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    let Some((tmp, mut file)) = created else {
        return Err(io::Error::other("could not allocate temporary script file"));
    };

    let result = (|| -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        file.write_all(body)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Write the scripts into `dir`, creating it (mode 0700) as needed. Each file is replaced
/// atomically and only when its content or permissions differ, so shells starting meanwhile
/// (in this or another Twitty) never read half a file.
pub fn install(dir: &Path) -> io::Result<()> {
    create_private_dir(dir)?;
    for (name, body) in files() {
        let path = dir.join(&name);
        if let Some(parent) = path.parent() {
            create_private_dir(parent)?;
        }
        if matches_private_file(&path, body.as_bytes(), 0o600) {
            continue;
        }
        let file_name = name
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("script");
        write_private_file(&path, file_name, body.as_bytes(), 0o600)?;
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
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let uid = crate::control::paths::current_uid()?;
        if meta.uid() != uid {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "private directory {} is owned by uid {}, expected {uid}",
                    dir.display(),
                    meta.uid()
                ),
            ));
        }
        if meta.permissions().mode() & 0o6077 != 0 {
            fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

/// Base private directory: `$XDG_RUNTIME_DIR/twitty`, else `$XDG_CACHE_HOME/twitty`, else
/// `$HOME/.cache/twitty`. Relative environment paths are ignored.
fn private_base_dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute() && d.is_dir());
    let base = match runtime {
        Some(dir) => dir,
        None => {
            let cache = std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .filter(|d| d.is_absolute());
            cache.or_else(|| {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|d| d.is_absolute())?;
                Some(home.join(".cache"))
            })?
        }
    };
    Some(base.join("twitty"))
}

/// Where the scripts go: `$XDG_RUNTIME_DIR/twitty/shell`, else `~/.cache/twitty/shell`.
pub fn default_dir() -> Option<PathBuf> {
    Some(private_base_dir()?.join("shell"))
}

/// A short digest of the scripts this build writes. Each build keeps its scripts in its own
/// subdirectory, so an updated Twitty never runs an older copy left in the runtime directory,
/// and two different versions running at once don't overwrite each other's scripts.
pub fn scripts_version() -> String {
    let mut all = Vec::new();
    for (name, body) in files() {
        all.extend_from_slice(name.to_string_lossy().as_bytes());
        all.push(0);
        all.extend_from_slice(body.as_bytes());
        all.push(0);
    }
    crate::workflows::sha256_hex(&all)[..12].to_string()
}

/// The script directory (`default_dir()/<scripts_version()>`), written on first use in this
/// process and again if it disappeared (e.g. the runtime directory was cleaned at logout).
fn script_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    static INSTALLED: OnceLock<()> = OnceLock::new();
    let dir = DIR
        .get_or_init(|| default_dir().map(|d| d.join(scripts_version())))
        .clone()?;
    let ready = INSTALLED.get().is_some() && dir.join("twitty.bash").is_file();
    if !ready {
        if let Err(e) = install(&dir) {
            tracing::warn!("shell integration unavailable: {}: {e}", dir.display());
            return None;
        }
        let _ = INSTALLED.set(());
    }
    Some(dir)
}

/// Where Twitty's private wrapper binaries (such as `claude`) go:
/// `$XDG_RUNTIME_DIR/twitty/bin`, else `~/.cache/twitty/bin`.
pub fn default_bin_dir() -> Option<PathBuf> {
    Some(private_base_dir()?.join("bin"))
}

/// Write Twitty's embedded `claude` wrapper script into `dir` (mode 0700 dir, mode 0700
/// executable file, atomic replace). Returns the path to `dir/claude`.
pub fn install_claude_wrapper(dir: &Path) -> io::Result<PathBuf> {
    create_private_dir(dir)?;
    let path = dir.join("claude");
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if !meta.file_type().is_file() {
            return Err(io::Error::other("wrapper path is not a regular file"));
        }
    }
    if !matches_private_file(&path, CLAUDE_WRAPPER.as_bytes(), 0o700) {
        write_private_file(&path, "claude", CLAUDE_WRAPPER.as_bytes(), 0o700)?;
    }
    Ok(path)
}

/// Prepend `bin_dir` to a colon-separated `PATH`, skipping duplicate entries for `bin_dir`.
pub fn prepend_path(bin_dir: &Path, existing: Option<&str>) -> String {
    let dir_str = bin_dir.to_string_lossy();
    let mut parts = vec![dir_str.as_ref()];
    if let Some(existing) = existing {
        for entry in existing.split(':').filter(|s| !s.is_empty()) {
            if entry != dir_str.as_ref() {
                parts.push(entry);
            }
        }
    }
    parts.join(":")
}

fn claude_bin_dir() -> Option<PathBuf> {
    static BIN_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    let dir = BIN_DIR.get_or_init(default_bin_dir).clone()?;
    if let Err(e) = install_claude_wrapper(&dir) {
        tracing::warn!("claude hook wrapper unavailable: {}: {e}", dir.display());
        return None;
    }
    Some(dir)
}

/// Put Twitty's private `claude` wrapper first on `PATH` and expose `TWITTY_BIN` so Claude Code
/// hooks call back into this Twitty binary.
pub fn apply_claude_wrapper(options: &mut SpawnOptions) {
    if cfg!(windows) {
        return;
    }
    let Some(dir) = claude_bin_dir() else {
        return;
    };
    let existing = options
        .env
        .iter()
        .rev()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var("PATH").ok());
    let new_path = prepend_path(&dir, existing.as_deref());
    options.env.push(("PATH".into(), new_path));
    // Startup files often prepend their own directories (~/.local/bin) to PATH; the shell
    // integration puts this directory first again after they ran.
    if let Some(dir) = dir.to_str() {
        options
            .env
            .push(("TWITTY_CLAUDE_DIR".into(), dir.to_owned()));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_str) = exe.to_str() {
            options.env.push(("TWITTY_BIN".into(), exe_str.to_owned()));
        }
    }
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
        options.shell = true;
    }
}
