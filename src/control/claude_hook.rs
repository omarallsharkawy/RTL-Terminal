// Adapted from cmux-gtk cmux/bin/cmux (AGPL-3.0-or-later,
// Copyright (c) 2026 Douglas Soares de Andrade). Changes: implemented in Rust as
// `twitty claude-hook <event>` with bounded stdin JSON parsing, Twitty text cleaning,
// Arabic-preserving messages, and native v2 socket calls.

//! Claude Code hook handler (`twitty claude-hook <event>`): reads a bounded JSON payload from
//! stdin and translates lifecycle events into Twitty sidebar status and notification calls.

use std::io::{self, Read};

use serde_json::{json, Map, Value};

use crate::shell_integration::{clean_text, MAX_NOTIFY_CHARS};

/// Maximum number of bytes read from hook stdin (64 KiB).
pub const MAX_HOOK_STDIN_BYTES: usize = 64 * 1024;

/// One control socket RPC call produced by a Claude Code hook event.
#[derive(Debug, Clone, PartialEq)]
pub struct HookCall {
    pub method: String,
    pub params: Value,
}

/// Read at most `max_bytes` from `reader` and convert lossily to UTF-8 so a huge or truncated
/// hook payload can never exhaust memory or fail on a split multi-byte character.
pub fn read_bounded_stdin<R: Read>(reader: R, max_bytes: usize) -> io::Result<String> {
    let mut buf = Vec::new();
    reader.take(max_bytes as u64).read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Translate a Claude Code hook `event` name and its raw stdin JSON payload into the ordered
/// control socket calls Twitty should execute for `pane`.
///
/// Unknown events return an empty list (ignored cleanly).
pub fn plan_hook_calls(event: &str, stdin_json: &str, pane: Option<u64>) -> Vec<HookCall> {
    let payload: Value = serde_json::from_str(stdin_json.trim()).unwrap_or(Value::Null);
    let normalized = event.trim().to_ascii_lowercase();

    match normalized.as_str() {
        "session-start" | "session_start" => {
            let mut p = target_map(pane);
            p.insert("key".into(), json!("claude"));
            p.insert("value".into(), json!("working"));
            p.insert("label".into(), json!("Working"));
            vec![HookCall {
                method: "workspace.set_status".into(),
                params: Value::Object(p),
            }]
        }
        "prompt-submit" | "prompt_submit" | "user-prompt-submit" | "user_prompt_submit"
        | "pre-tool-use" | "pre_tool_use" => {
            let mut clear = target_map(pane);
            clear.insert("key".into(), json!("needs-input"));

            let mut set = target_map(pane);
            set.insert("key".into(), json!("claude"));
            set.insert("value".into(), json!("working"));
            set.insert("label".into(), json!("Working"));

            vec![
                HookCall {
                    method: "workspace.clear_status".into(),
                    params: Value::Object(clear),
                },
                HookCall {
                    method: "workspace.set_status".into(),
                    params: Value::Object(set),
                },
            ]
        }
        "notification" => {
            let title = extract_clean(&payload, &["title"]).unwrap_or_else(|| "Claude".into());
            let body = extract_clean(&payload, &["message", "body"])
                .or_else(|| extract_clean(&payload, &["notification_type"]))
                .unwrap_or_else(|| "Needs input".into());

            let mut set = target_map(pane);
            set.insert("key".into(), json!("claude"));
            set.insert("value".into(), json!("needs_input"));
            set.insert("label".into(), json!(body));

            let mut notify = target_map(pane);
            notify.insert("title".into(), json!(title));
            notify.insert("body".into(), json!(body));
            notify.insert("send_desktop".into(), json!(true));

            vec![
                HookCall {
                    method: "workspace.set_status".into(),
                    params: Value::Object(set),
                },
                HookCall {
                    method: "notification.create".into(),
                    params: Value::Object(notify),
                },
            ]
        }
        "stop" => {
            let body = extract_clean(
                &payload,
                &["last_assistant_message", "message", "body", "reason"],
            )
            .unwrap_or_else(|| "Done".into());

            let mut set = target_map(pane);
            set.insert("key".into(), json!("claude"));
            set.insert("value".into(), json!("done"));
            set.insert("label".into(), json!(body));

            let mut notify = target_map(pane);
            notify.insert("title".into(), json!("Claude"));
            notify.insert("body".into(), json!(body));
            notify.insert("send_desktop".into(), json!(true));

            vec![
                HookCall {
                    method: "workspace.set_status".into(),
                    params: Value::Object(set),
                },
                HookCall {
                    method: "notification.create".into(),
                    params: Value::Object(notify),
                },
            ]
        }
        "session-end" | "session_end" => {
            let mut clear_claude = target_map(pane);
            clear_claude.insert("key".into(), json!("claude"));

            let mut clear_input = target_map(pane);
            clear_input.insert("key".into(), json!("needs-input"));

            vec![
                HookCall {
                    method: "workspace.clear_status".into(),
                    params: Value::Object(clear_claude),
                },
                HookCall {
                    method: "workspace.clear_status".into(),
                    params: Value::Object(clear_input),
                },
            ]
        }
        _ => Vec::new(),
    }
}

fn target_map(pane: Option<u64>) -> Map<String, Value> {
    let mut map = Map::new();
    if let Some(p) = pane {
        map.insert("surface".into(), json!(p.to_string()));
    }
    map
}

fn extract_clean(payload: &Value, keys: &[&str]) -> Option<String> {
    for &key in keys {
        if let Some(raw) = payload.get(key).and_then(Value::as_str) {
            let cleaned = clean_text(raw, MAX_NOTIFY_CHARS);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    None
}

/// Execute `twitty claude-hook <event>` against the running Twitty control socket.
#[cfg(unix)]
pub fn run_cli_hook(event: &str, pane: Option<u64>) -> i32 {
    use std::io::IsTerminal;

    use crate::control::client::Client;

    let stdin_payload = if io::stdin().is_terminal() {
        String::new()
    } else {
        read_bounded_stdin(io::stdin(), MAX_HOOK_STDIN_BYTES).unwrap_or_default()
    };

    let calls = plan_hook_calls(event, &stdin_payload, pane);
    if calls.is_empty() {
        return 0;
    }

    let mut client = match Client::connect_default() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("twitty: {err}");
            return 1;
        }
    };

    for call in calls {
        match client.call(call.method, call.params) {
            Ok(resp) if resp.ok => {}
            Ok(resp) => {
                eprintln!(
                    "twitty: {}",
                    resp.error.unwrap_or_else(|| "hook call failed".into())
                );
                return 1;
            }
            Err(err) => {
                eprintln!("twitty: {err}");
                return 1;
            }
        }
    }
    0
}
