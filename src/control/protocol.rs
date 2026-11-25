//! Wire protocol for the control socket: newline-delimited JSON requests and responses, bounded
//! line reading, and typed parameter parsing with strict size limits.

use std::io::{self, BufRead, Read};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::layout::SplitDir;
use crate::remote::{validate_destination, validate_name};
use crate::shell_integration::{clean_text, MAX_NOTIFY_CHARS};

/// Maximum length of one NDJSON line (excluding the trailing newline), in bytes (1 MiB).
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Maximum payload accepted by `pane.send-text`, in bytes (64 KiB).
pub const MAX_SEND_TEXT_BYTES: usize = 64 * 1024;

/// Default number of lines returned by `pane.read`.
pub const DEFAULT_READ_LINES: usize = 50;

/// Maximum number of lines returned by `pane.read`.
pub const MAX_READ_LINES: usize = 2000;

/// Maximum UTF-8 bytes returned by `pane.read` so responses always stay well below
/// `MAX_LINE_BYTES` even after JSON escaping.
pub const MAX_READ_BYTES: usize = 256 * 1024;

/// Keep at most `max_bytes` from the tail of `text`, aligned to a UTF-8 character boundary.
pub fn bound_read_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

/// Maximum number of argv entries accepted by `tab.new` / `pane.split`.
pub const MAX_COMMAND_ARGS: usize = 1024;

/// Maximum combined byte length of argv entries accepted by `tab.new` / `pane.split` (64 KiB).
pub const MAX_COMMAND_BYTES: usize = 64 * 1024;

/// Maximum byte length of a `cwd` path string.
const MAX_CWD_BYTES: usize = 4096;

/// One JSON request line from a control client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    #[serde(default)]
    pub id: Value,
    pub method: String,
    #[serde(default = "default_params")]
    pub params: Value,
}

fn default_params() -> Value {
    Value::Object(Map::new())
}

/// One JSON response line sent back to a control client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub id: Value,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn err(id: Value, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(message.into()),
        }
    }
}

/// Result of reading one bounded newline-delimited line.
#[derive(Debug, PartialEq, Eq)]
pub enum ReadLine {
    Line(String),
    Eof,
    TooLong,
}

/// Read a single UTF-8 line up to `max_bytes` (not counting a trailing `\r\n` or `\n`).
/// Never buffers more than `max_bytes + 1` bytes from `reader`.
pub fn read_bounded_line<R: BufRead>(reader: &mut R, max_bytes: usize) -> io::Result<ReadLine> {
    let mut buf = Vec::new();
    let mut limited = reader.take(max_bytes.saturating_add(1) as u64);
    let n = limited.read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(ReadLine::Eof);
    }
    if buf.len() > max_bytes {
        // Only fits if the extra byte is the terminating newline itself.
        if buf.last() != Some(&b'\n') {
            return Ok(ReadLine::TooLong);
        }
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    }
    if buf.len() > max_bytes {
        return Ok(ReadLine::TooLong);
    }
    String::from_utf8(buf)
        .map(ReadLine::Line)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Parse one raw request line into a [`Request`], returning a ready error [`Response`] on failure.
pub fn parse_request_line(line: &str) -> Result<Request, Response> {
    if line.trim().is_empty() {
        return Err(Response::err(Value::Null, "empty request"));
    }
    let raw: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(err) => return Err(Response::err(Value::Null, format!("invalid JSON: {err}"))),
    };
    let id = raw.get("id").cloned().unwrap_or(Value::Null);
    let Some(obj) = raw.as_object() else {
        return Err(Response::err(id, "request must be a JSON object"));
    };
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        return Err(Response::err(id, "missing or invalid \"method\" field"));
    };
    if method.is_empty() {
        return Err(Response::err(id, "\"method\" must not be empty"));
    }
    let params = match obj.get("params") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(v @ Value::Object(_)) => v.clone(),
        Some(_) => return Err(Response::err(id, "\"params\" must be a JSON object")),
    };
    Ok(Request {
        id,
        method: method.to_owned(),
        params,
    })
}

/// Validated, typed representation of every supported control method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MethodCall {
    SystemPing,
    CommandList,
    CommandRun {
        name: String,
        pane: Option<u64>,
    },
    TabList,
    PaneList,
    PaneFocus {
        pane: u64,
    },
    PaneSendText {
        pane: Option<u64>,
        text: String,
    },
    PaneRead {
        pane: Option<u64>,
        lines: usize,
    },
    TabNew {
        cwd: Option<PathBuf>,
        command: Vec<String>,
    },
    TabNewSsh {
        destination: String,
        name: Option<String>,
    },
    PaneSplit {
        pane: Option<u64>,
        direction: SplitDir,
        cwd: Option<PathBuf>,
        command: Vec<String>,
    },
    Notify {
        pane: Option<u64>,
        title: Option<String>,
        body: String,
    },
}

impl MethodCall {
    /// Validate `req.method` and `req.params` into a typed call.
    pub fn parse(req: &Request) -> Result<Self, String> {
        let params = match &req.params {
            Value::Null => None,
            Value::Object(map) => Some(map),
            _ => return Err("\"params\" must be a JSON object".into()),
        };
        let empty = Map::new();
        let params = params.unwrap_or(&empty);

        match req.method.as_str() {
            "system.ping" => Ok(Self::SystemPing),
            "command.list" => Ok(Self::CommandList),
            "command.run" => {
                let name = required_str(params, "name")?.to_owned();
                if name.is_empty() {
                    return Err("\"name\" must not be empty".into());
                }
                let pane = optional_u64(params, "pane")?;
                Ok(Self::CommandRun { name, pane })
            }
            "tab.list" => Ok(Self::TabList),
            "pane.list" => Ok(Self::PaneList),
            "pane.focus" => {
                let pane = required_u64(params, "pane")?;
                Ok(Self::PaneFocus { pane })
            }
            "pane.send-text" => {
                let pane = optional_u64(params, "pane")?;
                let text = required_str(params, "text")?;
                if text.len() > MAX_SEND_TEXT_BYTES {
                    return Err(format!(
                        "\"text\" exceeds maximum size of {MAX_SEND_TEXT_BYTES} bytes"
                    ));
                }
                Ok(Self::PaneSendText {
                    pane,
                    text: text.to_owned(),
                })
            }
            "pane.read" => {
                let pane = optional_u64(params, "pane")?;
                let lines = match optional_u64(params, "lines")? {
                    None => DEFAULT_READ_LINES,
                    Some(n) if n > MAX_READ_LINES as u64 => {
                        return Err(format!("\"lines\" exceeds maximum of {MAX_READ_LINES}"));
                    }
                    Some(n) => n as usize,
                };
                Ok(Self::PaneRead { pane, lines })
            }
            "tab.new" => {
                let cwd = optional_cwd(params)?;
                let command = optional_command(params)?;
                Ok(Self::TabNew { cwd, command })
            }
            "tab.new-ssh" => {
                let destination = required_str(params, "destination")?;
                let name = optional_str(params, "name")?;
                validate_destination(destination)?;
                if let Some(name) = name {
                    validate_name(name)?;
                }
                Ok(Self::TabNewSsh {
                    destination: destination.into(),
                    name: name.map(str::to_owned),
                })
            }
            "pane.split" => {
                let pane = optional_u64(params, "pane")?;
                let direction = match required_str(params, "direction")? {
                    "right" => SplitDir::Horizontal,
                    "down" => SplitDir::Vertical,
                    other => {
                        return Err(format!(
                            "invalid \"direction\" {other:?}: expected \"right\" or \"down\""
                        ))
                    }
                };
                let cwd = optional_cwd(params)?;
                let command = optional_command(params)?;
                Ok(Self::PaneSplit {
                    pane,
                    direction,
                    cwd,
                    command,
                })
            }
            "notify" => {
                let pane = optional_u64(params, "pane")?;
                let title = optional_str(params, "title")?
                    .map(|t| clean_text(t, MAX_NOTIFY_CHARS))
                    .filter(|t| !t.is_empty());
                let body = clean_text(required_str(params, "body")?, MAX_NOTIFY_CHARS);
                if title.is_none() && body.is_empty() {
                    return Err("notification \"body\" must not be empty".into());
                }
                Ok(Self::Notify { pane, title, body })
            }
            other => Err(format!("unknown method: {other}")),
        }
    }
}

fn required_str<'a>(params: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    match params.get(key) {
        Some(Value::String(s)) => Ok(s.as_str()),
        Some(_) => Err(format!("\"{key}\" must be a string")),
        None => Err(format!("missing required parameter \"{key}\"")),
    }
}

fn optional_str<'a>(params: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(format!("\"{key}\" must be a string")),
    }
}

fn required_u64(params: &Map<String, Value>, key: &str) -> Result<u64, String> {
    match params.get(key) {
        Some(v) => v
            .as_u64()
            .ok_or_else(|| format!("\"{key}\" must be a non-negative integer")),
        None => Err(format!("missing required parameter \"{key}\"")),
    }
}

fn optional_u64(params: &Map<String, Value>, key: &str) -> Result<Option<u64>, String> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("\"{key}\" must be a non-negative integer")),
    }
}

fn optional_cwd(params: &Map<String, Value>) -> Result<Option<PathBuf>, String> {
    let Some(raw) = optional_str(params, "cwd")? else {
        return Ok(None);
    };
    if raw.is_empty() {
        return Err("\"cwd\" must not be empty".into());
    }
    if raw.len() > MAX_CWD_BYTES {
        return Err(format!(
            "\"cwd\" exceeds maximum length of {MAX_CWD_BYTES} bytes"
        ));
    }
    if raw.chars().any(|c| c.is_control()) {
        return Err("\"cwd\" must not contain control characters".into());
    }
    Ok(Some(PathBuf::from(raw)))
}

fn optional_command(params: &Map<String, Value>) -> Result<Vec<String>, String> {
    let Some(val) = params.get("command") else {
        return Ok(Vec::new());
    };
    if val.is_null() {
        return Ok(Vec::new());
    }
    let Some(arr) = val.as_array() else {
        return Err("\"command\" must be an array of strings".into());
    };
    if arr.is_empty() {
        return Err("\"command\" must not be empty".into());
    }
    if arr.len() > MAX_COMMAND_ARGS {
        return Err(format!(
            "\"command\" exceeds maximum of {MAX_COMMAND_ARGS} arguments"
        ));
    }
    let mut out = Vec::with_capacity(arr.len());
    let mut total_bytes = 0usize;
    for item in arr {
        let Some(s) = item.as_str() else {
            return Err("\"command\" entries must be strings".into());
        };
        if s.contains('\0') {
            return Err("\"command\" entries must not contain NUL bytes".into());
        }
        total_bytes = total_bytes.saturating_add(s.len());
        if total_bytes > MAX_COMMAND_BYTES {
            return Err(format!(
                "\"command\" exceeds maximum total size of {MAX_COMMAND_BYTES} bytes"
            ));
        }
        out.push(s.to_owned());
    }
    if out[0].is_empty() {
        return Err("\"command\" program name must not be empty".into());
    }
    Ok(out)
}
