//! Event-loop side of the control socket: starts [`ControlServer`], exposes its socket path to
//! pane factories, and implements [`ControlHost`] so incoming [`Request`]s (both Twitty `ctl`
//! and cmux v2/v1) dispatch cleanly against [`App`] state.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Instant;

use serde_json::{json, Value};

use super::{App, AppEvent};
use crate::commands;
use crate::control::cmux::{dispatch_if_active, ControlHost};
use crate::control::{ControlServer, Request, Response, REQUEST_TIMEOUT};
use crate::input::InputAction;
use crate::layout::{PaneId, SplitDir};
use crate::pty::SpawnOptions;
use crate::session::workspace::{SplitFailure, Workspace};
use crate::ui::notifications::NotificationHistory;
use crate::workflows::WorkflowSet;

impl App {
    /// Start the control socket server if enabled in config.
    pub(super) fn start_control_server(&mut self) {
        if !self.config.control_socket || self.control.is_some() {
            return;
        }
        let proxy = self.proxy.clone();
        let handler = Arc::new(move |req: Request| -> Response {
            let id = req.id.clone();
            let (tx, rx) = mpsc::channel();
            let cancelled = Arc::new(AtomicBool::new(false));
            if proxy
                .send_event(AppEvent::Control(req, tx, Arc::clone(&cancelled)))
                .is_err()
            {
                return Response::err(id, "application event loop closed");
            }
            match rx.recv_timeout(REQUEST_TIMEOUT) {
                Ok(resp) => resp,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    cancelled.store(true, Ordering::Release);
                    Response::err(id, "request timed out")
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    cancelled.store(true, Ordering::Release);
                    Response::err(id, "application closed before replying")
                }
            }
        });
        match ControlServer::start(handler) {
            Ok(server) => {
                if let Some(path) = server.path() {
                    tracing::info!("control socket listening at {}", path.display());
                }
                self.control = Some(server);
            }
            Err(err) => {
                tracing::warn!("failed to start control socket: {err}");
            }
        }
    }

    /// Stop the control socket server if currently running.
    pub(super) fn stop_control_server(&mut self) {
        self.control = None;
    }

    /// Path of the running control socket, if enabled and started.
    pub(super) fn control_socket_path(&self) -> Option<String> {
        self.control
            .as_ref()
            .and_then(ControlServer::path)
            .map(|p| p.to_string_lossy().into_owned())
    }

    /// Handle one request delivered from a control client thread and send the reply.
    /// Returns true when the command asked the window to close (e.g. closing the last pane).
    pub(super) fn on_control_request(
        &mut self,
        req: Request,
        reply: mpsc::Sender<Response>,
        cancelled: Arc<AtomicBool>,
    ) -> bool {
        let Some((response, should_exit)) = dispatch_if_active(self, req, &cancelled) else {
            return false;
        };
        let _ = reply.send(response);
        should_exit
    }
}

impl ControlHost for App {
    fn ws(&self) -> &Workspace {
        &self.ws
    }

    fn ws_mut(&mut self) -> &mut Workspace {
        &mut self.ws
    }

    fn notification_history(&self) -> &NotificationHistory {
        &self.notification_history
    }

    fn notification_history_mut(&mut self) -> &mut NotificationHistory {
        &mut self.notification_history
    }

    fn pane_cwd(&self, id: PaneId) -> Option<PathBuf> {
        App::pane_cwd(self, id)
    }

    fn sidebar_visible(&self) -> bool {
        self.window.sidebar_visible()
    }

    fn set_sidebar_visible(&mut self, visible: bool) {
        self.set_sidebar_shown(visible);
    }

    fn toggle_sidebar(&mut self) -> bool {
        App::toggle_sidebar(self);
        self.window.sidebar_visible()
    }

    fn new_tab_with(&mut self, options: SpawnOptions) -> Option<PaneId> {
        App::new_tab_with(self, options)
    }

    fn split_focused_with(
        &mut self,
        dir: SplitDir,
        options: SpawnOptions,
    ) -> Result<PaneId, SplitFailure> {
        App::split_focused_with(self, dir, options)
    }

    fn focus_pane_by_id(&mut self, id: PaneId) -> Result<usize, String> {
        App::focus_pane_by_id(self, id)
    }

    fn set_active_tab(&mut self, index: usize) {
        App::set_active_tab(self, index);
    }

    fn close_pane(&mut self, id: PaneId) -> bool {
        App::close_pane(self, id)
    }

    fn close_tab(&mut self, index: usize) -> bool {
        App::close_tab(self, index)
    }

    fn send_input(&mut self, id: PaneId, bytes: &[u8]) {
        App::send_input(self, id, bytes);
        if self.focused_id() == Some(id) {
            self.cursor_blink.reset(Instant::now());
        }
    }

    fn notify(&mut self, id: PaneId, title: Option<String>, body: String, send_desktop: bool) {
        self.notify_with_desktop(id, title, body, send_desktop);
    }

    fn request_redraw(&mut self) {
        self.window.request_redraw();
    }

    fn refresh_title(&mut self) {
        self.refresh_window_title();
    }

    fn new_ssh_tab(&mut self, destination: &str, name: Option<&str>) -> Result<Value, String> {
        let target = crate::remote::Target::for_tab(destination, name)?;
        let name = target.name().to_owned();
        let id = App::new_ssh_tab(self, target).ok_or("failed to open SSH tab")?;
        Ok(json!({ "pane": id.0, "tab": self.ws.active(), "name": name }))
    }

    fn command_list(&self) -> Value {
        let cwd = self.focused_id().and_then(|id| App::pane_cwd(self, id));
        let set = WorkflowSet::load(&self.config.commands, cwd.as_deref());
        Value::Array(set.control_list_json_with(&self.keybindings))
    }

    fn command_run(&mut self, name: &str, pane: Option<u64>) -> Result<(Value, bool), String> {
        if let Some(action) = commands::action(name) {
            let target = match pane {
                Some(p) => {
                    let id = PaneId(p);
                    App::focus_pane_by_id(self, id)?;
                    id
                }
                None => self
                    .focused_id()
                    .ok_or_else(|| "no focused pane".to_owned())?,
            };
            let should_exit = match action {
                InputAction::SplitRight => {
                    self.split_focused(SplitDir::Horizontal)
                        .map_err(|err| err.message)?;
                    false
                }
                InputAction::SplitDown => {
                    self.split_focused(SplitDir::Vertical)
                        .map_err(|err| err.message)?;
                    false
                }
                other => self.run_action(target, other),
            };
            return Ok((json!({ "command": name, "pane": target.0 }), should_exit));
        }
        let target = match pane {
            Some(p) => {
                let id = PaneId(p);
                if self.ws.pane(id).is_some() {
                    id
                } else {
                    return Err(format!("unknown pane: {p}"));
                }
            }
            None => self
                .focused_id()
                .ok_or_else(|| "no focused pane".to_owned())?,
        };
        let cwd = App::pane_cwd(self, target);
        let set = WorkflowSet::load(&self.config.commands, cwd.as_deref());
        let workflow = set
            .resolve_control_run(name)
            .map_err(|err| err.to_string())?;
        if pane.is_some() {
            App::focus_pane_by_id(self, target)?;
        }
        self.run_workflow(target, &workflow)?;
        Ok((json!({ "command": name, "pane": target.0 }), false))
    }
}
