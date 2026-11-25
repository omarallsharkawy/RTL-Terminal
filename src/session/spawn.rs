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

pub(crate) struct ShellFactory<'a> {
    proxy: &'a EventLoopProxy<AppEvent>,
    /// Blink the cursor when the program has not chosen a style.
    cursor_blink: bool,
    /// Program to run instead of the shell, and extra environment, for the next spawn.
    pub options: SpawnOptions,
    /// Load Twitty's shell integration into bash, zsh and fish (`shell_setup`).
    shell_integration: bool,
    pub remote: Option<Target>,
}

impl<'a> ShellFactory<'a> {
    pub fn new(proxy: &'a EventLoopProxy<AppEvent>, cursor_blink: bool) -> Self {
        Self {
            proxy,
            cursor_blink,
            options: SpawnOptions::default(),
            shell_integration: false,
            remote: None,
        }
    }

    /// Load the shell integration into the shells this factory starts (config
    /// `shell_integration`).
    pub fn with_shell_integration(mut self, on: bool) -> Self {
        self.shell_integration = on;
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
        if self.shell_integration {
            shell_setup::apply(&mut options);
        }
        let pty = match Pty::spawn_with(
            &options,
            cols as u16,
            rows as u16,
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
    match remote_pty(proxy, pane.id, &remote.target, cols as u16, rows as u16) {
        Ok(pty) => pane.pty = Some(pty),
        Err(err) => {
            remote.disconnected(false);
            let error = crate::shell_integration::clean_text(&format!("{err:#}"), 512);
            let line =
                format!("\r\nTwitty: {error}. Press Enter to retry / اضغط Enter للمحاولة تاني\r\n");
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
