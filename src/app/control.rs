//! Event-loop side of the control socket: starts [`ControlServer`], attaches `TWITTY_SOCKET`
//! to new pane environments, and dispatches incoming [`Request`]s against [`App`] state.

use std::sync::{mpsc, Arc};
use std::time::Instant;

use serde_json::{json, Value};

use super::{App, AppEvent};
use crate::commands;
use crate::control::protocol::{bound_read_text, MAX_READ_BYTES};
use crate::control::{ControlServer, MethodCall, Request, Response, REQUEST_TIMEOUT};
use crate::layout::PaneId;
use crate::pty::{Pty, SpawnOptions};
use crate::remote::Target;
use crate::shell_integration::clean_text;
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
            if proxy.send_event(AppEvent::Control(req, tx)).is_err() {
                return Response::err(id, "application event loop closed");
            }
            match rx.recv_timeout(REQUEST_TIMEOUT) {
                Ok(resp) => resp,
                Err(mpsc::RecvTimeoutError::Timeout) => Response::err(id, "request timed out"),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
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

    /// Attach `TWITTY_SOCKET` to `options` when the control server is running.
    pub(super) fn attach_control_env(&self, options: &mut SpawnOptions) {
        if let Some(path) = self.control.as_ref().and_then(ControlServer::path) {
            options
                .env
                .push(("TWITTY_SOCKET".into(), path.to_string_lossy().into_owned()));
        }
    }

    /// Handle one request delivered from a control client thread and send the reply.
    /// Returns true when the command asked the window to close (e.g. closing the last pane).
    pub(super) fn on_control_request(
        &mut self,
        req: Request,
        reply: mpsc::Sender<Response>,
    ) -> bool {
        let id = req.id.clone();
        let (response, should_exit) = match MethodCall::parse(&req) {
            Ok(call) => match self.dispatch_control(call) {
                Ok((result, exit)) => (Response::ok(id, result), exit),
                Err(err) => (Response::err(id, err), false),
            },
            Err(err) => (Response::err(id, err), false),
        };
        let _ = reply.send(response);
        should_exit
    }

    fn dispatch_control(&mut self, call: MethodCall) -> Result<(Value, bool), String> {
        match call {
            MethodCall::SystemPing => Ok((
                json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "pid": std::process::id(),
                }),
                false,
            )),
            MethodCall::CommandList => {
                let cwd = self.focused_id().and_then(|id| self.pane_cwd(id));
                let set = WorkflowSet::load(&self.config.commands, cwd.as_deref());
                Ok((Value::Array(set.control_list_json()), false))
            }
            MethodCall::CommandRun { name, pane } => {
                if let Some(action) = commands::action(&name) {
                    let target = match pane {
                        Some(p) => {
                            let id = PaneId(p);
                            self.focus_pane_by_id(id)?;
                            id
                        }
                        None => self
                            .focused_id()
                            .ok_or_else(|| "no focused pane".to_owned())?,
                    };
                    let should_exit = self.run_action(target, action);
                    return Ok((json!({ "command": name, "pane": target.0 }), should_exit));
                }
                let target = self.resolve_pane(pane)?;
                let cwd = self.pane_cwd(target);
                let set = WorkflowSet::load(&self.config.commands, cwd.as_deref());
                let workflow = set
                    .resolve_control_run(&name)
                    .map_err(|err| err.to_string())?;
                if pane.is_some() {
                    self.focus_pane_by_id(target)?;
                }
                self.run_workflow(target, &workflow)?;
                Ok((json!({ "command": name, "pane": target.0 }), false))
            }
            MethodCall::TabList => {
                let active = self.ws.active();
                let list: Vec<Value> = self
                    .ws
                    .tabs()
                    .iter()
                    .enumerate()
                    .map(|(index, tab)| {
                        let panes: Vec<u64> = tab.root.leaves().into_iter().map(|p| p.0).collect();
                        json!({
                            "index": index,
                            "title": clean_text(&self.ws.tab_title(index), 256),
                            "active": index == active,
                            "panes": panes,
                        })
                    })
                    .collect();
                Ok((Value::Array(list), false))
            }
            MethodCall::PaneList => {
                let focused = self.focused_id();
                let mut list = Vec::new();
                for (tab_index, tab) in self.ws.tabs().iter().enumerate() {
                    for id in tab.root.leaves() {
                        let Some(pane) = self.ws.pane(id) else {
                            continue;
                        };
                        let cwd = pane
                            .meta
                            .cwd
                            .clone()
                            .or_else(|| pane.pty.as_ref().and_then(Pty::cwd))
                            .map(|p| clean_text(&p.to_string_lossy(), 4096))
                            .filter(|s| !s.is_empty());
                        let title = pane
                            .title
                            .as_deref()
                            .map(|t| clean_text(t, 256))
                            .filter(|t| !t.is_empty());
                        let attention = pane.meta.attention.as_ref().map(|n| {
                            json!({
                                "title": n.title,
                                "body": n.body,
                            })
                        });
                        list.push(json!({
                            "id": pane.id.0,
                            "tab": tab_index,
                            "focused": focused == Some(pane.id),
                            "title": title,
                            "cwd": cwd,
                            "running": pane.meta.running,
                            "last_exit": pane.meta.last_exit,
                            "attention": attention,
                            "size": {
                                "cols": pane.size.0,
                                "rows": pane.size.1,
                            },
                        }));
                    }
                }
                Ok((Value::Array(list), false))
            }
            MethodCall::PaneFocus { pane } => {
                let tab = self.focus_pane_by_id(PaneId(pane))?;
                Ok((json!({ "pane": pane, "tab": tab }), false))
            }
            MethodCall::PaneSendText { pane, text } => {
                let target = self.resolve_pane(pane)?;
                self.send_input(target, text.as_bytes());
                if self.focused_id() == Some(target) {
                    self.cursor_blink.reset(Instant::now());
                }
                Ok((json!({ "pane": target.0, "bytes": text.len() }), false))
            }
            MethodCall::PaneRead { pane, lines } => {
                let target = self.resolve_pane(pane)?;
                let text = self
                    .ws
                    .pane(target)
                    .map(|p| bound_read_text(&p.terminal.recent_lines(lines), MAX_READ_BYTES))
                    .ok_or_else(|| format!("unknown pane: {}", target.0))?;
                Ok((json!({ "pane": target.0, "text": text }), false))
            }
            MethodCall::TabNew { cwd, command } => {
                if let Some(ref dir) = cwd {
                    if !dir.is_dir() {
                        return Err(format!(
                            "cwd is not an existing directory: {}",
                            dir.display()
                        ));
                    }
                }
                let options = SpawnOptions {
                    cwd,
                    command,
                    env: Vec::new(),
                };
                let id = self
                    .new_tab_with(options)
                    .ok_or_else(|| "failed to open new tab".to_owned())?;
                Ok((json!({ "pane": id.0, "tab": self.ws.active() }), false))
            }
            MethodCall::TabNewSsh { destination, name } => {
                let target = Target::for_tab(&destination, name.as_deref())?;
                let name = target.name().to_owned();
                let id = self.new_ssh_tab(target).ok_or("failed to open SSH tab")?;
                Ok((
                    json!({ "pane": id.0, "tab": self.ws.active(), "name": name }),
                    false,
                ))
            }
            MethodCall::PaneSplit {
                pane,
                direction,
                cwd,
                command,
            } => {
                if let Some(ref dir) = cwd {
                    if !dir.is_dir() {
                        return Err(format!(
                            "cwd is not an existing directory: {}",
                            dir.display()
                        ));
                    }
                }
                match pane {
                    Some(p) => {
                        self.focus_pane_by_id(PaneId(p))?;
                    }
                    None => {
                        if self.focused_id().is_none() {
                            return Err("no focused pane".into());
                        }
                    }
                }
                let options = SpawnOptions {
                    cwd,
                    command,
                    env: Vec::new(),
                };
                let id = self
                    .split_focused_with(direction, options)
                    .ok_or_else(|| "could not split pane".to_owned())?;
                Ok((json!({ "pane": id.0, "tab": self.ws.active() }), false))
            }
            MethodCall::Notify { pane, title, body } => {
                let target = self.resolve_pane(pane)?;
                self.notify(target, title, body);
                Ok((json!({ "pane": target.0 }), false))
            }
        }
    }

    fn resolve_pane(&self, pane: Option<u64>) -> Result<PaneId, String> {
        match pane {
            Some(p) => {
                let id = PaneId(p);
                if self.ws.pane(id).is_some() {
                    Ok(id)
                } else {
                    Err(format!("unknown pane: {p}"))
                }
            }
            None => self
                .focused_id()
                .ok_or_else(|| "no focused pane".to_owned()),
        }
    }
}
