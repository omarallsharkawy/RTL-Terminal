//! Starting real shells: a terminal whose events and PTY output go to the event loop.

use std::path::PathBuf;
use winit::event_loop::EventLoopProxy;

use crate::app::AppEvent;
use crate::layout::PaneId;
use crate::pty::Pty;
use crate::session::pane::Pane;
use crate::session::workspace::PaneFactory;
use crate::terminal::Terminal;

pub struct ShellFactory<'a> {
    proxy: &'a EventLoopProxy<AppEvent>,
    /// Blink the cursor when the program has not chosen a style.
    cursor_blink: bool,
}

impl<'a> ShellFactory<'a> {
    pub fn new(proxy: &'a EventLoopProxy<AppEvent>, cursor_blink: bool) -> Self {
        Self {
            proxy,
            cursor_blink,
        }
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
        let pty = match Pty::spawn_in(
            cwd.as_deref(),
            cols as u16,
            rows as u16,
            move |data| {
                let _ = proxy.send_event(AppEvent::PtyData(id, data));
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
