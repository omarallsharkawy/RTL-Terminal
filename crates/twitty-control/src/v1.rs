// Adapted from cmux-gtk cmux/src/socket/v2/{mod,system,workspace,pane,surface,notification,sidebar,helpers}.rs
// and cmux/src/socket/v1.rs (AGPL-3.0-or-later, Copyright (c) 2026 Douglas Soares de Andrade).
// Changes: mapped cmux v2 methods and minimal Claude-hook v1 text commands onto Twitty's
// Workspace/Pane/NotificationHistory model via ControlHost, kept Twitty's ctl methods intact,
// enforced Twitty text cleaning and size limits, and added a window-free HeadlessHost.

//! The minimal cmux v1 text protocol used by the Claude hook.

use std::collections::HashMap;

use serde_json::{json, Value};
use twitty_core::shell_integration::clean_text;

use crate::protocol::{Request, Response, MAX_METHOD_LEN};

const MAX_V1_TOKENS: usize = 64;

/// Parse a non-JSON line using the minimal cmux v1 text protocol needed by the Claude hook path:
/// `ping`, `set_status`, `clear_status`, `list_status`, `notify`, `notify_surface`, `notify_target`.
pub fn parse_v1_line(line: &str) -> Result<Request, Response> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err(Response::err(Value::Null, "empty request"));
    }
    let (command, rest) = match trimmed.split_once(char::is_whitespace) {
        Some((cmd, r)) => (cmd, r.trim()),
        None => (trimmed, ""),
    };
    let (args, flags) = parse_v1_args(rest);
    let workspace = flags.get("tab").or_else(|| flags.get("workspace"));
    let surface = flags.get("panel").or_else(|| flags.get("surface"));

    let (method, mut params) = match command {
        "ping" => ("system.ping", json!({})),
        "set_status" => {
            let key = args.first().map(String::as_str).unwrap_or("");
            let value = args.get(1).map(String::as_str).unwrap_or("");
            let mut p = json!({ "key": key, "value": value });
            if args.len() > 2 {
                let label = args[2..].join(" ");
                if !label.is_empty() {
                    p["label"] = json!(label);
                }
            }
            if let Some(icon) = flags.get("icon") {
                p["icon"] = json!(icon);
            }
            if let Some(color) = flags.get("color") {
                p["color"] = json!(color);
            }
            ("workspace.set_status", p)
        }
        "clear_status" => {
            let mut p = json!({});
            if let Some(key) = args.first() {
                p["key"] = json!(key);
            }
            ("workspace.clear_status", p)
        }
        "list_status" => ("workspace.list_status", json!({})),
        "notify" => {
            let title = args.first().map(String::as_str).unwrap_or("");
            let body = args.get(1).map(String::as_str).unwrap_or("");
            (
                "notification.create",
                json!({ "title": title, "body": body }),
            )
        }
        "notify_surface" => {
            let sid = args.first().map(String::as_str).unwrap_or("");
            let title = args.get(1).map(String::as_str).unwrap_or("");
            let body = args.get(2).map(String::as_str).unwrap_or("");
            (
                "notification.create",
                json!({ "surface": sid, "title": title, "body": body }),
            )
        }
        "notify_target" => {
            let ws_arg = args.first().map(String::as_str).unwrap_or("");
            let sid = args.get(1).map(String::as_str).unwrap_or("");
            let title = args.get(2).map(String::as_str).unwrap_or("");
            let body = args.get(3).map(String::as_str).unwrap_or("");
            (
                "notification.create",
                json!({
                    "workspace": ws_arg,
                    "surface": sid,
                    "title": title,
                    "body": body,
                }),
            )
        }
        other => {
            return Err(Response::cmux_err(
                Value::Null,
                "unknown_method",
                format!("Unknown command: {}", clean_text(other, MAX_METHOD_LEN)),
            ));
        }
    };

    if let Some(ws) = workspace {
        if params.get("workspace").is_none() {
            params["workspace"] = json!(ws);
        }
    }
    if let Some(s) = surface {
        if params.get("surface").is_none() {
            params["surface"] = json!(s);
        }
    }

    Ok(Request {
        id: Value::String("v1".into()),
        method: method.into(),
        params,
    })
}

/// Parse `"arg1 \"quoted arg\" --flag=value"` into `(positional_args, flags)`, bounded to
/// [`MAX_V1_TOKENS`].
fn parse_v1_args(input: &str) -> (Vec<String>, HashMap<String, String>) {
    let mut args = Vec::new();
    let mut flags = HashMap::new();
    let mut chars = input.chars().peekable();

    while chars.peek().is_some() && args.len() + flags.len() < MAX_V1_TOKENS {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.peek().is_none() {
            break;
        }

        let mut token = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(ch) = chars.next() {
                if ch == '"' {
                    break;
                }
                if ch == '\\' {
                    if let Some(&next) = chars.peek() {
                        if matches!(next, '"' | '\\') {
                            token.push(next);
                            chars.next();
                            continue;
                        }
                    }
                }
                token.push(ch);
            }
        } else {
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() {
                    break;
                }
                token.push(ch);
                chars.next();
            }
        }

        if let Some(flag) = token.strip_prefix("--") {
            if let Some((key, value)) = flag.split_once('=') {
                flags.insert(key.to_owned(), value.to_owned());
            } else {
                flags.insert(flag.to_owned(), String::new());
            }
        } else {
            args.push(token);
        }
    }

    (args, flags)
}
