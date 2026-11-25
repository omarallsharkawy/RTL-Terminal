//! The listening-ports scanner behind the sidebar: started the first time the sidebar is
//! drawn, told which shells to watch on every frame (a no-op while nothing changed), and
//! paused while the sidebar is hidden. Results come back as `AppEvent::Ports`.

use std::collections::HashMap;

use super::{App, AppEvent};
use crate::layout::PaneId;
use crate::ports::{PortWatcher, SystemProc, SCAN_INTERVAL};

#[derive(Default)]
pub(super) enum PortWatch {
    /// Not started yet.
    #[default]
    Idle,
    Running(PortWatcher<PaneId>),
    /// The platform has no `/proc`, or the thread could not be started.
    Unavailable,
}

impl App {
    /// Watch the shells of all panes while the sidebar is shown; pause otherwise.
    pub(super) fn sync_port_watch(&mut self) {
        let shown = self.window.sidebar_shown(&self.ws);
        if !shown && !matches!(self.ports, PortWatch::Running(_)) {
            return;
        }
        if matches!(self.ports, PortWatch::Idle) {
            self.ports = self.start_port_watch();
        }
        let PortWatch::Running(watcher) = &mut self.ports else {
            return;
        };
        let roots = if shown {
            let mut roots: Vec<(PaneId, u32)> = self
                .ws
                .panes()
                .filter(|pane| pane.remote.is_none())
                .filter_map(|p| Some((p.id, p.pty.as_ref()?.pid()?)))
                .collect();
            roots.sort_unstable();
            roots
        } else {
            Vec::new()
        };
        watcher.watch(roots);
    }

    fn start_port_watch(&self) -> PortWatch {
        if !cfg!(target_os = "linux") {
            return PortWatch::Unavailable;
        }
        let proxy = self.proxy.clone();
        match PortWatcher::spawn(SystemProc, SCAN_INTERVAL, move |ports| {
            proxy.send_event(AppEvent::Ports(ports)).is_ok()
        }) {
            Ok(watcher) => PortWatch::Running(watcher),
            Err(e) => {
                tracing::warn!("port scanner not started: {e}");
                PortWatch::Unavailable
            }
        }
    }

    /// A port scan finished; redraw when the sidebar changes.
    pub(super) fn on_ports(&mut self, ports: HashMap<PaneId, Vec<u16>>) {
        if self.window.set_ports(ports) {
            self.window.request_redraw();
        }
    }
}
