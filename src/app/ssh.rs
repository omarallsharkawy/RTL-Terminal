//! Route SSH process and timer events to the pane's reconnect state.

use super::{App, AppEvent};
use crate::layout::PaneId;
use crate::remote::{ConnectionState, BOOTSTRAP_FAILED, SSH_DISCONNECTED};
use crate::session::spawn::start_remote_pty;
use crate::shell_integration::clean_text;

impl App {
    pub(super) fn on_ssh_attached(&mut self, id: PaneId) {
        if let Some(remote) = self.ws.pane_mut(id).and_then(|pane| pane.remote.as_mut()) {
            remote.connected();
            self.window.request_redraw();
        }
    }

    /// Return true when the pane stays open after its process exits.
    pub(super) fn on_ssh_exit(&mut self, id: PaneId, code: Option<u32>) -> bool {
        if !matches!(code, Some(SSH_DISCONNECTED | BOOTSTRAP_FAILED)) {
            return false;
        }
        self.on_pty_output(id);
        let Some(pane) = self.ws.pane_mut(id) else {
            return false;
        };
        let Some(remote) = &mut pane.remote else {
            return false;
        };
        pane.pty = None;

        let lost = code == Some(SSH_DISCONNECTED);
        let delay = remote.disconnected(lost);
        let destination = clean_text(remote.target.destination(), 256);
        let status = match (lost, delay) {
            (_, Some(_)) => format!("Disconnected from {destination}, reconnecting… / اتقطع الاتصال…"),
            (true, None) => format!("Disconnected from {destination}. Press Enter to retry / اضغط Enter للمحاولة تاني"),
            (false, None) => format!("Could not attach to {destination}. Press Enter to retry / اضغط Enter للمحاولة تاني"),
        };
        pane.terminal
            .process_bytes(format!("\x18\x1b[0m\r\n{status}\r\n").as_bytes());
        self.window.request_redraw();
        if let Some(delay) = delay {
            let proxy = self.proxy.clone();
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                let _ = proxy.send_event(AppEvent::RetrySsh(id));
            });
        }
        true
    }

    pub(super) fn retry_ssh(&mut self, id: PaneId) {
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        if pane
            .remote
            .as_ref()
            .is_some_and(|remote| remote.state == ConnectionState::Disconnected)
        {
            start_remote_pty(&self.proxy, pane);
            self.window.request_redraw();
        }
    }

    pub(super) fn manual_ssh_retry(&mut self, id: PaneId) -> bool {
        let stopped = self
            .ws
            .pane(id)
            .and_then(|pane| pane.remote.as_ref())
            .is_some_and(|remote| remote.waiting_for_enter);
        if stopped {
            self.retry_ssh(id);
        }
        stopped
    }
}
