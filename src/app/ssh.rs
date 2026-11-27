//! Route SSH process and timer events to the pane's reconnect state.

use crate::ui::chrome::t;
use std::time::Instant;
use twitty_core::language::fill;

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
        let template = match (lost, delay) {
            (_, Some(_)) => t("Disconnected from {}, reconnecting…"),
            (true, None) => t("Disconnected from {}. Press Enter to retry"),
            (false, None) => t("Could not connect to {}. Press Enter to retry"),
        };
        let status = fill(template, &[&destination]);
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

    /// Force an immediate SSH reconnect on `id`, cancelling any retry backoff or stalled PTY.
    pub(super) fn reconnect_ssh_pane(&mut self, id: PaneId) {
        let Some(pane) = self.ws.pane_mut(id) else {
            return;
        };
        let Some(remote) = pane.remote.as_mut() else {
            return;
        };
        pane.pty = None;
        remote.reset_for_manual_reconnect();
        start_remote_pty(&self.proxy, pane);
        self.window.request_redraw();
    }

    /// Reconnect the SSH pane in sidebar tab `index` (focused remote pane first).
    pub(super) fn reconnect_ssh_tab(&mut self, index: usize) {
        let Some(tab) = self.ws.tabs().get(index) else {
            return;
        };
        let target_id = std::iter::once(tab.focused)
            .chain(tab.root.leaves())
            .find(|&id| self.ws.pane(id).is_some_and(|p| p.remote.is_some()));
        if let Some(id) = target_id {
            self.reconnect_ssh_pane(id);
        }
    }

    /// Load `~/.ssh/config` hosts on a background thread for the SSH connection palette.
    pub(super) fn spawn_ssh_hosts_load(&self) {
        let proxy = self.proxy.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("twitty-ssh-hosts".into())
            .spawn(move || {
                let hosts = crate::composer::generators::ssh_hosts_from_default();
                let _ = proxy.send_event(super::event::AppEvent::SshHostsLoaded(hosts));
            })
        {
            tracing::warn!("twitty ssh hosts thread failed to start: {err}");
        }
    }

    pub(super) fn on_ssh_hosts_loaded(
        &mut self,
        hosts: Vec<crate::composer::generators::GeneratedItem>,
    ) {
        self.ssh_hosts = hosts;
        if self.ssh_palette_open() {
            self.sync_palette();
        }
    }

    fn record_recent_ssh(&mut self, destination: &str) {
        let dest = destination.trim();
        if dest.is_empty() {
            return;
        }
        self.recent_ssh.retain(|existing| existing != dest);
        self.recent_ssh.insert(0, dest.to_owned());
        self.recent_ssh.truncate(16);
    }

    /// Parse user SSH input (`host`, `user@host`, or `user@host:session`) and open it either in a
    /// new persistent SSH tab or as a split in the active tab.
    pub(super) fn connect_ssh_input(&mut self, raw: &str, split: bool) {
        let target = match crate::remote::parse_ssh_input(raw) {
            Ok(target) => target,
            Err(err) => {
                self.toast(crate::ui::toast::ToastKind::Error, &err);
                return;
            }
        };
        let dest = target.destination().to_owned();
        let opened = if split {
            if self.can_split_focused(crate::layout::SplitDir::Horizontal) {
                self.split_ssh_focused(crate::layout::SplitDir::Horizontal, target)
                    .ok()
            } else if self.can_split_focused(crate::layout::SplitDir::Vertical) {
                self.split_ssh_focused(crate::layout::SplitDir::Vertical, target)
                    .ok()
            } else {
                self.new_ssh_tab(target)
            }
        } else {
            self.new_ssh_tab(target)
        };
        if opened.is_some() {
            self.record_recent_ssh(&dest);
        } else {
            let msg = fill(t("Could not connect to {}. Press Enter to retry"), &[&dest]);
            self.toast(crate::ui::toast::ToastKind::Error, &msg);
        }
    }

    /// Open a new split connected to the same SSH host as pane `id` (fresh remote session).
    pub(super) fn split_ssh_same_host(&mut self, id: PaneId) {
        let Some(dest) = self
            .ws
            .pane(id)
            .and_then(|pane| pane.remote.as_ref())
            .map(|remote| remote.target.destination().to_owned())
        else {
            return;
        };
        self.connect_ssh_input(&dest, true);
    }
}
