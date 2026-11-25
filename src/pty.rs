use std::env;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

/// Maximum queued input chunks per pane before writes fail fast instead of growing without bound.
const WRITE_QUEUE_CAPACITY: usize = 256;

/// Variables a parent terminal (or tmux) sets for its own shells. Inherited by a Twitty
/// started from inside one of them, they make programs talk that terminal's protocols
/// (kitty graphics, iTerm2 escapes) or believe they run inside tmux.
pub const INHERITED_TERMINAL_VARS: &[&str] = &[
    "ALACRITTY_LOG",
    "ALACRITTY_SOCKET",
    "ALACRITTY_WINDOW_ID",
    "CMUX_CLAUDE_HOOKS_DISABLED",
    "CMUX_CLAUDE_PID",
    "CMUX_PANEL_ID",
    "CMUX_SOCKET",
    "CMUX_SOCKET_MODE",
    "CMUX_SOCKET_PASSWORD",
    "CMUX_SOCKET_PATH",
    "CMUX_SURFACE_ID",
    "CMUX_TAB_ID",
    "CMUX_WORKSPACE_ID",
    "COLUMNS",
    "GHOSTTY_BIN_DIR",
    "GHOSTTY_RESOURCES_DIR",
    "ITERM_PROFILE",
    "ITERM_SESSION_ID",
    "KITTY_INSTALLATION_DIR",
    "KITTY_LISTEN_ON",
    "KITTY_PID",
    "KITTY_PUBLIC_KEY",
    "KITTY_WINDOW_ID",
    "KONSOLE_DBUS_SERVICE",
    "KONSOLE_DBUS_SESSION",
    "KONSOLE_DBUS_WINDOW",
    "KONSOLE_VERSION",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "LINES",
    "TERMINFO",
    "TMUX",
    "TMUX_PANE",
    // Twitty's own shell integration (`session::shell_setup`), from a Twitty this one was
    // started in: set again for the shells that get it.
    "TWITTY_BIN",
    "TWITTY_CLAUDE_DIR",
    "TWITTY_SHELL_INTEGRATION",
    "__TWITTY_BASH_LOGIN",
    "__TWITTY_BASH_RCFILE",
    "__TWITTY_FISH_XDG_DATA_DIRS",
    "__TWITTY_ZSH_BOOTSTRAP",
    "__TWITTY_ZSH_ZDOTDIR",
    "TWITTY_PANE_ID",
    "TWITTY_SOCKET",
    "VTE_VERSION",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_PANE",
    "WEZTERM_UNIX_SOCKET",
    "WT_PROFILE_ID",
    "WT_SESSION",
];

/// Whether an inherited environment variable name is stripped from child PTYs before applying
/// Twitty's own pane environment.
pub fn is_stripped_env_var(name: &str) -> bool {
    name.starts_with("CMUX_") || INHERITED_TERMINAL_VARS.contains(&name)
}

/// Program output waiting for the UI thread, shared with the PTY reader thread.
///
/// Reads are appended to one buffer and the UI is woken only when it was empty, so a flood of
/// output (`cat` of a big log) becomes a few large batches instead of one event per 8 KiB read.
/// The buffer is bounded: a full one makes the reader wait, so the program is slowed to the
/// speed the screen keeps up with and Ctrl+C or a keypress is never stuck behind megabytes of
/// queued output.
#[derive(Clone, Default)]
pub struct OutputQueue(Arc<(Mutex<QueueState>, Condvar)>);

#[derive(Default)]
struct QueueState {
    data: Vec<u8>,
    /// The UI went away: drop output instead of waiting for room.
    closed: bool,
}

impl OutputQueue {
    /// Bytes queued before the reader waits for the UI.
    pub const LIMIT: usize = 4 << 20;

    /// Append output, waiting while the queue is full. True when the queue was empty, so the
    /// UI must be told there is output to take.
    pub fn push(&self, bytes: &[u8]) -> bool {
        let (lock, room) = &*self.0;
        let Ok(mut state) = lock.lock() else {
            return false;
        };
        while state.data.len() >= Self::LIMIT && !state.closed {
            state = match room.wait(state) {
                Ok(s) => s,
                Err(_) => return false,
            };
        }
        if state.closed {
            return false;
        }
        let was_empty = state.data.is_empty();
        state.data.extend_from_slice(bytes);
        was_empty
    }

    /// Everything queued so far, emptying the queue and waking a waiting reader.
    pub fn take(&self) -> Vec<u8> {
        let (lock, room) = &*self.0;
        let data = lock
            .lock()
            .map(|mut s| std::mem::take(&mut s.data))
            .unwrap_or_default();
        room.notify_all();
        data
    }

    /// Stop queueing (the pane is gone); a waiting reader is released.
    pub fn close(&self) {
        let (lock, room) = &*self.0;
        if let Ok(mut s) = lock.lock() {
            s.closed = true;
            s.data = Vec::new();
        }
        room.notify_all();
    }
}

pub struct Pty {
    /// Output read from the shell, taken by the UI with `take_output`.
    output: OutputQueue,
    master: Box<dyn MasterPty + Send>,
    /// Writes go through a dedicated bounded channel so a large paste never blocks the UI and
    /// a wedged child cannot grow memory without bound.
    writer: SyncSender<Vec<u8>>,
    /// Ends the shell when the pane is closed (the reader thread keeps the PTY open otherwise).
    killer: Box<dyn ChildKiller + Send + Sync>,
    program_name: Option<String>,
    direct_program: bool,
    /// Process id of the shell, to find its working directory and listening ports (Linux only).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pid: Option<u32>,
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.output.close();
        let _ = self.killer.kill();
    }
}

/// How to start a pane's process.
#[derive(Clone, Debug, Default)]
pub struct SpawnOptions {
    /// Starting directory, used when it exists.
    pub cwd: Option<std::path::PathBuf>,
    /// Program and arguments to run instead of the user's shell. Empty: the shell.
    pub command: Vec<String>,
    /// Extra environment variables, applied last.
    pub env: Vec<(String, String)>,
}

impl Pty {
    /// Start the default shell. `on_output` is called (on the reader thread) when output arrives
    /// in an empty queue; take it with `take_output`. `on_exit` is called when the shell ends.
    pub fn spawn<F, E>(cols: u16, rows: u16, on_output: F, on_exit: E) -> Result<Self>
    where
        F: FnMut() + Send + 'static,
        E: FnMut() + Send + 'static,
    {
        Self::spawn_in(None, cols, rows, on_output, on_exit)
    }

    /// Like `spawn`, starting the shell in `cwd` when it is an existing directory.
    pub fn spawn_in<F, E>(
        cwd: Option<&Path>,
        cols: u16,
        rows: u16,
        on_output: F,
        on_exit: E,
    ) -> Result<Self>
    where
        F: FnMut() + Send + 'static,
        E: FnMut() + Send + 'static,
    {
        let options = SpawnOptions {
            cwd: cwd.map(Path::to_path_buf),
            ..SpawnOptions::default()
        };
        Self::spawn_with(&options, cols, rows, on_output, on_exit)
    }

    /// Start `options.command` (default: the user's shell) with extra environment variables.
    pub fn spawn_with<F, E>(
        options: &SpawnOptions,
        cols: u16,
        rows: u16,
        mut on_output: F,
        mut on_exit: E,
    ) -> Result<Self>
    where
        F: FnMut() + Send + 'static,
        E: FnMut() + Send + 'static,
    {
        Self::spawn_with_status(
            options,
            cols,
            rows,
            move |_, wake| {
                if wake {
                    on_output();
                }
            },
            move |_| on_exit(),
        )
    }

    pub(crate) fn spawn_with_status<F, E>(
        options: &SpawnOptions,
        cols: u16,
        rows: u16,
        mut on_output: F,
        mut on_exit: E,
    ) -> Result<Self>
    where
        F: FnMut(&[u8], bool) + Send + 'static,
        E: FnMut(Option<u32>) + Send + 'static,
    {
        let cols = cols.max(1);
        let rows = rows.max(1);
        let cwd = options.cwd.as_deref();
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY pair")?;

        let mut cmd = match options.command.split_first() {
            // An explicit program and its arguments, run directly (no shell parses them).
            Some((program, args)) => {
                let mut cmd = CommandBuilder::new(program);
                cmd.args(args);
                cmd
            }
            None => {
                let (shell_cmd, shell_args) = default_shell();
                let mut cmd = CommandBuilder::new(shell_cmd);
                for arg in shell_args {
                    cmd.arg(arg);
                }
                cmd
            }
        };
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Twitty");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        for key in INHERITED_TERMINAL_VARS {
            cmd.env_remove(key);
        }
        for (key, _) in env::vars_os() {
            if let Some(name) = key.to_str() {
                if name.starts_with("CMUX_") {
                    cmd.env_remove(name);
                }
            }
        }
        if env::var_os("LANG").is_none() && env::var_os("LC_ALL").is_none() {
            cmd.env("LANG", "en_US.UTF-8");
        }

        if let Some(dir) = cwd.filter(|d| d.is_dir()) {
            cmd.cwd(dir);
        }
        for (key, value) in &options.env {
            cmd.env(key, value);
        }

        let program_name = cmd
            .get_argv()
            .first()
            .and_then(|program| Path::new(program).file_name())
            .map(|name| name.to_string_lossy().into_owned());
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn shell in PTY")?;
        drop(pair.slave);

        let killer = child.clone_killer();
        let pid = child.process_id();
        let mut reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;
        let mut pty_writer = pair
            .master
            .take_writer()
            .context("Failed to take PTY writer")?;
        let (writer, write_rx) = sync_channel::<Vec<u8>>(WRITE_QUEUE_CAPACITY);
        if let Err(err) = thread::Builder::new()
            .name("twitty-pty-writer".into())
            .spawn(move || {
                for data in write_rx {
                    if pty_writer
                        .write_all(&data)
                        .and_then(|_| pty_writer.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err).context("Failed to spawn PTY writer thread");
        }

        struct ChildGuard(Option<Box<dyn portable_pty::Child + Send + Sync>>);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                if let Some(mut c) = self.0.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
        }

        let output = OutputQueue::default();
        let queue = output.clone();
        let mut child_guard = ChildGuard(Some(child));
        thread::Builder::new()
            .name("twitty-pty-reader".into())
            .spawn(move || {
                let mut buf = [0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let bytes = &buf[..n];
                            let wake = queue.push(bytes);
                            on_output(bytes, wake);
                        }
                        Err(_) => break,
                    }
                }
                let exit_code = child_guard
                    .0
                    .take()
                    .and_then(|mut c| c.wait().ok())
                    .map(|status| status.exit_code());
                on_exit(exit_code);
            })
            .context("Failed to spawn PTY reader thread")?;

        Ok(Self {
            output,
            master: pair.master,
            writer,
            killer,
            program_name,
            direct_program: !options.command.is_empty(),
            pid,
        })
    }

    /// Process id of the shell, when the platform reports one.
    pub(crate) fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub(crate) fn program_name(&self) -> Option<&str> {
        self.program_name.as_deref()
    }

    pub(crate) fn has_foreground_program(&self) -> bool {
        self.direct_program || self.shell_has_foreground_job()
    }

    #[cfg(unix)]
    fn shell_has_foreground_job(&self) -> bool {
        match (self.pid, self.master.process_group_leader()) {
            (Some(shell), Some(group)) => group > 0 && group as u32 != shell,
            _ => false,
        }
    }

    #[cfg(not(unix))]
    fn shell_has_foreground_job(&self) -> bool {
        false
    }

    /// The shell's current working directory, so a new tab or split can open in it.
    /// Only known on Linux (through `/proc`).
    pub fn cwd(&self) -> Option<PathBuf> {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_link(format!("/proc/{}/cwd", self.pid?)).ok()
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }

    /// Output read from the shell since the last call.
    pub(crate) fn take_output(&self) -> Vec<u8> {
        self.output.take()
    }

    pub fn write(&self, data: &[u8]) -> Result<()> {
        match self.writer.try_send(data.to_vec()) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(anyhow::anyhow!("PTY write queue full")),
            Err(TrySendError::Disconnected(_)) => Err(anyhow::anyhow!("PTY writer closed")),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to resize PTY")?;
        Ok(())
    }
}

/// The user's shell and its arguments: `TWITTY_SHELL` (split at spaces), else `$SHELL`, else
/// bash (PowerShell or cmd on Windows).
pub fn default_shell() -> (String, Vec<String>) {
    if let Ok(shell) = env::var("TWITTY_SHELL") {
        let mut parts = shell.split_whitespace().map(str::to_string);
        if let Some(program) = parts.next() {
            return (program, parts.collect());
        }
    }

    #[cfg(windows)]
    {
        // Prefer PowerShell 7, then Windows PowerShell; cmd.exe is only the last resort.
        for candidate in ["pwsh.exe", "powershell.exe"] {
            if find_in_path(candidate).is_some() {
                return (candidate.to_string(), vec!["-NoLogo".to_string()]);
            }
        }
        if let Ok(comspec) = env::var("COMSPEC") {
            if !comspec.trim().is_empty() {
                return (comspec, vec![]);
            }
        }
        ("cmd.exe".to_string(), vec![])
    }

    #[cfg(not(windows))]
    {
        if let Ok(shell) = env::var("SHELL") {
            if !shell.trim().is_empty() {
                return (shell, vec![]);
            }
        }
        ("/bin/bash".to_string(), vec![])
    }
}

#[cfg(windows)]
fn find_in_path(program: &str) -> Option<std::path::PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|path| path.is_file())
}
