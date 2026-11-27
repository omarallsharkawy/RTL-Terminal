// Adapted from cmux-gtk cmux/src/socket/v2/{mod,system,workspace,pane,surface,notification,sidebar,helpers}.rs
// and cmux/src/socket/v1.rs (AGPL-3.0-or-later, Copyright (c) 2026 Douglas Soares de Andrade).
// Changes: mapped cmux v2 methods and minimal Claude-hook v1 text commands onto Twitty's
// Workspace/Pane/NotificationHistory model via ControlHost, kept Twitty's ctl methods intact,
// enforced Twitty text cleaning and size limits, and added a window-free HeadlessHost.

//! cmux-compatible v2 JSON-RPC and minimal v1 text control dispatch, mapped onto Twitty's tabs,
//! panes, notifications, and sidebar status.

mod handlers;
mod headless;
mod keys;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Map, Value};

use crate::control::protocol::{
    bound_read_text, MethodCall, Request, Response, MAX_METHOD_LEN, MAX_READ_BYTES,
};
use crate::layout::{PaneId, SplitDir};
use crate::pty::SpawnOptions;
use crate::session::workspace::{SplitFailure, Workspace};
use crate::shell_integration::clean_text;
use crate::ui::notifications::NotificationHistory;

use handlers::*;
pub use headless::HeadlessHost;
pub use keys::encode_key_name;
pub use twitty_control::protocol::parse_v1_line;

pub(super) const MAX_CWD_BYTES: usize = 4096;
pub(super) const MAX_TITLE_CHARS: usize = 256;

/// Every control method supported by Twitty (both Twitty `ctl` and cmux v2).
pub const SUPPORTED_METHODS: &[&str] = &[
    // System
    "system.ping",
    "system.identify",
    "system.capabilities",
    // Twitty native commands and tab/pane controls
    "command.list",
    "command.run",
    "tab.list",
    "tab.new",
    "tab.new-ssh",
    "pane.send-text",
    "pane.read",
    "notify",
    // cmux v2 workspaces (= Twitty tabs)
    "workspace.list",
    "workspace.current",
    "workspace.create",
    "workspace.new",
    "workspace.select",
    "workspace.close",
    "workspace.rename",
    "workspace.set_status",
    "workspace.clear_status",
    "workspace.list_status",
    // cmux v2 panes & surfaces (= Twitty panes)
    "pane.list",
    "pane.split",
    "pane.new",
    "pane.create",
    "pane.focus",
    "pane.close",
    "surface.list",
    "surface.current",
    "surface.split",
    "surface.focus",
    "surface.close",
    "surface.send_text",
    "surface.send_input",
    "surface.send_key",
    "surface.read_text",
    // cmux v2 notifications
    "notification.create",
    "notification.create_for_surface",
    "notification.create_for_target",
    "notification.list",
    "notification.clear",
    // cmux v2 sidebar visibility
    "sidebar.show",
    "sidebar.hide",
    "sidebar.toggle",
    "sidebar.status",
];

// ---------------------------------------------------------------------------
// ControlHost trait & unified dispatcher
// ---------------------------------------------------------------------------

/// Narrow interface between control protocol dispatch and the running application (or headless
/// test workspace).
pub(crate) trait ControlHost {
    fn ws(&self) -> &Workspace;
    fn ws_mut(&mut self) -> &mut Workspace;
    fn notification_history(&self) -> &NotificationHistory;
    fn notification_history_mut(&mut self) -> &mut NotificationHistory;
    fn pane_cwd(&self, id: PaneId) -> Option<PathBuf>;
    fn sidebar_visible(&self) -> bool;
    fn set_sidebar_visible(&mut self, visible: bool);
    fn toggle_sidebar(&mut self) -> bool;
    fn new_tab_with(&mut self, options: SpawnOptions) -> Option<PaneId>;
    fn split_focused_with(
        &mut self,
        dir: SplitDir,
        options: SpawnOptions,
    ) -> Result<PaneId, SplitFailure>;
    fn focus_pane_by_id(&mut self, id: PaneId) -> Result<usize, String>;
    fn set_active_tab(&mut self, index: usize);
    fn close_pane(&mut self, id: PaneId) -> bool;
    fn close_tab(&mut self, index: usize) -> bool;
    fn send_input(&mut self, id: PaneId, bytes: &[u8]);
    fn notify(&mut self, id: PaneId, title: Option<String>, body: String, send_desktop: bool);
    fn request_redraw(&mut self);
    fn refresh_title(&mut self);
    fn command_list(&self) -> Value;
    fn command_run(&mut self, name: &str, pane: Option<u64>) -> Result<(Value, bool), String>;
    /// Open a persistent SSH tab (`tab.new-ssh`); hosts without a window refuse.
    fn new_ssh_tab(&mut self, _destination: &str, _name: Option<&str>) -> Result<Value, String> {
        Err("SSH tabs are not available here".into())
    }
}

/// Dispatch `req` only if `cancelled` has not been set (e.g. when the control caller already
/// timed out before the event loop dequeued the request).
pub(crate) fn dispatch_if_active(
    host: &mut impl ControlHost,
    req: Request,
    cancelled: &AtomicBool,
) -> Option<(Response, bool)> {
    if cancelled.load(Ordering::Acquire) {
        return None;
    }
    Some(dispatch_request(host, req))
}

/// Dispatch one parsed control [`Request`] against `host`, handling both Twitty's native `ctl`
/// methods and the cmux v2 method set. Returns `(Response, should_exit)`.
pub(crate) fn dispatch_request(host: &mut impl ControlHost, req: Request) -> (Response, bool) {
    let id = req.id.clone();
    let empty = Map::new();
    let params = match &req.params {
        Value::Null => &empty,
        Value::Object(map) => map,
        _ => {
            return (
                Response::cmux_err(id, "invalid_params", "\"params\" must be a JSON object"),
                false,
            );
        }
    };

    match req.method.as_str() {
        // ---- Shared / System ----
        "system.ping" => (
            Response::ok(
                id,
                json!({
                    "pong": true,
                    "version": env!("CARGO_PKG_VERSION"),
                    "pid": std::process::id(),
                }),
            ),
            false,
        ),
        "system.identify" => (
            Response::ok(
                id,
                json!({
                    "app": "twitty",
                    "protocol": "cmux-v2",
                    "platform": std::env::consts::OS,
                    "version": env!("CARGO_PKG_VERSION"),
                    "pid": std::process::id(),
                }),
            ),
            false,
        ),
        "system.capabilities" => (
            Response::ok(id, json!({ "methods": SUPPORTED_METHODS })),
            false,
        ),

        // ---- Existing Twitty ctl methods (kept unchanged) ----
        "command.list" | "command.run" | "tab.list" | "tab.new" | "tab.new-ssh"
        | "pane.send-text" | "pane.read" | "notify" => dispatch_twitty_native(host, &req),

        // ---- Overlapping methods: pane.list, pane.focus, pane.split ----
        "pane.list" => {
            if params.contains_key("workspace") || params.contains_key("workspace_id") {
                (handle_cmux_pane_list(host, id, params, false), false)
            } else {
                dispatch_twitty_native(host, &req)
            }
        }
        "pane.focus" => {
            if params.contains_key("pane")
                && !params.contains_key("panel")
                && !params.contains_key("surface")
                && !params.contains_key("id")
            {
                dispatch_twitty_native(host, &req)
            } else {
                (handle_cmux_pane_focus(host, id, params), false)
            }
        }
        "pane.split" => {
            if params.contains_key("orientation") && !params.contains_key("direction") {
                (handle_cmux_split(host, id, params), false)
            } else {
                dispatch_twitty_native(host, &req)
            }
        }

        // ---- cmux v2 workspaces (= Twitty tabs) ----
        "workspace.list" => (handle_workspace_list(host, id), false),
        "workspace.current" => (handle_workspace_current(host, id), false),
        "workspace.create" => (handle_workspace_create(host, id, params, true), false),
        "workspace.new" => (handle_workspace_create(host, id, params, false), false),
        "workspace.select" => (handle_workspace_select(host, id, params), false),
        "workspace.close" => handle_workspace_close(host, id, params),
        "workspace.rename" => (handle_workspace_rename(host, id, params), false),
        "workspace.set_status" => (handle_workspace_set_status(host, id, params), false),
        "workspace.clear_status" => (handle_workspace_clear_status(host, id, params), false),
        "workspace.list_status" => (handle_workspace_list_status(host, id, params), false),

        // ---- cmux v2 panes & surfaces (= Twitty panes) ----
        "pane.new" | "pane.create" | "surface.split" => {
            (handle_cmux_split(host, id, params), false)
        }
        "pane.close" | "surface.close" => handle_cmux_pane_close(host, id, params),
        "surface.list" => (handle_cmux_pane_list(host, id, params, true), false),
        "surface.current" => (handle_surface_current(host, id), false),
        "surface.focus" => (handle_cmux_pane_focus(host, id, params), false),
        "surface.send_text" | "surface.send_input" => {
            (handle_surface_send_text(host, id, params), false)
        }
        "surface.send_key" => (handle_surface_send_key(host, id, params), false),
        "surface.read_text" => (handle_surface_read_text(host, id, params), false),

        // ---- cmux v2 notifications ----
        "notification.create"
        | "notification.create_for_surface"
        | "notification.create_for_target" => (handle_notification_create(host, id, params), false),
        "notification.list" => (handle_notification_list(host, id, params), false),
        "notification.clear" => (handle_notification_clear(host, id), false),

        // ---- cmux v2 sidebar visibility ----
        "sidebar.show" => {
            host.set_sidebar_visible(true);
            (Response::ok(id, json!({ "visible": true })), false)
        }
        "sidebar.hide" => {
            host.set_sidebar_visible(false);
            (Response::ok(id, json!({ "visible": false })), false)
        }
        "sidebar.toggle" => {
            let visible = host.toggle_sidebar();
            (
                Response::ok(id, json!({ "toggled": true, "visible": visible })),
                false,
            )
        }
        "sidebar.status" => (
            Response::ok(
                id,
                json!({
                    "visible": host.sidebar_visible(),
                    "workspace_count": host.ws().tabs().len(),
                }),
            ),
            false,
        ),

        // ---- Unknown / unsupported (including browser.*, markdown.*, settings.*) ----
        other => (
            Response::cmux_err(
                id,
                "unknown_method",
                format!("Unknown method: {}", clean_text(other, MAX_METHOD_LEN)),
            ),
            false,
        ),
    }
}

// ---------------------------------------------------------------------------
// Twitty native ctl dispatch (unchanged wire contract)
// ---------------------------------------------------------------------------

fn dispatch_twitty_native(host: &mut impl ControlHost, req: &Request) -> (Response, bool) {
    let id = req.id.clone();
    let call = match MethodCall::parse(req) {
        Ok(call) => call,
        Err(err) => return (Response::err(id, err), false),
    };
    match exec_twitty_call(host, call) {
        Ok((result, exit)) => (Response::ok(id, result), exit),
        Err(err) => (Response::err(id, err), false),
    }
}

fn exec_twitty_call(
    host: &mut impl ControlHost,
    call: MethodCall,
) -> Result<(Value, bool), String> {
    match call {
        MethodCall::SystemPing => Ok((
            json!({
                "pong": true,
                "version": env!("CARGO_PKG_VERSION"),
                "pid": std::process::id(),
            }),
            false,
        )),
        MethodCall::CommandList => Ok((host.command_list(), false)),
        MethodCall::CommandRun { name, pane } => host.command_run(&name, pane),
        MethodCall::TabList => {
            let active = host.ws().active();
            let list: Vec<Value> = host
                .ws()
                .tabs()
                .iter()
                .enumerate()
                .map(|(index, tab)| {
                    let panes: Vec<u64> = tab.root.leaves().into_iter().map(|p| p.0).collect();
                    json!({
                        "index": index,
                        "title": clean_text(&host.ws().tab_title(index), MAX_TITLE_CHARS),
                        "active": index == active,
                        "panes": panes,
                    })
                })
                .collect();
            Ok((Value::Array(list), false))
        }
        MethodCall::PaneList => {
            let focused = host.ws().focused_id();
            let mut list = Vec::new();
            for (tab_index, tab) in host.ws().tabs().iter().enumerate() {
                for pid in tab.root.leaves() {
                    let Some(pane) = host.ws().pane(pid) else {
                        continue;
                    };
                    let cwd = host
                        .pane_cwd(pid)
                        .map(|p| clean_text(&p.to_string_lossy(), MAX_CWD_BYTES))
                        .filter(|s| !s.is_empty());
                    let title = pane
                        .title
                        .as_deref()
                        .map(|t| clean_text(t, MAX_TITLE_CHARS))
                        .filter(|t| !t.is_empty());
                    let attention = pane.meta.attention.as_ref().map(|n| {
                        json!({
                            "title": n.title,
                            "body": n.body,
                        })
                    });
                    let agent_status = pane.meta.agent_status.as_ref().map(|s| {
                        json!({
                            "state": s.state.as_str(),
                            "text": s.text,
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
                        "agent_status": agent_status,
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
            let tab = host.focus_pane_by_id(PaneId(pane))?;
            Ok((json!({ "pane": pane, "tab": tab }), false))
        }
        MethodCall::PaneSendText { pane, text } => {
            let target = resolve_twitty_pane(host, pane)?;
            host.send_input(target, text.as_bytes());
            Ok((json!({ "pane": target.0, "bytes": text.len() }), false))
        }
        MethodCall::PaneRead { pane, lines } => {
            let target = resolve_twitty_pane(host, pane)?;
            let text = host
                .ws()
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
                shell: false,
            };
            let id = host
                .new_tab_with(options)
                .ok_or_else(|| "failed to open new tab".to_owned())?;
            Ok((json!({ "pane": id.0, "tab": host.ws().active() }), false))
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
            let prev_focus = host.ws().focused_id();
            match pane {
                Some(p) => {
                    host.focus_pane_by_id(PaneId(p))?;
                }
                None => {
                    if prev_focus.is_none() {
                        return Err("no focused pane".into());
                    }
                }
            }
            let options = SpawnOptions {
                cwd,
                command,
                env: Vec::new(),
                shell: false,
            };
            let id = match host.split_focused_with(direction, options) {
                Ok(id) => id,
                Err(err) => {
                    if pane.is_some() {
                        if let Some(prev) = prev_focus {
                            let _ = host.focus_pane_by_id(prev);
                        }
                    }
                    return Err(err.message);
                }
            };
            Ok((json!({ "pane": id.0, "tab": host.ws().active() }), false))
        }
        MethodCall::Notify { pane, title, body } => {
            let target = resolve_twitty_pane(host, pane)?;
            host.notify(target, title, body, true);
            Ok((json!({ "pane": target.0 }), false))
        }
        MethodCall::TabNewSsh { destination, name } => {
            Ok((host.new_ssh_tab(&destination, name.as_deref())?, false))
        }
    }
}

fn resolve_twitty_pane(host: &impl ControlHost, pane: Option<u64>) -> Result<PaneId, String> {
    match pane {
        Some(p) => {
            let id = PaneId(p);
            if host.ws().pane(id).is_some() {
                Ok(id)
            } else {
                Err(format!("unknown pane: {p}"))
            }
        }
        None => host
            .ws()
            .focused_id()
            .ok_or_else(|| "no focused pane".to_owned()),
    }
}
