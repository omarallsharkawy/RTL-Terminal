//! Parsing and validation of custom command lists from `config.json` and `.twitty/commands.json`.

use serde::Deserialize;
use serde_json::Value;

use crate::config::warn;
use crate::shell_integration::clean_text;

use super::{
    CommandMode, WorkflowSpec, MAX_COMMANDS_PER_SOURCE, MAX_COMMAND_CHARS, MAX_CWD_CHARS,
    MAX_DESCRIPTION_CHARS, MAX_NAME_CHARS,
};

/// Parse a project `.twitty/commands.json` file body.
///
/// Accepts either a JSON array `[ { ... }, ... ]` or a JSON object `{ "commands": [ ... ] }`
/// (ignoring unknown top-level and per-entry fields). Invalid entries are skipped with a
/// `config::warn`; at most [`MAX_COMMANDS_PER_SOURCE`] entries are kept.
pub fn parse_project_bytes(bytes: &[u8], source_label: &str) -> Result<Vec<WorkflowSpec>, String> {
    let text = std::str::from_utf8(bytes).map_err(|err| {
        let msg = format!("twitty: ignoring {source_label} (not valid UTF-8: {err})");
        warn(msg.clone());
        msg
    })?;
    let root: Value = serde_json::from_str(text).map_err(|err| {
        let msg = format!("twitty: ignoring {source_label} (invalid JSON: {err})");
        warn(msg.clone());
        msg
    })?;
    let array = match &root {
        Value::Array(items) => items.as_slice(),
        Value::Object(map) => match map.get("commands") {
            Some(Value::Array(items)) => items.as_slice(),
            _ => {
                let msg = format!(
                    "twitty: ignoring {source_label} (expected a JSON array or an object with a \"commands\" array)"
                );
                warn(msg.clone());
                return Err(msg);
            }
        },
        _ => {
            let msg = format!(
                "twitty: ignoring {source_label} (expected a JSON array or an object with a \"commands\" array)"
            );
            warn(msg.clone());
            return Err(msg);
        }
    };
    Ok(parse_entries_slice(array, source_label))
}

/// Parse an array of raw JSON command entries, skipping invalid ones with `config::warn`.
pub fn parse_entries_slice(values: &[Value], source_label: &str) -> Vec<WorkflowSpec> {
    let mut out = Vec::with_capacity(values.len().min(MAX_COMMANDS_PER_SOURCE));
    if values.len() > MAX_COMMANDS_PER_SOURCE {
        warn(format!(
            "twitty: {source_label} has {} commands; keeping at most {MAX_COMMANDS_PER_SOURCE}",
            values.len()
        ));
    }
    for (index, value) in values.iter().enumerate() {
        if out.len() >= MAX_COMMANDS_PER_SOURCE {
            break;
        }
        match parse_entry(value) {
            Ok(spec) => out.push(spec),
            Err(reason) => {
                warn(format!(
                    "twitty: skipping {source_label}[{index}]: {reason}"
                ));
            }
        }
    }
    out
}

pub fn parse_entry(value: &Value) -> Result<WorkflowSpec, String> {
    let Some(obj) = value.as_object() else {
        return Err("entry must be a JSON object".into());
    };

    let raw_name = match obj.get("name") {
        Some(Value::String(s)) => s.as_str(),
        Some(_) => return Err("\"name\" must be a string".into()),
        None => return Err("missing required field \"name\"".into()),
    };
    let name = clean_text(raw_name, MAX_NAME_CHARS);
    if name.is_empty() {
        return Err("\"name\" must not be empty".into());
    }

    let name_ar = match obj.get("name_ar") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => clean_text(s, MAX_NAME_CHARS),
        Some(_) => return Err("\"name_ar\" must be a string".into()),
    };

    let raw_command = match obj.get("command") {
        Some(Value::String(s)) => s.as_str(),
        Some(_) => return Err("\"command\" must be a string".into()),
        None => return Err("missing required field \"command\"".into()),
    };
    let command = clean_text(raw_command, MAX_COMMAND_CHARS);
    if command.is_empty() {
        return Err("\"command\" must not be empty".into());
    }

    let mode = match obj.get("mode") {
        None | Some(Value::Null) => CommandMode::Insert,
        Some(Value::String(s)) => match s.as_str() {
            "insert" => CommandMode::Insert,
            "run" => CommandMode::Run,
            other => {
                return Err(format!(
                    "invalid \"mode\" {other:?} (expected \"insert\" or \"run\")"
                ));
            }
        },
        Some(_) => return Err("\"mode\" must be a string".into()),
    };

    let cwd = match obj.get("cwd") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            if s.chars().any(char::is_control) {
                return Err("\"cwd\" must not contain control characters".into());
            }
            let cleaned = clean_text(s, MAX_CWD_CHARS);
            (!cleaned.is_empty()).then_some(cleaned)
        }
        Some(_) => return Err("\"cwd\" must be a string".into()),
    };

    let description = match obj.get("description") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let cleaned = clean_text(s, MAX_DESCRIPTION_CHARS);
            (!cleaned.is_empty()).then_some(cleaned)
        }
        Some(_) => return Err("\"description\" must be a string".into()),
    };

    Ok(WorkflowSpec {
        name,
        name_ar,
        command,
        mode,
        cwd,
        description,
    })
}

/// Serde deserializer for `TwittyConfig::commands`: bad entries are skipped with `config::warn`
/// instead of discarding the entire `config.json`.
pub fn deserialize_config_commands<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<WorkflowSpec>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => Ok(parse_entries_slice(&items, "config.commands")),
        _ => {
            warn("twitty: ignoring invalid config.commands (expected a JSON array)".into());
            Ok(Vec::new())
        }
    }
}
