//! Starting real shells: a terminal whose events and PTY output go to the event loop.

use std::path::PathBuf;
use winit::event_loop::EventLoopProxy;

use crate::app::AppEvent;
use crate::layout::PaneId;
use crate::pty::{Pty, SpawnOptions};
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
}

impl<'a> ShellFactory<'a> {
    pub fn new(proxy: &'a EventLoopProxy<AppEvent>, cursor_blink: bool) -> Self {
        Self {
            proxy,
            cursor_blink,
            options: SpawnOptions::default(),
            shell_integration: false,
        }
    }

    /// Load the shell integration into the shells this factory starts (config
    /// `shell_integration`).
    pub fn with_shell_integration(mut self, on: bool) -> Self {
        self.shell_integration = on;
        self
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
        let proxy = self.proxy.clone();
        let mut terminal = Terminal::with_events(cols, rows, move |event| {
            let _ = proxy.send_event(AppEvent::Terminal(id, event));
        });
        terminal.set_default_cursor_blink(self.cursor_blink);
        let proxy = self.proxy.clone();
        let proxy_exit = self.proxy.clone();
        // New tabs and splits open in the directory of the pane that has focus.
        let mut options = self.options.clone();
        options.cwd = cwd.or(options.cwd);
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
                let _ = proxy_exit.send_event(AppEvent::PtyExit(id));
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
}
