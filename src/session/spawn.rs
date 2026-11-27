//! Starting real shells: a terminal whose events and PTY output go to the event loop.

use std::path::PathBuf;

use anyhow::{Context, Result};
use winit::event_loop::EventLoopProxy;

use crate::app::AppEvent;
use crate::layout::PaneId;
use crate::pty::{Pty, SpawnOptions};
use crate::remote::{AttachMarker, RemotePane, Target, PANE_WORKER};
use crate::session::pane::Pane;
use crate::session::shell_setup;
use crate::session::workspace::PaneFactory;
use crate::terminal::Terminal;

/// Build the cmux-compatible environment variables exposed to a pane when `cmux_compat` and
/// `control_socket` are both enabled.
pub fn cmux_env_vars(
    socket_path: &str,
    workspace_id: u64,
    surface_id: u64,
) -> [(String, String); 3] {
    [
        ("CMUX_SOCKET_PATH".into(), socket_path.to_owned()),
        ("CMUX_WORKSPACE_ID".into(), workspace_id.to_string()),
        ("CMUX_SURFACE_ID".into(), surface_id.to_string()),
    ]
}

pub(crate) struct ShellFactory<'a> {
    proxy: &'a EventLoopProxy<AppEvent>,
    /// Blink the cursor when the program has not chosen a style.
    cursor_blink: bool,
    /// Program to run instead of the shell, and extra environment, for the next spawn.
    pub options: SpawnOptions,
    /// Load Twitty's shell integration into bash, zsh and fish (`shell_setup`).
    shell_integration: bool,
    /// Control socket path when the control server is running.
    control_socket: Option<String>,
    /// Expose `CMUX_SOCKET_PATH`, `CMUX_WORKSPACE_ID`, `CMUX_SURFACE_ID`.
    cmux_compat: bool,
    /// Prepend Twitty's private `claude` wrapper to `PATH` inside Twitty shells.
    claude_hooks: bool,
    /// Stable tab id for the pane being spawned (set by `Workspace` via `PaneFactory::set_tab_id`).
    tab_id: u64,
    pub remote: Option<Target>,
}

impl<'a> ShellFactory<'a> {
    pub fn new(proxy: &'a EventLoopProxy<AppEvent>, cursor_blink: bool) -> Self {
        Self {
            proxy,
            cursor_blink,
            options: SpawnOptions::default(),
            shell_integration: false,
            control_socket: None,
            cmux_compat: false,
            claude_hooks: false,
            tab_id: 1,
            remote: None,
        }
    }

    /// Load the shell integration into the shells this factory starts (config
    /// `shell_integration`).
    pub fn with_shell_integration(mut self, on: bool) -> Self {
        self.shell_integration = on;
        self
    }

    /// Configure control socket path and cmux / Claude hook integration for spawned panes.
    pub fn with_control(
        mut self,
        socket_path: Option<String>,
        cmux_compat: bool,
        claude_hooks: bool,
    ) -> Self {
        self.control_socket = socket_path;
        self.cmux_compat = cmux_compat;
        self.claude_hooks = claude_hooks;
        self
    }

    fn terminal(&self, id: PaneId, cols: usize, rows: usize) -> Terminal {
        let proxy = self.proxy.clone();
        let mut terminal = Terminal::with_events(cols, rows, move |event| {
            let _ = proxy.send_event(AppEvent::Terminal(id, event));
        });
        terminal.set_default_cursor_blink(self.cursor_blink);
        terminal
    }
}

impl PaneFactory for ShellFactory<'_> {
    fn set_tab_id(&mut self, tab_id: u64) {
        self.tab_id = tab_id.max(1);
    }

    fn spawn(
        &mut self,
        id: PaneId,
        cols: usize,
        rows: usize,
        cwd: Option<PathBuf>,
    ) -> Option<Pane> {
        if let Some(target) = self.remote.take() {
            return self.spawn_remote(id, cols, rows, target);
        }
        let terminal = self.terminal(id, cols, rows);
        let proxy = self.proxy.clone();
        let proxy_exit = self.proxy.clone();
        // Explicit cwd overrides the focused pane's directory; otherwise inherit it.
        let mut options = self.options.clone();
        options.cwd = options.cwd.or(cwd);
        // Lets `twitty ctl` and agents running inside the pane address it.
        options
            .env
            .push(("TWITTY_PANE_ID".into(), id.0.to_string()));
        if let Some(ref socket) = self.control_socket {
            if !options.env.iter().any(|(k, _)| k == "TWITTY_SOCKET") {
                options.env.push(("TWITTY_SOCKET".into(), socket.clone()));
            }
            if self.cmux_compat {
                options.env.extend(cmux_env_vars(socket, self.tab_id, id.0));
            }
        }
        let is_user_shell = options.command.is_empty();
        if self.shell_integration {
            shell_setup::apply(&mut options);
        }
        if self.claude_hooks && self.control_socket.is_some() && is_user_shell {
            shell_setup::apply_claude_wrapper(&mut options);
        }
        let pty_cols = cols.clamp(1, u16::MAX as usize) as u16;
        let pty_rows = rows.clamp(1, u16::MAX as usize) as u16;
        let pty = match Pty::spawn_with(
            &options,
            pty_cols,
            pty_rows,
            move || {
                let _ = proxy.send_event(AppEvent::PtyOutput(id));
            },
            move || {
                let _ = proxy_exit.send_event(AppEvent::PtyExit(id, None));
            },
        ) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("failed to spawn PTY: {e:?}");
                return None;
            }
        };
        Some(Pane::new(id, terminal, Some(pty), (cols, rows)))
    }

    fn spawn_remote(
        &mut self,
        id: PaneId,
        cols: usize,
        rows: usize,
        target: Target,
    ) -> Option<Pane> {
        let mut pane = Pane::new(id, self.terminal(id, cols, rows), None, (cols, rows));
        pane.remote = Some(RemotePane::new(target));
        start_remote_pty(self.proxy, &mut pane);
        Some(pane)
    }
}

pub(crate) fn start_remote_pty(proxy: &EventLoopProxy<AppEvent>, pane: &mut Pane) {
    let Some(remote) = &mut pane.remote else {
        return;
    };
    remote.connecting();
    let (cols, rows) = pane.size;
    let pty_cols = cols.clamp(1, u16::MAX as usize) as u16;
    let pty_rows = rows.clamp(1, u16::MAX as usize) as u16;
    match remote_pty(proxy, pane.id, &remote.target, pty_cols, pty_rows) {
        Ok(pty) => pane.pty = Some(pty),
        Err(err) => {
            remote.disconnected(false);
            let error = crate::shell_integration::clean_text(&format!("{err:#}"), 512);
            let line = format!(
                "\r\nTwitty: {error}. {}\r\n",
                crate::ui::chrome::t("Press Enter to retry")
            );
            pane.terminal.process_bytes(line.as_bytes());
        }
    }
}

fn remote_pty(
    proxy: &EventLoopProxy<AppEvent>,
    id: PaneId,
    target: &Target,
    cols: u16,
    rows: u16,
) -> Result<Pty> {
    let executable = std::env::current_exe()?;
    let options = SpawnOptions {
        command: vec![
            executable
                .to_str()
                .context("Twitty's path is not UTF-8")?
                .to_string(),
            PANE_WORKER.into(),
            target.destination().into(),
            target.name().into(),
            cols.to_string(),
            rows.to_string(),
        ],
        ..SpawnOptions::default()
    };
    let output = proxy.clone();
    let exit = proxy.clone();
    let mut marker = AttachMarker::default();
    Pty::spawn_with_status(
        &options,
        cols,
        rows,
        move |bytes, wake| {
            if marker.push(bytes) {
                let _ = output.send_event(AppEvent::SshAttached(id));
            }
            if wake {
                let _ = output.send_event(AppEvent::PtyOutput(id));
            }
        },
        move |status| {
            let _ = exit.send_event(AppEvent::PtyExit(id, status));
        },
    )
}
