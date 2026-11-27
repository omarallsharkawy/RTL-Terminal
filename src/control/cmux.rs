// Adapted from cmux-gtk cmux/src/socket/v2/{mod,system,workspace,pane,surface,notification,sidebar,helpers}.rs
// and cmux/src/socket/v1.rs (AGPL-3.0-or-later, Copyright (c) 2026 Douglas Soares de Andrade).
// Changes: mapped cmux v2 methods and minimal Claude-hook v1 text commands onto Twitty's
// Workspace/Pane/NotificationHistory model via ControlHost, kept Twitty's ctl methods intact,
// enforced Twitty text cleaning and size limits, and added a window-free HeadlessHost.

//! cmux-compatible v2 JSON-RPC and minimal v1 text control dispatch, mapped onto Twitty's tabs,
//! panes, notifications, and sidebar status.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use alacritty_terminal::term::TermMode;
use serde_json::{json, Map, Value};
use winit::event::ElementState;
use winit::keyboard::{Key, KeyCode, ModifiersState, NamedKey};

use crate::control::protocol::{
    bound_read_text, parse_wire_line, MethodCall, Request, Response, DEFAULT_READ_LINES,
    MAX_COMMAND_ARGS, MAX_COMMAND_BYTES, MAX_METHOD_LEN, MAX_READ_BYTES, MAX_READ_LINES,
    MAX_SEND_TEXT_BYTES,
};
use crate::input::terminal_key_raw;
use crate::layout::{PaneId, Rect, SplitDir};
use crate::pty::SpawnOptions;
use crate::session::pane::{Notification, Pane};
use crate::session::workspace::{GridGeometry, PaneFactory, SplitFailure, Workspace};
use crate::shell_integration::{clean_text, MAX_NOTIFY_CHARS};
use crate::sidebar::{entry_title, pick_tab_agent_status, AgentStatus, Entry, Notice};
use crate::terminal::Terminal;
use crate::ui::notifications::NotificationHistory;

pub use twitty_control::protocol::parse_v1_line;

const MAX_CWD_BYTES: usize = 4096;
const MAX_TITLE_CHARS: usize = 256;
const MAX_KEY_MODS: usize = 16;

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
// Key encoding for `surface.send_key`
// ---------------------------------------------------------------------------

/// Encode a cmux `surface.send_key` request (`key`, `mods`) into terminal input bytes using
/// Twitty's key encoder and the target pane's current [`TermMode`].
pub fn encode_key_name(key: &str, mods: &[String], mode: TermMode) -> Result<Vec<u8>, String> {
    if key.is_empty() {
        return Err("key must not be empty".into());
    }
    let mut modifiers = ModifiersState::empty();
    let mut base_key = key;

    if mods.is_empty() && key.len() > 1 && key.contains('+') {
        let parts: Vec<&str> = key.split('+').collect();
        if parts.len() >= 2
            && parts[..parts.len() - 1]
                .iter()
                .all(|m| parse_mod(m).is_some())
        {
            for m in &parts[..parts.len() - 1] {
                if let Some(flag) = parse_mod(m) {
                    modifiers |= flag;
                }
            }
            base_key = parts[parts.len() - 1];
        }
    } else {
        for m in mods {
            let Some(flag) = parse_mod(m) else {
                return Err(format!("unknown modifier: {m:?}"));
            };
            modifiers |= flag;
        }
    }

    let (physical, logical, text_owned) = resolve_key(base_key)?;
    let text_adjusted = if modifiers.shift_key() && !modifiers.control_key() {
        text_owned.as_deref().map(|t| t.to_ascii_uppercase())
    } else {
        text_owned
    };
    let text = text_adjusted.as_deref();

    terminal_key_raw(
        physical,
        &logical,
        Some(&logical),
        text,
        ElementState::Pressed,
        false,
        modifiers,
        mode,
    )
    .ok_or_else(|| format!("key {key:?} produced no terminal input"))
}

fn parse_mod(raw: &str) -> Option<ModifiersState> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(ModifiersState::CONTROL),
        "shift" => Some(ModifiersState::SHIFT),
        "alt" | "opt" | "option" => Some(ModifiersState::ALT),
        "super" | "meta" | "cmd" | "command" => Some(ModifiersState::SUPER),
        _ => None,
    }
}

fn resolve_key(name: &str) -> Result<(Option<KeyCode>, Key, Option<String>), String> {
    let mut chars = name.chars();
    if let Some(ch) = chars.next() {
        if chars.next().is_none() {
            if ch.is_ascii_control() {
                return Err("control characters are not valid key names".into());
            }
            if ch == ' ' {
                return Ok((
                    Some(KeyCode::Space),
                    Key::Named(NamedKey::Space),
                    Some(" ".into()),
                ));
            }
            if ch.is_ascii_graphic() {
                let physical = ascii_keycode(ch);
                let s = ch.to_string();
                return Ok((physical, Key::Character(s.clone().into()), Some(s)));
            }
            return Err(format!("unsupported key name: {name:?}"));
        }
    }

    let (physical, named, text) = match name.to_ascii_lowercase().as_str() {
        "return" | "enter" => (Some(KeyCode::Enter), NamedKey::Enter, None),
        "escape" | "esc" => (Some(KeyCode::Escape), NamedKey::Escape, None),
        "tab" => (Some(KeyCode::Tab), NamedKey::Tab, None),
        "backspace" => (Some(KeyCode::Backspace), NamedKey::Backspace, None),
        "delete" | "del" => (Some(KeyCode::Delete), NamedKey::Delete, None),
        "space" => (Some(KeyCode::Space), NamedKey::Space, Some(" ".into())),
        "up" | "arrow_up" => (Some(KeyCode::ArrowUp), NamedKey::ArrowUp, None),
        "down" | "arrow_down" => (Some(KeyCode::ArrowDown), NamedKey::ArrowDown, None),
        "left" | "arrow_left" => (Some(KeyCode::ArrowLeft), NamedKey::ArrowLeft, None),
        "right" | "arrow_right" => (Some(KeyCode::ArrowRight), NamedKey::ArrowRight, None),
        "home" => (Some(KeyCode::Home), NamedKey::Home, None),
        "end" => (Some(KeyCode::End), NamedKey::End, None),
        "page_up" | "pageup" => (Some(KeyCode::PageUp), NamedKey::PageUp, None),
        "page_down" | "pagedown" => (Some(KeyCode::PageDown), NamedKey::PageDown, None),
        "insert" => (Some(KeyCode::Insert), NamedKey::Insert, None),
        "f1" => (Some(KeyCode::F1), NamedKey::F1, None),
        "f2" => (Some(KeyCode::F2), NamedKey::F2, None),
        "f3" => (Some(KeyCode::F3), NamedKey::F3, None),
        "f4" => (Some(KeyCode::F4), NamedKey::F4, None),
        "f5" => (Some(KeyCode::F5), NamedKey::F5, None),
        "f6" => (Some(KeyCode::F6), NamedKey::F6, None),
        "f7" => (Some(KeyCode::F7), NamedKey::F7, None),
        "f8" => (Some(KeyCode::F8), NamedKey::F8, None),
        "f9" => (Some(KeyCode::F9), NamedKey::F9, None),
        "f10" => (Some(KeyCode::F10), NamedKey::F10, None),
        "f11" => (Some(KeyCode::F11), NamedKey::F11, None),
        "f12" => (Some(KeyCode::F12), NamedKey::F12, None),
        _ => return Err(format!("unknown key name: {name:?}")),
    };
    Ok((physical, Key::Named(named), text))
}

fn ascii_keycode(ch: char) -> Option<KeyCode> {
    Some(match ch.to_ascii_lowercase() {
        'a' => KeyCode::KeyA,
        'b' => KeyCode::KeyB,
        'c' => KeyCode::KeyC,
        'd' => KeyCode::KeyD,
        'e' => KeyCode::KeyE,
        'f' => KeyCode::KeyF,
        'g' => KeyCode::KeyG,
        'h' => KeyCode::KeyH,
        'i' => KeyCode::KeyI,
        'j' => KeyCode::KeyJ,
        'k' => KeyCode::KeyK,
        'l' => KeyCode::KeyL,
        'm' => KeyCode::KeyM,
        'n' => KeyCode::KeyN,
        'o' => KeyCode::KeyO,
        'p' => KeyCode::KeyP,
        'q' => KeyCode::KeyQ,
        'r' => KeyCode::KeyR,
        's' => KeyCode::KeyS,
        't' => KeyCode::KeyT,
        'u' => KeyCode::KeyU,
        'v' => KeyCode::KeyV,
        'w' => KeyCode::KeyW,
        'x' => KeyCode::KeyX,
        'y' => KeyCode::KeyY,
        'z' => KeyCode::KeyZ,
        '0' => KeyCode::Digit0,
        '1' => KeyCode::Digit1,
        '2' => KeyCode::Digit2,
        '3' => KeyCode::Digit3,
        '4' => KeyCode::Digit4,
        '5' => KeyCode::Digit5,
        '6' => KeyCode::Digit6,
        '7' => KeyCode::Digit7,
        '8' => KeyCode::Digit8,
        '9' => KeyCode::Digit9,
        '[' => KeyCode::BracketLeft,
        ']' => KeyCode::BracketRight,
        '\\' => KeyCode::Backslash,
        '/' => KeyCode::Slash,
        _ => return None,
    })
}

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

// ---------------------------------------------------------------------------
// cmux v2 handlers
// ---------------------------------------------------------------------------

fn handle_workspace_list(host: &impl ControlHost, id: Value) -> Response {
    let active = host.ws().active();
    let workspaces: Vec<Value> = host
        .ws()
        .tabs()
        .iter()
        .enumerate()
        .map(|(index, tab)| {
            let leaves = tab.root.leaves();
            let selected = index == active;
            let cwd = host
                .pane_cwd(tab.focused)
                .map(|p| clean_text(&p.to_string_lossy(), MAX_CWD_BYTES))
                .unwrap_or_default();
            let mut attention_panes: Vec<_> = leaves
                .iter()
                .filter_map(|&pid| {
                    let n = host.ws().pane(pid)?.meta.attention.as_ref()?;
                    Some((n.at, pid, n))
                })
                .collect();
            attention_panes.sort_by_key(|&(at, ..)| at);
            let unread_count = attention_panes.len();
            let latest = attention_panes.last();
            let latest_notification = latest.map(|(_, _, n)| match &n.title {
                Some(t) if !n.body.is_empty() => format!("{t}: {}", n.body),
                Some(t) => t.clone(),
                None => n.body.clone(),
            });
            let attention_panel_id = latest.map(|(_, pid, _)| pid.0.to_string());
            json!({
                "index": index,
                "id": tab.id.to_string(),
                "title": clean_text(&host.ws().tab_title(index), MAX_TITLE_CHARS),
                "directory": cwd,
                "panel_count": leaves.len(),
                "unread_count": unread_count,
                "latest_notification": latest_notification,
                "attention_panel_id": attention_panel_id,
                "selected": selected,
                "is_selected": selected,
            })
        })
        .collect();

    Response::ok(id, json!({ "workspaces": workspaces }))
}

fn handle_workspace_current(host: &impl ControlHost, id: Value) -> Response {
    let active = host.ws().active();
    let Some(tab) = host.ws().tabs().get(active) else {
        return Response::cmux_err(id, "not_found", "No workspace selected");
    };
    let cwd = host
        .pane_cwd(tab.focused)
        .map(|p| clean_text(&p.to_string_lossy(), MAX_CWD_BYTES))
        .unwrap_or_default();
    Response::ok(
        id,
        json!({
            "index": active,
            "id": tab.id.to_string(),
            "title": clean_text(&host.ws().tab_title(active), MAX_TITLE_CHARS),
            "directory": cwd,
            "panel_count": tab.root.leaves().len(),
            "focused_panel_id": tab.focused.0.to_string(),
        }),
    )
}

fn handle_workspace_create(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
    preserve_selection: bool,
) -> Response {
    let cwd = match parse_optional_cwd(params) {
        Ok(cwd) => cwd,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    if let Some(ref dir) = cwd {
        if !dir.is_dir() {
            return Response::cmux_err(
                id,
                "invalid_params",
                format!("directory does not exist: {}", dir.display()),
            );
        }
    }
    let title = match parse_optional_string(params, "title") {
        Ok(t) => t
            .map(|s| clean_text(s, MAX_TITLE_CHARS))
            .filter(|s| !s.is_empty()),
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    let command = match parse_optional_command(params) {
        Ok(cmd) => cmd,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };

    let prev_active = if preserve_selection && !host.ws().is_empty() {
        Some(host.ws().active())
    } else {
        None
    };

    let options = SpawnOptions {
        cwd,
        command,
        env: Vec::new(),
        shell: false,
    };
    let Some(pane_id) = host.new_tab_with(options) else {
        return Response::cmux_err(id, "internal", "failed to create workspace");
    };

    let new_index = host.ws().active();
    if let Some(title) = title {
        host.ws_mut().rename_tab(new_index, Some(title));
    }
    let tab_id = host.ws().tabs()[new_index].id;
    if let Some(prev) = prev_active {
        host.set_active_tab(prev);
    }
    host.refresh_title();
    host.request_redraw();

    Response::ok(
        id,
        json!({
            "workspace_id": tab_id.to_string(),
            "workspace": tab_id.to_string(),
            "panel_id": pane_id.0.to_string(),
            "surface_id": pane_id.0.to_string(),
            "index": new_index,
        }),
    )
}

fn handle_workspace_select(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let index = match parse_optional_usize(params, "index") {
        Ok(v) => v,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    let ws_id = match parse_optional_id(params, &["workspace", "workspace_id"]) {
        Ok(v) => v,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };

    let target_index = if let Some(idx) = index {
        if idx < host.ws().tabs().len() {
            Some(idx)
        } else {
            None
        }
    } else if let Some(wid) = ws_id {
        host.ws().tab_index_by_id(wid)
    } else {
        return Response::cmux_err(
            id,
            "invalid_params",
            "Provide 'index' or 'workspace'/'workspace_id'",
        );
    };

    let Some(target_index) = target_index else {
        return Response::cmux_err(id, "not_found", "Workspace not found");
    };

    let tab_id = host.ws().tabs()[target_index].id;
    host.set_active_tab(target_index);
    Response::ok(
        id,
        json!({
            "selected": true,
            "workspace_id": tab_id.to_string(),
            "index": target_index,
        }),
    )
}

fn handle_workspace_close(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> (Response, bool) {
    let index = match parse_optional_usize(params, "index") {
        Ok(v) => v,
        Err(msg) => return (Response::cmux_err(id, "invalid_params", msg), false),
    };
    let ws_id = match parse_optional_id(params, &["workspace", "workspace_id"]) {
        Ok(v) => v,
        Err(msg) => return (Response::cmux_err(id, "invalid_params", msg), false),
    };

    let target_index = if let Some(idx) = index {
        (idx < host.ws().tabs().len()).then_some(idx)
    } else if let Some(wid) = ws_id {
        host.ws().tab_index_by_id(wid)
    } else if !host.ws().is_empty() {
        Some(host.ws().active())
    } else {
        None
    };

    let Some(target_index) = target_index else {
        return (
            Response::cmux_err(id, "not_found", "Workspace not found"),
            false,
        );
    };

    let tab_id = host.ws().tabs()[target_index].id;
    let should_exit = host.close_tab(target_index);
    (
        Response::ok(
            id,
            json!({
                "closed": true,
                "workspace_id": tab_id.to_string(),
            }),
        ),
        should_exit,
    )
}

fn handle_workspace_rename(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let target_index = match resolve_workspace_index(host, params) {
        Ok(idx) => idx,
        Err((code, msg)) => return Response::cmux_err(id, code, msg),
    };
    let raw_title = params
        .get("title")
        .or_else(|| params.get("name"))
        .and_then(Value::as_str);
    let Some(raw_title) = raw_title else {
        return Response::cmux_err(id, "invalid_params", "Provide 'title' string");
    };
    let cleaned = clean_text(raw_title, MAX_TITLE_CHARS);
    let custom = (!cleaned.is_empty()).then_some(cleaned);
    host.ws_mut().rename_tab(target_index, custom);
    let tab_id = host.ws().tabs()[target_index].id;
    let shown = clean_text(&host.ws().tab_title(target_index), MAX_TITLE_CHARS);
    host.refresh_title();
    host.request_redraw();
    Response::ok(
        id,
        json!({
            "ok": true,
            "workspace_id": tab_id.to_string(),
            "title": shown,
        }),
    )
}

fn handle_workspace_set_status(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let key = match params.get("key").and_then(Value::as_str) {
        Some(s) if !clean_text(s, 64).is_empty() => s,
        _ => return Response::cmux_err(id, "invalid_params", "Provide non-empty 'key'"),
    };
    let value = match params.get("value").and_then(Value::as_str) {
        Some(s) if !clean_text(s, 64).is_empty() => s,
        _ => return Response::cmux_err(id, "invalid_params", "Provide non-empty 'value'"),
    };
    let label = match parse_optional_string(params, "label").and_then(|v| match v {
        Some(s) => Ok(Some(s)),
        None => parse_optional_string(params, "message"),
    }) {
        Ok(v) => v,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    let icon = match parse_optional_string(params, "icon") {
        Ok(v) => v,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    let color = match parse_optional_string(params, "color") {
        Ok(v) => v,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };

    let pane_id = match resolve_cmux_target_pane(host, params) {
        Ok(p) => p,
        Err((code, msg)) => return Response::cmux_err(id, code, msg),
    };
    let tab_id = host.ws().tab_id_of_pane(pane_id);
    if let Some(pane) = host.ws_mut().pane_mut(pane_id) {
        pane.meta.set_status(key, value, label, icon, color);
    }
    host.request_redraw();
    Response::ok(
        id,
        json!({
            "ok": true,
            "workspace_id": tab_id.map(|t| t.to_string()),
            "surface": pane_id.0.to_string(),
        }),
    )
}

fn handle_workspace_clear_status(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let key = match parse_optional_string(params, "key") {
        Ok(v) => v.map(|s| clean_text(s, 64)).filter(|s| !s.is_empty()),
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    let pane_id = match resolve_cmux_target_pane(host, params) {
        Ok(p) => p,
        Err((code, msg)) => return Response::cmux_err(id, code, msg),
    };
    if let Some(pane) = host.ws_mut().pane_mut(pane_id) {
        pane.meta.clear_status(key.as_deref());
    }
    host.request_redraw();
    Response::ok(id, json!({ "ok": true }))
}

fn handle_workspace_list_status(
    host: &impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let explicit_pane = match parse_optional_id(params, &["surface", "panel", "pane"]) {
        Ok(v) => v,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    let pane_ids: Vec<PaneId> = if let Some(pid) = explicit_pane {
        let pane_id = PaneId(pid);
        if host.ws().pane(pane_id).is_none() {
            return Response::cmux_err(id, "not_found", "Surface not found");
        }
        vec![pane_id]
    } else {
        let tab_idx = match resolve_workspace_index(host, params) {
            Ok(idx) => idx,
            Err((code, msg)) => return Response::cmux_err(id, code, msg),
        };
        host.ws().tabs()[tab_idx].root.leaves()
    };

    let mut entries = Vec::new();
    for pid in pane_ids {
        let Some(pane) = host.ws().pane(pid) else {
            continue;
        };
        for entry in &pane.meta.status_entries {
            entries.push(json!({
                "key": entry.key,
                "value": entry.value,
                "label": entry.label,
                "icon": entry.icon,
                "color": entry.color,
                "surface": pid.0.to_string(),
            }));
        }
    }
    Response::ok(id, json!({ "entries": entries }))
}

fn handle_cmux_pane_list(
    host: &impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
    include_surfaces_alias: bool,
) -> Response {
    let tab_idx = match resolve_workspace_index(host, params) {
        Ok(idx) => idx,
        Err((code, msg)) => return Response::cmux_err(id, code, msg),
    };
    let tab = &host.ws().tabs()[tab_idx];
    let tab_id = tab.id;
    let panels: Vec<Value> = tab
        .root
        .leaves()
        .into_iter()
        .filter_map(|pid| {
            let pane = host.ws().pane(pid)?;
            let cwd = host
                .pane_cwd(pid)
                .map(|p| clean_text(&p.to_string_lossy(), MAX_CWD_BYTES))
                .filter(|s| !s.is_empty());
            let title = pane
                .title
                .as_deref()
                .map(|t| clean_text(t, MAX_TITLE_CHARS))
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| format!("pane {}", pid.0));
            let agent_status = pane.meta.agent_status.as_ref().map(|s| {
                json!({
                    "state": s.state.as_str(),
                    "text": s.text,
                })
            });
            Some(json!({
                "id": pid.0.to_string(),
                "type": "terminal",
                "title": title,
                "directory": cwd,
                "focused": tab.focused == pid,
                "workspace_id": tab_id.to_string(),
                "agent_status": agent_status,
            }))
        })
        .collect();

    if include_surfaces_alias {
        Response::ok(id, json!({ "panels": panels, "surfaces": panels }))
    } else {
        Response::ok(id, json!({ "panels": panels }))
    }
}

fn handle_surface_current(host: &impl ControlHost, id: Value) -> Response {
    let Some(pid) = host.ws().focused_id() else {
        return Response::cmux_err(id, "not_found", "No focused surface");
    };
    let Some(pane) = host.ws().pane(pid) else {
        return Response::cmux_err(id, "not_found", "Surface not found");
    };
    let tab_id = host.ws().tab_id_of_pane(pid);
    let cwd = host
        .pane_cwd(pid)
        .map(|p| clean_text(&p.to_string_lossy(), MAX_CWD_BYTES))
        .filter(|s| !s.is_empty());
    let title = pane
        .title
        .as_deref()
        .map(|t| clean_text(t, MAX_TITLE_CHARS))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| format!("pane {}", pid.0));
    Response::ok(
        id,
        json!({
            "id": pid.0.to_string(),
            "type": "terminal",
            "title": title,
            "directory": cwd,
            "workspace_id": tab_id.map(|t| t.to_string()),
        }),
    )
}

fn handle_cmux_split(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let dir = match params
        .get("direction")
        .or_else(|| params.get("orientation"))
        .and_then(Value::as_str)
    {
        None | Some("horizontal") | Some("right") | Some("left") => SplitDir::Horizontal,
        Some("vertical") | Some("down") | Some("up") => SplitDir::Vertical,
        Some(other) => {
            return Response::cmux_err(
                id,
                "invalid_params",
                format!("invalid split direction/orientation: {other:?}"),
            );
        }
    };
    let cwd = match parse_optional_cwd(params) {
        Ok(c) => c,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    if let Some(ref d) = cwd {
        if !d.is_dir() {
            return Response::cmux_err(
                id,
                "invalid_params",
                format!("directory does not exist: {}", d.display()),
            );
        }
    }
    let command = match parse_optional_command(params) {
        Ok(cmd) => cmd,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    let explicit_pane = match parse_optional_id(params, &["surface", "panel", "pane"]) {
        Ok(v) => v,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    let prev_focus = host.ws().focused_id();
    if let Some(pid) = explicit_pane {
        if let Err(msg) = host.focus_pane_by_id(PaneId(pid)) {
            return Response::cmux_err(id, "not_found", msg);
        }
    } else if prev_focus.is_none() {
        return Response::cmux_err(id, "not_found", "No workspace selected");
    }

    let options = SpawnOptions {
        cwd,
        command,
        env: Vec::new(),
        shell: false,
    };
    let new_id = match host.split_focused_with(dir, options) {
        Ok(id) => id,
        Err(err) => {
            if explicit_pane.is_some() {
                if let Some(prev) = prev_focus {
                    let _ = host.focus_pane_by_id(prev);
                }
            }
            return Response::cmux_err(id, err.code, err.message);
        }
    };
    let tab_idx = host.ws().active();
    let tab_id = host.ws().tabs()[tab_idx].id;
    Response::ok(
        id,
        json!({
            "panel_id": new_id.0.to_string(),
            "surface_id": new_id.0.to_string(),
            "pane": new_id.0,
            "tab": tab_idx,
            "workspace_id": tab_id.to_string(),
        }),
    )
}

fn handle_cmux_pane_focus(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let target = match parse_optional_id(params, &["panel", "surface", "id", "pane"]) {
        Ok(Some(pid)) => PaneId(pid),
        Ok(None) => {
            return Response::cmux_err(id, "invalid_params", "Provide 'panel'/'surface'/'id'")
        }
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    match host.focus_pane_by_id(target) {
        Ok(tab) => Response::ok(
            id,
            json!({
                "focused": true,
                "pane": target.0,
                "panel_id": target.0.to_string(),
                "tab": tab,
            }),
        ),
        Err(_) => Response::cmux_err(id, "not_found", "Panel not found"),
    }
}

fn handle_cmux_pane_close(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> (Response, bool) {
    let explicit = match parse_optional_id(params, &["panel", "surface", "id", "pane"]) {
        Ok(v) => v,
        Err(msg) => return (Response::cmux_err(id, "invalid_params", msg), false),
    };
    let target = match explicit {
        Some(pid) => {
            let pane_id = PaneId(pid);
            if host.ws().pane(pane_id).is_none() {
                return (
                    Response::cmux_err(id, "not_found", "Panel not found"),
                    false,
                );
            }
            pane_id
        }
        None => match host.ws().focused_id() {
            Some(pid) => pid,
            None => {
                return (
                    Response::cmux_err(id, "not_found", "No focused panel"),
                    false,
                )
            }
        },
    };
    let should_exit = host.close_pane(target);
    (
        Response::ok(
            id,
            json!({
                "closed": true,
                "panel_id": target.0.to_string(),
            }),
        ),
        should_exit,
    )
}

fn handle_surface_send_text(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let text = match params
        .get("text")
        .or_else(|| params.get("input"))
        .and_then(Value::as_str)
    {
        Some(t) => t,
        None => return Response::cmux_err(id, "invalid_params", "Provide 'text' or 'input'"),
    };
    if text.len() > MAX_SEND_TEXT_BYTES {
        return Response::cmux_err(
            id,
            "invalid_params",
            format!("text exceeds maximum size of {MAX_SEND_TEXT_BYTES} bytes"),
        );
    }
    let target = match resolve_cmux_target_pane(host, params) {
        Ok(p) => p,
        Err((code, msg)) => return Response::cmux_err(id, code, msg),
    };
    host.send_input(target, text.as_bytes());
    Response::ok(
        id,
        json!({
            "sent": true,
            "surface": target.0.to_string(),
            "bytes": text.len(),
        }),
    )
}

fn handle_surface_send_key(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let Some(key_name) = params.get("key").and_then(Value::as_str) else {
        return Response::cmux_err(
            id,
            "invalid_params",
            "Provide 'key' (e.g. 'c', 'Return', 'Escape')",
        );
    };
    let mut mods = Vec::new();
    if let Some(raw_mods) = params.get("mods") {
        if !raw_mods.is_null() {
            let Some(arr) = raw_mods.as_array() else {
                return Response::cmux_err(
                    id,
                    "invalid_params",
                    "'mods' must be an array of strings",
                );
            };
            if arr.len() > MAX_KEY_MODS {
                return Response::cmux_err(
                    id,
                    "invalid_params",
                    format!("'mods' exceeds maximum of {MAX_KEY_MODS} entries"),
                );
            }
            for item in arr {
                let Some(s) = item.as_str() else {
                    return Response::cmux_err(
                        id,
                        "invalid_params",
                        "'mods' entries must be strings",
                    );
                };
                mods.push(s.to_owned());
            }
        }
    }

    let target = match resolve_cmux_target_pane(host, params) {
        Ok(p) => p,
        Err((code, msg)) => return Response::cmux_err(id, code, msg),
    };
    let mode = host.ws().pane(target).map_or(TermMode::NONE, Pane::mode);
    let bytes = match encode_key_name(key_name, &mods, mode) {
        Ok(b) => b,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    host.send_input(target, &bytes);
    Response::ok(
        id,
        json!({
            "sent": true,
            "surface": target.0.to_string(),
            "bytes": bytes.len(),
        }),
    )
}

fn handle_surface_read_text(
    host: &impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let target = match resolve_cmux_target_pane(host, params) {
        Ok(p) => p,
        Err((code, msg)) => return Response::cmux_err(id, code, msg),
    };
    let lines = match parse_optional_usize(params, "lines") {
        Ok(None) => {
            if params
                .get("scrollback")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                MAX_READ_LINES
            } else {
                DEFAULT_READ_LINES
            }
        }
        Ok(Some(n)) if n > MAX_READ_LINES => {
            return Response::cmux_err(
                id,
                "invalid_params",
                format!("'lines' exceeds maximum of {MAX_READ_LINES}"),
            );
        }
        Ok(Some(n)) => n,
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    let Some(pane) = host.ws().pane(target) else {
        return Response::cmux_err(id, "not_found", "Surface not found");
    };
    let text = bound_read_text(&pane.terminal.recent_lines(lines), MAX_READ_BYTES);
    Response::ok(
        id,
        json!({
            "text": text,
            "surface": target.0.to_string(),
        }),
    )
}

fn handle_notification_create(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let title = match parse_optional_string(params, "title") {
        Ok(t) => t
            .map(|s| clean_text(s, MAX_NOTIFY_CHARS))
            .filter(|s| !s.is_empty()),
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    let body = match parse_optional_string(params, "body").and_then(|v| match v {
        Some(s) => Ok(Some(s)),
        None => parse_optional_string(params, "message"),
    }) {
        Ok(b) => b
            .map(|s| clean_text(s, MAX_NOTIFY_CHARS))
            .unwrap_or_default(),
        Err(msg) => return Response::cmux_err(id, "invalid_params", msg),
    };
    if title.is_none() && body.is_empty() {
        return Response::cmux_err(id, "invalid_params", "Provide non-empty 'title' or 'body'");
    }
    let send_desktop = params
        .get("send_desktop")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let target = match resolve_cmux_target_pane(host, params) {
        Ok(p) => p,
        Err((code, msg)) => return Response::cmux_err(id, code, msg),
    };
    let tab_id = host.ws().tab_id_of_pane(target);
    host.notify(target, title, body, send_desktop);
    Response::ok(
        id,
        json!({
            "notified": true,
            "workspace": tab_id.map(|t| t.to_string()),
            "workspace_id": tab_id.map(|t| t.to_string()),
            "surface": target.0.to_string(),
        }),
    )
}

fn handle_notification_list(
    host: &impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let unread_only = params
        .get("unread")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let notifications: Vec<Value> = host
        .notification_history()
        .newest_first()
        .into_iter()
        .filter(|n| !unread_only || !n.seen)
        .map(|n| {
            let timestamp =
                n.at.duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs_f64();
            let ws_id = host.ws().tab_id_of_pane(n.pane).map(|t| t.to_string());
            json!({
                "id": n.id.to_string(),
                "title": n.title.as_deref().unwrap_or(""),
                "body": n.body,
                "workspace_id": ws_id,
                "panel_id": n.pane.0.to_string(),
                "surface_id": n.pane.0.to_string(),
                "timestamp": timestamp,
                "is_read": n.seen,
            })
        })
        .collect();
    let count = notifications.len();
    Response::ok(
        id,
        json!({
            "notifications": notifications,
            "count": count,
        }),
    )
}

fn handle_notification_clear(host: &mut impl ControlHost, id: Value) -> Response {
    host.notification_history_mut().clear();
    let ids: Vec<PaneId> = host.ws().panes().map(|p| p.id).collect();
    for pid in ids {
        if let Some(pane) = host.ws_mut().pane_mut(pid) {
            pane.meta.attention = None;
        }
    }
    host.request_redraw();
    Response::ok(id, json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// Shared cmux parameter helpers
// ---------------------------------------------------------------------------

fn resolve_workspace_index(
    host: &impl ControlHost,
    params: &Map<String, Value>,
) -> Result<usize, (&'static str, String)> {
    if let Some(idx) = parse_optional_usize(params, "index").map_err(|m| ("invalid_params", m))? {
        return (idx < host.ws().tabs().len())
            .then_some(idx)
            .ok_or(("not_found", "Workspace not found".into()));
    }
    if let Some(wid) = parse_optional_id(params, &["workspace", "workspace_id"])
        .map_err(|m| ("invalid_params", m))?
    {
        return host
            .ws()
            .tab_index_by_id(wid)
            .ok_or(("not_found", "Workspace not found".into()));
    }
    if host.ws().is_empty() {
        Err(("not_found", "No workspace selected".into()))
    } else {
        Ok(host.ws().active())
    }
}

fn resolve_cmux_target_pane(
    host: &impl ControlHost,
    params: &Map<String, Value>,
) -> Result<PaneId, (&'static str, String)> {
    if let Some(pid) = parse_optional_id(params, &["surface", "panel", "pane"])
        .map_err(|m| ("invalid_params", m))?
    {
        let pane_id = PaneId(pid);
        return if host.ws().pane(pane_id).is_some() {
            Ok(pane_id)
        } else {
            Err(("not_found", format!("Surface not found: {pid}")))
        };
    }
    if params.contains_key("workspace")
        || params.contains_key("workspace_id")
        || params.contains_key("index")
    {
        let idx = resolve_workspace_index(host, params)?;
        return Ok(host.ws().tabs()[idx].focused);
    }
    host.ws()
        .focused_id()
        .ok_or(("not_found", "No focused surface".into()))
}

fn parse_optional_id(params: &Map<String, Value>, keys: &[&str]) -> Result<Option<u64>, String> {
    for &key in keys {
        let Some(val) = params.get(key) else {
            continue;
        };
        if val.is_null() {
            continue;
        }
        if let Some(n) = val.as_u64() {
            return Ok(Some(n));
        }
        if let Some(s) = val.as_str() {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                continue;
            }
            return trimmed
                .parse::<u64>()
                .map(Some)
                .map_err(|_| format!("invalid id for '{key}': {trimmed:?}"));
        }
        return Err(format!("'{key}' must be an id string or integer"));
    }
    Ok(None)
}

fn parse_optional_usize(params: &Map<String, Value>, key: &str) -> Result<Option<usize>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| format!("'{key}' must be a non-negative integer")),
    }
}

fn parse_optional_string<'a>(
    params: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(format!("'{key}' must be a string")),
    }
}

fn parse_optional_cwd(params: &Map<String, Value>) -> Result<Option<PathBuf>, String> {
    let raw = match parse_optional_string(params, "directory")? {
        Some(s) => Some(s),
        None => parse_optional_string(params, "cwd")?,
    };
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Err("'directory'/'cwd' must not be empty".into());
    }
    if raw.len() > MAX_CWD_BYTES {
        return Err(format!(
            "'directory'/'cwd' exceeds maximum length of {MAX_CWD_BYTES} bytes"
        ));
    }
    if raw.chars().any(char::is_control) {
        return Err("'directory'/'cwd' must not contain control characters".into());
    }
    Ok(Some(PathBuf::from(raw)))
}

fn parse_optional_command(params: &Map<String, Value>) -> Result<Vec<String>, String> {
    let Some(val) = params.get("command") else {
        return Ok(Vec::new());
    };
    if val.is_null() {
        return Ok(Vec::new());
    }
    let Some(arr) = val.as_array() else {
        return Err("'command' must be an argv array of strings".into());
    };
    if arr.is_empty() {
        return Err("'command' must not be empty".into());
    }
    if arr.len() > MAX_COMMAND_ARGS {
        return Err(format!(
            "'command' exceeds maximum of {MAX_COMMAND_ARGS} arguments"
        ));
    }
    let mut out = Vec::with_capacity(arr.len());
    let mut total_bytes = 0usize;
    for item in arr {
        let Some(s) = item.as_str() else {
            return Err("'command' entries must be strings".into());
        };
        if s.contains('\0') {
            return Err("'command' entries must not contain NUL bytes".into());
        }
        total_bytes = total_bytes.saturating_add(s.len());
        if total_bytes > MAX_COMMAND_BYTES {
            return Err(format!(
                "'command' exceeds maximum total size of {MAX_COMMAND_BYTES} bytes"
            ));
        }
        out.push(s.to_owned());
    }
    if out[0].is_empty() {
        return Err("'command' program name must not be empty".into());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// HeadlessHost: window-free Workspace-backed harness for tests
// ---------------------------------------------------------------------------

struct HeadlessGrid;

impl GridGeometry for HeadlessGrid {
    fn content_rect(&self, tab_count: usize) -> Rect {
        let bar = if tab_count > 1 { 20.0 } else { 0.0 };
        Rect::new(0.0, bar, 1000.0, 600.0 - bar)
    }

    fn gap(&self) -> f32 {
        1.0
    }

    fn grid_size(&self, rect: Rect) -> (usize, usize) {
        ((rect.w / 10.0) as usize, (rect.h / 20.0) as usize)
    }
}

struct HeadlessFactory {
    options: SpawnOptions,
    tab_id: u64,
    spawned: Vec<(PaneId, u64, SpawnOptions)>,
}

impl PaneFactory for HeadlessFactory {
    fn set_tab_id(&mut self, tab_id: u64) {
        self.tab_id = tab_id;
    }

    fn spawn(
        &mut self,
        id: PaneId,
        cols: usize,
        rows: usize,
        cwd: Option<PathBuf>,
    ) -> Option<Pane> {
        let mut opts = self.options.clone();
        opts.cwd = opts.cwd.or(cwd);
        let terminal = Terminal::new(cols, rows, |_| {});
        let mut pane = Pane::new(id, terminal, None, (cols, rows));
        pane.meta.cwd = opts.cwd.clone();
        self.spawned.push((id, self.tab_id, opts));
        Some(pane)
    }
}

/// Window-free `Workspace` + [`NotificationHistory`] harness implementing control dispatch so
/// every cmux v2, v1, and Twitty `ctl` method can be tested without a GUI window or PTY.
pub struct HeadlessHost {
    ws: Workspace,
    notifications: NotificationHistory,
    sidebar_visible: bool,
    window_focused: bool,
    written_inputs: Vec<(u64, Vec<u8>)>,
    desktop_notifications: Vec<(u64, Option<String>, String)>,
    spawned: Vec<(u64, u64, SpawnOptions)>,
}

impl Default for HeadlessHost {
    fn default() -> Self {
        Self::new()
    }
}

impl HeadlessHost {
    /// Start a headless host with one initial tab and pane.
    pub fn new() -> Self {
        let mut host = Self {
            ws: Workspace::new(),
            notifications: NotificationHistory::default(),
            sidebar_visible: true,
            window_focused: false,
            written_inputs: Vec::new(),
            desktop_notifications: Vec::new(),
            spawned: Vec::new(),
        };
        host.new_tab_with(SpawnOptions::default());
        host
    }

    /// Dispatch a parsed [`Request`], returning `(Response, should_exit)`.
    pub fn dispatch(&mut self, req: Request) -> (Response, bool) {
        dispatch_request(self, req)
    }

    /// Parse and dispatch one raw wire line (JSON or minimal v1 text).
    pub fn dispatch_line(&mut self, line: &str) -> (Response, bool) {
        match parse_wire_line(line) {
            Ok(req) => dispatch_request(self, req),
            Err(resp) => (resp, false),
        }
    }

    /// Feed terminal bytes into pane `pane_id` (simulating program output).
    pub fn feed_pane_output(&mut self, pane_id: u64, bytes: &[u8]) {
        if let Some(pane) = self.ws.pane_mut(PaneId(pane_id)) {
            pane.terminal.process_bytes(bytes);
        }
    }

    /// Set whether the simulated window has focus (controls unseen attention / desktop notify).
    pub fn set_window_focused(&mut self, focused: bool) {
        self.window_focused = focused;
    }

    /// All input bytes sent to panes via `send_text` / `send_key`.
    pub fn written_inputs(&self) -> &[(u64, Vec<u8>)] {
        &self.written_inputs
    }

    /// Desktop notifications that would have been sent via `notify-send`.
    pub fn desktop_notifications(&self) -> &[(u64, Option<String>, String)] {
        &self.desktop_notifications
    }

    /// Recorded `(pane_id, tab_id, SpawnOptions)` for every pane created on this host.
    pub fn spawned(&self) -> &[(u64, u64, SpawnOptions)] {
        &self.spawned
    }

    pub fn tab_count(&self) -> usize {
        self.ws.tabs().len()
    }

    pub fn pane_count(&self) -> usize {
        self.ws.panes().count()
    }

    pub fn active_tab(&self) -> usize {
        self.ws.active()
    }

    pub fn focused_pane(&self) -> Option<u64> {
        self.ws.focused_id().map(|p| p.0)
    }

    pub fn dispatch_if_active(
        &mut self,
        req: Request,
        cancelled: &AtomicBool,
    ) -> Option<(Response, bool)> {
        dispatch_if_active(self, req, cancelled)
    }

    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    /// Read the per-pane [`AgentStatus`] stored on `pane_id`.
    pub fn pane_agent_status(&self, pane_id: u64) -> Option<AgentStatus> {
        self.ws.pane(PaneId(pane_id))?.meta.agent_status.clone()
    }

    /// Build sidebar [`Entry`] values exactly as the window renderer does.
    pub fn sidebar_entries(&self) -> Vec<Entry> {
        self.ws
            .tabs()
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let focused = self.ws.pane(tab.focused);
                let title = match tab.title.as_deref().filter(|t| !t.is_empty()) {
                    Some(custom) => clean_text(custom, MAX_TITLE_CHARS),
                    None => entry_title(focused.and_then(|p| p.title.as_deref()), None, index),
                };
                let mut notifications: Vec<_> = tab
                    .root
                    .leaves()
                    .into_iter()
                    .filter_map(|id| self.ws.pane(id)?.meta.attention.as_ref())
                    .collect();
                notifications.sort_by_key(|n| n.at);
                let focused_status = focused.and_then(|p| p.meta.agent_status.as_ref());
                let other_statuses: Vec<_> = tab
                    .root
                    .leaves()
                    .into_iter()
                    .filter(|&id| id != tab.focused)
                    .filter_map(|id| self.ws.pane(id)?.meta.agent_status.as_ref())
                    .collect();
                let agent_status = pick_tab_agent_status(focused_status, other_statuses);
                Entry::new(
                    &title,
                    focused.and_then(|p| p.meta.cwd.as_deref()),
                    focused.is_some_and(|p| p.meta.running),
                    focused.and_then(|p| p.meta.last_exit),
                    notifications
                        .into_iter()
                        .map(|n| Notice::new(n.title.as_deref(), &n.body)),
                )
                .with_agent_status(agent_status)
            })
            .collect()
    }
}

impl ControlHost for HeadlessHost {
    fn ws(&self) -> &Workspace {
        &self.ws
    }

    fn ws_mut(&mut self) -> &mut Workspace {
        &mut self.ws
    }

    fn notification_history(&self) -> &NotificationHistory {
        &self.notifications
    }

    fn notification_history_mut(&mut self) -> &mut NotificationHistory {
        &mut self.notifications
    }

    fn pane_cwd(&self, id: PaneId) -> Option<PathBuf> {
        self.ws.pane(id)?.meta.cwd.clone()
    }

    fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    fn set_sidebar_visible(&mut self, visible: bool) {
        self.sidebar_visible = visible;
    }

    fn toggle_sidebar(&mut self) -> bool {
        self.sidebar_visible = !self.sidebar_visible;
        self.sidebar_visible
    }

    fn new_tab_with(&mut self, options: SpawnOptions) -> Option<PaneId> {
        let mut factory = HeadlessFactory {
            options,
            tab_id: 0,
            spawned: Vec::new(),
        };
        let id = self.ws.new_tab(&HeadlessGrid, &mut factory)?;
        self.ws.apply_layout(&HeadlessGrid);
        for (pid, tid, opts) in factory.spawned {
            self.spawned.push((pid.0, tid, opts));
        }
        Some(id)
    }

    fn split_focused_with(
        &mut self,
        dir: SplitDir,
        options: SpawnOptions,
    ) -> Result<PaneId, SplitFailure> {
        if !self.ws.can_split_focused(&HeadlessGrid, dir) {
            return Err(self.ws.split_failure_reason(&HeadlessGrid, dir));
        }
        let mut factory = HeadlessFactory {
            options,
            tab_id: 0,
            spawned: Vec::new(),
        };
        let Some((_, new_id)) = self.ws.split_focused(&HeadlessGrid, &mut factory, dir) else {
            return Err(self.ws.split_failure_reason(&HeadlessGrid, dir));
        };
        self.ws.apply_layout(&HeadlessGrid);
        for (pid, tid, opts) in factory.spawned {
            self.spawned.push((pid.0, tid, opts));
        }
        Ok(new_id)
    }

    fn focus_pane_by_id(&mut self, id: PaneId) -> Result<usize, String> {
        let Some((tab, _, _)) = self.ws.focus_any_pane(id) else {
            return Err(format!("unknown pane: {}", id.0));
        };
        if self.window_focused {
            if let Some(pane) = self.ws.pane_mut(id) {
                pane.meta.attention = None;
            }
            self.notifications.mark_seen(id);
        }
        Ok(tab)
    }

    fn set_active_tab(&mut self, index: usize) {
        if self.ws.set_active_tab(index) && self.window_focused {
            if let Some(id) = self.ws.focused_id() {
                if let Some(pane) = self.ws.pane_mut(id) {
                    pane.meta.attention = None;
                }
                self.notifications.mark_seen(id);
            }
        }
    }

    fn close_pane(&mut self, id: PaneId) -> bool {
        let Some(closed) = self.ws.close_pane(id) else {
            return false;
        };
        if closed.focused.is_none() {
            return true;
        }
        self.ws.apply_layout(&HeadlessGrid);
        false
    }

    fn close_tab(&mut self, index: usize) -> bool {
        let Some((_, closed)) = self.ws.close_tab(index) else {
            return false;
        };
        if closed.focused.is_none() {
            return true;
        }
        self.ws.apply_layout(&HeadlessGrid);
        false
    }

    fn send_input(&mut self, id: PaneId, bytes: &[u8]) {
        self.written_inputs.push((id.0, bytes.to_vec()));
    }

    fn notify(&mut self, id: PaneId, title: Option<String>, body: String, send_desktop: bool) {
        let Some(tab_idx) = self.ws.tabs().iter().position(|t| t.root.contains(id)) else {
            return;
        };
        let tab_title = self.ws.tab_title(tab_idx);
        let seen = self.window_focused && self.ws.focused_id() == Some(id);
        self.notifications.push(
            id,
            title.as_deref(),
            &body,
            &tab_title,
            SystemTime::now(),
            seen,
        );
        let Some(notice) = crate::notify::unseen_notice(
            self.window_focused,
            self.ws.focused_id(),
            id,
            title.as_deref(),
            &body,
        ) else {
            return;
        };
        if let Some(pane) = self.ws.pane_mut(id) {
            pane.meta.attention = Some(Notification {
                title: notice.title.clone(),
                body: notice.body.clone(),
                at: SystemTime::now(),
            });
        }
        if send_desktop {
            self.desktop_notifications
                .push((id.0, notice.title, notice.body));
        }
    }

    fn request_redraw(&mut self) {}

    fn refresh_title(&mut self) {}

    fn command_list(&self) -> Value {
        Value::Array(crate::workflows::WorkflowSet::load(&[], None).control_list_json())
    }

    fn command_run(&mut self, name: &str, pane: Option<u64>) -> Result<(Value, bool), String> {
        let target = resolve_twitty_pane(self, pane)?;
        if let Some(action) = crate::commands::action(name) {
            match action {
                crate::input::InputAction::SplitRight => {
                    self.focus_pane_by_id(target)?;
                    self.split_focused_with(SplitDir::Horizontal, SpawnOptions::default())
                        .map_err(|err| err.message)?;
                }
                crate::input::InputAction::SplitDown => {
                    self.focus_pane_by_id(target)?;
                    self.split_focused_with(SplitDir::Vertical, SpawnOptions::default())
                        .map_err(|err| err.message)?;
                }
                _ => {}
            }
            Ok((json!({ "command": name, "pane": target.0 }), false))
        } else {
            Err(format!("unknown command: {name}"))
        }
    }
}
