//! cmux v2 workspace, pane, surface, and notification method handlers.

use std::path::PathBuf;
use std::time::SystemTime;

use alacritty_terminal::term::TermMode;
use serde_json::{json, Map, Value};

use super::keys::{encode_key_name, MAX_KEY_MODS};
use super::{ControlHost, MAX_CWD_BYTES, MAX_TITLE_CHARS};
use crate::control::protocol::{
    bound_read_text, Response, DEFAULT_READ_LINES, MAX_COMMAND_ARGS, MAX_COMMAND_BYTES,
    MAX_READ_BYTES, MAX_READ_LINES, MAX_SEND_TEXT_BYTES,
};
use crate::layout::{PaneId, SplitDir};
use crate::pty::SpawnOptions;
use crate::session::pane::Pane;
use crate::shell_integration::{clean_text, MAX_NOTIFY_CHARS};

// ---------------------------------------------------------------------------
// cmux v2 handlers
// ---------------------------------------------------------------------------

pub(super) fn handle_workspace_list(host: &impl ControlHost, id: Value) -> Response {
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
                "color": tab.color.map(|c| c.as_str()),
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

pub(super) fn handle_workspace_current(host: &impl ControlHost, id: Value) -> Response {
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
            "color": tab.color.map(|c| c.as_str()),
            "directory": cwd,
            "panel_count": tab.root.leaves().len(),
            "focused_panel_id": tab.focused.0.to_string(),
        }),
    )
}

pub(super) fn handle_workspace_create(
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

pub(super) fn handle_workspace_select(
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

pub(super) fn handle_workspace_close(
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

pub(super) fn handle_workspace_rename(
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

pub(super) fn handle_workspace_set_color(
    host: &mut impl ControlHost,
    id: Value,
    params: &Map<String, Value>,
) -> Response {
    let target_index = match resolve_workspace_index(host, params) {
        Ok(idx) => idx,
        Err((code, msg)) => return Response::cmux_err(id, code, msg),
    };
    let color = match params.get("color") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty()
                || trimmed.eq_ignore_ascii_case("none")
                || trimmed.eq_ignore_ascii_case("clear")
            {
                None
            } else if let Some(c) = twitty_core::color::WorkspaceColor::from_name(trimmed) {
                Some(c)
            } else {
                return Response::cmux_err(
                    id,
                    "invalid_params",
                    "Unknown color; use blue, green, amber, red, purple, teal, or none",
                );
            }
        }
        Some(_) => {
            return Response::cmux_err(
                id,
                "invalid_params",
                "'color' must be a color name string or null",
            );
        }
    };
    host.ws_mut().set_tab_color(target_index, color);
    let tab_id = host.ws().tabs()[target_index].id;
    host.request_redraw();
    Response::ok(
        id,
        json!({
            "ok": true,
            "workspace_id": tab_id.to_string(),
            "color": color.map(|c| c.as_str()),
        }),
    )
}

pub(super) fn handle_workspace_set_status(
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

pub(super) fn handle_workspace_clear_status(
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

pub(super) fn handle_workspace_list_status(
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

pub(super) fn handle_cmux_pane_list(
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

pub(super) fn handle_surface_current(host: &impl ControlHost, id: Value) -> Response {
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

pub(super) fn handle_cmux_split(
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

pub(super) fn handle_cmux_pane_focus(
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

pub(super) fn handle_cmux_pane_close(
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

pub(super) fn handle_surface_send_text(
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

pub(super) fn handle_surface_send_key(
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

pub(super) fn handle_surface_read_text(
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

pub(super) fn handle_notification_create(
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

pub(super) fn handle_notification_list(
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

pub(super) fn handle_notification_clear(host: &mut impl ControlHost, id: Value) -> Response {
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
