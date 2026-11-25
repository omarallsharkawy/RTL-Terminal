//! Route SSH process and timer events to the pane's reconnect state.

use std::time::Instant;

use super::App;
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
        if let Some(delay) = delay {
            remote.schedule_retry(Instant::now(), delay);
        }
        let destination = clean_text(remote.target.destination(), 256);
        let status = match (lost, delay) {
            (_, Some(_)) => format!("Disconnected from {destination}, reconnecting… / اتقطع الاتصال…"),
            (true, None) => format!("Disconnected from {destination}. Press Enter to retry / اضغط Enter للمحاولة تاني"),
            (false, None) => format!("Could not attach to {destination}. Press Enter to retry / اضغط Enter للمحاولة تاني"),
        };
        pane.terminal
            .process_bytes(format!("\x18\x1b[0m\r\n{status}\r\n").as_bytes());
        self.window.request_redraw();
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

    fn retry_ssh_generation(&mut self, id: PaneId, generation: u64) {
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        if pane
            .remote
            .as_ref()
            .is_some_and(|remote| remote.may_retry(generation))
        {
            start_remote_pty(&self.proxy, pane);
            self.window.request_redraw();
        }
    }

    /// Trigger any SSH reconnect retries whose deadline has arrived at `now`, returning the
    /// earliest remaining deadline for `ControlFlow::WaitUntil`.
    pub(super) fn poll_ssh_retries(&mut self, now: Instant) -> Option<Instant> {
        let due: Vec<PaneId> = self
            .ws
            .panes()
            .filter(|pane| {
                pane.remote
                    .as_ref()
                    .and_then(|r| r.next_retry_deadline())
                    .is_some_and(|at| now >= at)
            })
            .map(|pane| pane.id)
            .collect();
        for id in due {
            let generation = self
                .ws
                .pane_mut(id)
                .and_then(|pane| pane.remote.as_mut())
                .and_then(|remote| remote.take_due_retry(now));
            if let Some(generation) = generation {
                self.retry_ssh_generation(id, generation);
            }
        }
        self.ws
            .panes()
            .filter_map(|pane| pane.remote.as_ref()?.next_retry_deadline())
            .min()
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
