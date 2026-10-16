use std::env;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};

/// Variables a parent terminal (or tmux) sets for its own shells. Inherited by a Twitty
/// started from inside one of them, they make programs talk that terminal's protocols
/// (kitty graphics, iTerm2 escapes) or believe they run inside tmux.
pub const INHERITED_TERMINAL_VARS: &[&str] = &[
    "ALACRITTY_LOG",
    "ALACRITTY_SOCKET",
    "ALACRITTY_WINDOW_ID",
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
    "VTE_VERSION",
    "WEZTERM_EXECUTABLE",
    "WEZTERM_PANE",
    "WEZTERM_UNIX_SOCKET",
    "WT_PROFILE_ID",
    "WT_SESSION",
];

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
    /// Writes go through a dedicated thread so a large paste never blocks the UI.
    writer: Sender<Vec<u8>>,
    /// Ends the shell when the pane is closed (the reader thread keeps the PTY open otherwise).
    killer: Box<dyn ChildKiller + Send + Sync>,
    /// Process id of the shell, to find its working directory.
    pid: Option<u32>,
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.output.close();
        let _ = self.killer.kill();
    }
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
        mut on_output: F,
        mut on_exit: E,
    ) -> Result<Self>
    where
        F: FnMut() + Send + 'static,
        E: FnMut() + Send + 'static,
    {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY pair")?;

        let (shell_cmd, shell_args) = default_shell();
        let mut cmd = CommandBuilder::new(shell_cmd);
        for arg in shell_args {
            cmd.arg(arg);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Twitty");
        cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
        for key in INHERITED_TERMINAL_VARS {
            cmd.env_remove(key);
        }
        if env::var_os("LANG").is_none() && env::var_os("LC_ALL").is_none() {
            cmd.env("LANG", "en_US.UTF-8");
        }

        if let Some(dir) = cwd.filter(|d| d.is_dir()) {
            cmd.cwd(dir);
        }

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
        let (writer, write_rx) = channel::<Vec<u8>>();
        thread::spawn(move || {
            for data in write_rx {
                if pty_writer
                    .write_all(&data)
                    .and_then(|_| pty_writer.flush())
                    .is_err()
                {
                    break;
                }
            }
        });

        let output = OutputQueue::default();
        let queue = output.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 64 * 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if queue.push(&buf[..n]) {
                            on_output();
                        }
                    }
                    Err(_) => break,
                }
            }
            let _ = child.wait();
            on_exit();
        });

        Ok(Self {
            output,
            master: pair.master,
            writer,
            killer,
            pid,
        })
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
    pub fn take_output(&self) -> Vec<u8> {
        self.output.take()
    }

    pub fn write(&self, data: &[u8]) -> Result<()> {
        self.writer
            .send(data.to_vec())
            .map_err(|_| anyhow::anyhow!("PTY writer closed"))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to resize PTY")?;
        Ok(())
    }
}

fn default_shell() -> (String, Vec<String>) {
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
