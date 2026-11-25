//! Hand-written CLI argument parser and runner for `twitty ctl`, `notify`, `send`, `list`, and
//! `read`.

use serde_json::{json, Map, Value};

use crate::remote::{validate_destination, validate_name};

/// Parsed command-line action.
#[derive(Debug, Clone, PartialEq)]
pub enum CliAction {
    /// No CLI subcommand: start the terminal window.
    RunGui,
    Help,
    Version,
    Ssh {
        destination: String,
        name: Option<String>,
    },
    Ctl {
        method: String,
        params: Value,
    },
    Notify {
        pane: Option<u64>,
        title: Option<String>,
        body: String,
    },
    Send {
        pane: Option<u64>,
        text: String,
    },
    List,
    Read {
        pane: Option<u64>,
        lines: Option<usize>,
    },
    ClaudeHook {
        event: String,
        pane: Option<u64>,
    },
}

/// Usage help printed by `twitty --help`.
pub fn help_text() -> &'static str {
    concat!(
        "Twitty - Native Bidirectional RTL Terminal Emulator\n",
        "\n",
        "USAGE:\n",
        "    twitty                                  Launch the terminal window\n",
        "    twitty ctl <method> [JSON params]       Send a raw JSON request, print response JSON\n",
        "    twitty notify [--pane N] [--title T] BODY\n",
        "                                            Send a notification to a pane\n",
        "    twitty send [--pane N] TEXT             Type TEXT into a pane's PTY\n",
        "    twitty list                             List tabs and panes\n",
        "    twitty read [--pane N] [--lines N]      Read recent lines from a pane\n",
        "    twitty claude-hook <event> [--pane N]   Handle a Claude Code hook (JSON on stdin)\n",
        "    twitty ssh <destination> [--name NAME]  Open a persistent SSH tab\n",
        "    twitty --help | -h                      Show this help\n",
        "    twitty --version | -V                   Show version\n",
        "\n",
        "ENVIRONMENT:\n",
        "    TWITTY_SOCKET    Path to the control socket (default: newest live socket)\n",
        "    TWITTY_PANE_ID   Default pane id for --pane when unset\n",
        "    CMUX_SOCKET_PATH Alternate control socket path (cmux compatibility)\n",
        "    CMUX_SURFACE_ID  Alternate default pane id (cmux compatibility)\n",
        "    TWITTY_SHELL     Override the default shell\n",
        "    TWITTY_LOG       Log level (error, warn, info, debug, trace)\n",
    )
}

/// Parse CLI arguments (excluding argv[0]). `env_pane` is `$TWITTY_PANE_ID` if present.
pub fn parse_args(args: &[String], env_pane: Option<&str>) -> Result<CliAction, String> {
    let Some((first, rest)) = args.split_first() else {
        return Ok(CliAction::RunGui);
    };
    let default_pane = env_pane
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<u64>().ok());

    match first.as_str() {
        "-h" | "--help" | "help" => Ok(CliAction::Help),
        "-V" | "--version" => Ok(CliAction::Version),
        "ctl" => parse_ctl(rest),
        "notify" => parse_notify(rest, default_pane),
        "send" => parse_send(rest, default_pane),
        "list" => parse_list(rest),
        "read" => parse_read(rest, default_pane),
        "claude-hook" => parse_claude_hook(rest, default_pane),
        "ssh" => parse_ssh(rest),
        other => Err(format!("unknown argument {other:?} (try `twitty --help`)")),
    }
}

fn parse_ssh(args: &[String]) -> Result<CliAction, String> {
    let (destination, name) = match args {
        [help] if matches!(help.as_str(), "-h" | "--help") => return Ok(CliAction::Help),
        [destination] => (destination, None),
        [destination, flag, name] if flag == "--name" => (destination, Some(name)),
        _ => return Err("usage: twitty ssh <destination> [--name NAME]".into()),
    };
    validate_destination(destination)?;
    if let Some(name) = name {
        validate_name(name)?;
    }
    Ok(CliAction::Ssh {
        destination: destination.clone(),
        name: name.cloned(),
    })
}

fn parse_ctl(args: &[String]) -> Result<CliAction, String> {
    if args.is_empty() {
        return Err("usage: twitty ctl <method> [JSON params]".into());
    }
    if matches!(args[0].as_str(), "-h" | "--help") {
        return Ok(CliAction::Help);
    }
    let method = args[0].clone();
    let params = match args.len() {
        1 => Value::Object(Map::new()),
        2 => {
            let v: Value =
                serde_json::from_str(&args[1]).map_err(|e| format!("invalid JSON params: {e}"))?;
            if !v.is_object() {
                return Err("JSON params must be an object".into());
            }
            v
        }
        _ => return Err("too many arguments: usage: twitty ctl <method> [JSON params]".into()),
    };
    Ok(CliAction::Ctl { method, params })
}

fn parse_notify(args: &[String], default_pane: Option<u64>) -> Result<CliAction, String> {
    let mut pane = default_pane;
    let mut title: Option<String> = None;
    let mut positional: Vec<&str> = Vec::new();
    let mut i = 0;
    let mut after_sep = false;

    while i < args.len() {
        let arg = args[i].as_str();
        if !after_sep {
            if arg == "--" {
                after_sep = true;
                i += 1;
                continue;
            }
            if matches!(arg, "-h" | "--help") && positional.is_empty() {
                return Ok(CliAction::Help);
            }
            if arg == "--title" || arg == "-t" {
                let val = args
                    .get(i + 1)
                    .ok_or_else(|| format!("missing value for {arg}"))?;
                title = Some(val.clone());
                i += 2;
                continue;
            }
            if let Some(val) = arg.strip_prefix("--title=") {
                title = Some(val.to_owned());
                i += 1;
                continue;
            }
            if arg == "--pane" || arg == "-p" {
                let val = args
                    .get(i + 1)
                    .ok_or_else(|| format!("missing value for {arg}"))?;
                pane = Some(parse_u64_flag("--pane", val)?);
                i += 2;
                continue;
            }
            if let Some(val) = arg.strip_prefix("--pane=") {
                pane = Some(parse_u64_flag("--pane", val)?);
                i += 1;
                continue;
            }
            if arg.starts_with('-') && arg != "-" {
                return Err(format!("unknown option {arg:?} for `notify`"));
            }
        }
        positional.push(arg);
        i += 1;
    }

    if positional.is_empty() {
        return Err("usage: twitty notify [--pane N] [--title T] BODY".into());
    }
    Ok(CliAction::Notify {
        pane,
        title,
        body: positional.join(" "),
    })
}

fn parse_send(args: &[String], default_pane: Option<u64>) -> Result<CliAction, String> {
    let mut pane = default_pane;
    let mut positional: Vec<&str> = Vec::new();
    let mut i = 0;
    let mut after_sep = false;

    while i < args.len() {
        let arg = args[i].as_str();
        if !after_sep {
            if arg == "--" {
                after_sep = true;
                i += 1;
                continue;
            }
            if matches!(arg, "-h" | "--help") && positional.is_empty() {
                return Ok(CliAction::Help);
            }
            if arg == "--pane" || arg == "-p" {
                let val = args
                    .get(i + 1)
                    .ok_or_else(|| format!("missing value for {arg}"))?;
                pane = Some(parse_u64_flag("--pane", val)?);
                i += 2;
                continue;
            }
            if let Some(val) = arg.strip_prefix("--pane=") {
                pane = Some(parse_u64_flag("--pane", val)?);
                i += 1;
                continue;
            }
            if arg.starts_with('-') && arg != "-" && positional.is_empty() {
                return Err(format!("unknown option {arg:?} for `send`"));
            }
        }
        positional.push(arg);
        i += 1;
    }

    if positional.is_empty() {
        return Err("usage: twitty send [--pane N] TEXT".into());
    }
    Ok(CliAction::Send {
        pane,
        text: positional.join(" "),
    })
}

fn parse_list(args: &[String]) -> Result<CliAction, String> {
    match args {
        [] => Ok(CliAction::List),
        [a] if matches!(a.as_str(), "-h" | "--help") => Ok(CliAction::Help),
        [a, ..] => Err(format!("unexpected argument {a:?} for `list`")),
    }
}

fn parse_read(args: &[String], default_pane: Option<u64>) -> Result<CliAction, String> {
    let mut pane = default_pane;
    let mut lines: Option<usize> = None;
    let mut i = 0;

    while i < args.len() {
        let arg = args[i].as_str();
        if matches!(arg, "-h" | "--help") {
            return Ok(CliAction::Help);
        }
        if arg == "--pane" || arg == "-p" {
            let val = args
                .get(i + 1)
                .ok_or_else(|| format!("missing value for {arg}"))?;
            pane = Some(parse_u64_flag("--pane", val)?);
            i += 2;
            continue;
        }
        if let Some(val) = arg.strip_prefix("--pane=") {
            pane = Some(parse_u64_flag("--pane", val)?);
            i += 1;
            continue;
        }
        if arg == "--lines" || arg == "-n" {
            let val = args
                .get(i + 1)
                .ok_or_else(|| format!("missing value for {arg}"))?;
            lines = Some(parse_usize_flag("--lines", val)?);
            i += 2;
            continue;
        }
        if let Some(val) = arg.strip_prefix("--lines=") {
            lines = Some(parse_usize_flag("--lines", val)?);
            i += 1;
            continue;
        }
        return Err(format!("unexpected argument {arg:?} for `read`"));
    }

    Ok(CliAction::Read { pane, lines })
}

fn parse_claude_hook(args: &[String], default_pane: Option<u64>) -> Result<CliAction, String> {
    let mut pane = default_pane;
    let mut event: Option<String> = None;
    let mut i = 0;

    while i < args.len() {
        let arg = args[i].as_str();
        if matches!(arg, "-h" | "--help") && event.is_none() {
            return Ok(CliAction::Help);
        }
        if arg == "--pane" || arg == "-p" {
            let val = args
                .get(i + 1)
                .ok_or_else(|| format!("missing value for {arg}"))?;
            pane = Some(parse_u64_flag("--pane", val)?);
            i += 2;
            continue;
        }
        if let Some(val) = arg.strip_prefix("--pane=") {
            pane = Some(parse_u64_flag("--pane", val)?);
            i += 1;
            continue;
        }
        if arg.starts_with('-') {
            return Err(format!("unknown option {arg:?} for `claude-hook`"));
        }
        if event.is_some() {
            return Err(format!("unexpected argument {arg:?} for `claude-hook`"));
        }
        event = Some(arg.to_owned());
        i += 1;
    }

    let Some(event) = event else {
        return Err("usage: twitty claude-hook <event> [--pane N]".into());
    };
    Ok(CliAction::ClaudeHook { event, pane })
}

fn parse_u64_flag(flag: &str, val: &str) -> Result<u64, String> {
    val.parse::<u64>()
        .map_err(|_| format!("invalid integer for {flag}: {val:?}"))
}

fn parse_usize_flag(flag: &str, val: &str) -> Result<usize, String> {
    val.parse::<usize>()
        .map_err(|_| format!("invalid integer for {flag}: {val:?}"))
}

/// Format `tab.list` and `pane.list` results for human-readable `twitty list` output.
pub fn format_list(tabs: &Value, panes: &Value) -> String {
    use crate::shell_integration::{clean_text, MAX_NOTIFY_CHARS};

    let tabs = tabs.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let panes = panes.as_array().map(Vec::as_slice).unwrap_or(&[]);

    if tabs.is_empty() {
        return "No tabs open.\n".to_owned();
    }

    let mut out = String::new();
    for tab in tabs {
        let index = tab.get("index").and_then(Value::as_u64).unwrap_or(0);
        let title = clean_text(tab.get("title").and_then(Value::as_str).unwrap_or(""), 256);
        let active = tab.get("active").and_then(Value::as_bool).unwrap_or(false);
        let active_tag = if active { " [active]" } else { "" };
        out.push_str(&format!("Tab {index}{active_tag}: {title}\n"));

        for pane in panes {
            if pane.get("tab").and_then(Value::as_u64) != Some(index) {
                continue;
            }
            let id = pane.get("id").and_then(Value::as_u64).unwrap_or(0);
            let focused = pane
                .get("focused")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let marker = if focused { "*" } else { " " };
            let cols = pane
                .get("size")
                .and_then(|s| s.get("cols"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let rows = pane
                .get("size")
                .and_then(|s| s.get("rows"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let mut line = format!("  {marker} pane {id}  {cols}x{rows}");
            if let Some(cwd) = pane
                .get("cwd")
                .and_then(Value::as_str)
                .map(|s| clean_text(s, 4096))
                .filter(|s| !s.is_empty())
            {
                line.push_str(&format!("  {cwd}"));
            }
            if pane
                .get("running")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                line.push_str("  [running]");
            } else if let Some(code) = pane.get("last_exit").and_then(Value::as_i64) {
                line.push_str(&format!("  [exit {code}]"));
            }
            if let Some(pt) = pane
                .get("title")
                .and_then(Value::as_str)
                .map(|s| clean_text(s, 256))
                .filter(|s| !s.is_empty())
            {
                line.push_str(&format!("  {pt}"));
            }
            if let Some(att) = pane.get("attention").and_then(Value::as_object) {
                let atitle = clean_text(
                    att.get("title").and_then(Value::as_str).unwrap_or(""),
                    MAX_NOTIFY_CHARS,
                );
                let abody = clean_text(
                    att.get("body").and_then(Value::as_str).unwrap_or(""),
                    MAX_NOTIFY_CHARS,
                );
                if !atitle.is_empty() && !abody.is_empty() {
                    line.push_str(&format!("  [!] {atitle}: {abody}"));
                } else if !atitle.is_empty() {
                    line.push_str(&format!("  [!] {atitle}"));
                } else if !abody.is_empty() {
                    line.push_str(&format!("  [!] {abody}"));
                }
            }
            line.push('\n');
            out.push_str(&line);
        }
    }
    out
}

/// Build the JSON params object for a shortcut [`CliAction`].
pub fn build_params(action: &CliAction) -> Option<(&str, Value)> {
    match action {
        CliAction::Ctl { method, params } => Some((method.as_str(), params.clone())),
        CliAction::Ssh { destination, name } => Some((
            "tab.new-ssh",
            json!({ "destination": destination, "name": name }),
        )),
        CliAction::Notify { pane, title, body } => {
            let mut map = Map::new();
            if let Some(p) = pane {
                map.insert("pane".into(), json!(p));
            }
            if let Some(t) = title {
                map.insert("title".into(), json!(t));
            }
            map.insert("body".into(), json!(body));
            Some(("notify", Value::Object(map)))
        }
        CliAction::Send { pane, text } => {
            let mut map = Map::new();
            if let Some(p) = pane {
                map.insert("pane".into(), json!(p));
            }
            map.insert("text".into(), json!(text));
            Some(("pane.send-text", Value::Object(map)))
        }
        CliAction::Read { pane, lines } => {
            let mut map = Map::new();
            if let Some(p) = pane {
                map.insert("pane".into(), json!(p));
            }
            if let Some(n) = lines {
                map.insert("lines".into(), json!(n));
            }
            Some(("pane.read", Value::Object(map)))
        }
        CliAction::RunGui
        | CliAction::Help
        | CliAction::Version
        | CliAction::List
        | CliAction::ClaudeHook { .. } => None,
    }
}

/// Execute a non-GUI [`CliAction`] and return the process exit code.
#[cfg(unix)]
pub fn run_cli(action: CliAction) -> i32 {
    use crate::control::client::Client;

    match action {
        CliAction::RunGui => return 0,
        CliAction::Help => {
            print!("{}", help_text());
            return 0;
        }
        CliAction::Version => {
            println!("twitty {}", env!("CARGO_PKG_VERSION"));
            return 0;
        }
        CliAction::ClaudeHook { event, pane } => {
            return crate::control::claude_hook::run_cli_hook(&event, pane);
        }
        _ => {}
    }

    let mut client = match Client::connect_default() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("twitty: {err}");
            return 1;
        }
    };

    match action {
        CliAction::Ctl { method, params } => match client.call(method, params) {
            Ok(resp) => {
                match serde_json::to_string(&resp) {
                    Ok(line) => println!("{line}"),
                    Err(e) => {
                        eprintln!("twitty: failed to encode response: {e}");
                        return 1;
                    }
                }
                if resp.ok {
                    0
                } else {
                    1
                }
            }
            Err(err) => {
                eprintln!("twitty: {err}");
                1
            }
        },
        CliAction::List => {
            let tabs = match client.call("tab.list", Value::Object(Map::new())) {
                Ok(r) if r.ok => r.result.unwrap_or(Value::Array(Vec::new())),
                Ok(r) => {
                    eprintln!("twitty: {}", r.error.unwrap_or_else(|| "error".into()));
                    return 1;
                }
                Err(err) => {
                    eprintln!("twitty: {err}");
                    return 1;
                }
            };
            let panes = match client.call("pane.list", Value::Object(Map::new())) {
                Ok(r) if r.ok => r.result.unwrap_or(Value::Array(Vec::new())),
                Ok(r) => {
                    eprintln!("twitty: {}", r.error.unwrap_or_else(|| "error".into()));
                    return 1;
                }
                Err(err) => {
                    eprintln!("twitty: {err}");
                    return 1;
                }
            };
            print!("{}", format_list(&tabs, &panes));
            0
        }
        CliAction::Read { .. } => {
            let Some((method, params)) = build_params(&action) else {
                return 1;
            };
            match client.call(method, params) {
                Ok(r) if r.ok => {
                    if let Some(text) = r
                        .result
                        .as_ref()
                        .and_then(|v| v.get("text"))
                        .and_then(Value::as_str)
                    {
                        if !text.is_empty() {
                            println!("{text}");
                        }
                    }
                    0
                }
                Ok(r) => {
                    eprintln!("twitty: {}", r.error.unwrap_or_else(|| "error".into()));
                    1
                }
                Err(err) => {
                    eprintln!("twitty: {err}");
                    1
                }
            }
        }
        CliAction::Notify { .. } | CliAction::Send { .. } | CliAction::Ssh { .. } => {
            let Some((method, params)) = build_params(&action) else {
                return 1;
            };
            match client.call(method, params) {
                Ok(r) if r.ok => 0,
                Ok(r) => {
                    eprintln!("twitty: {}", r.error.unwrap_or_else(|| "error".into()));
                    1
                }
                Err(err) => {
                    eprintln!("twitty: {err}");
                    1
                }
            }
        }
        CliAction::RunGui | CliAction::Help | CliAction::Version | CliAction::ClaudeHook { .. } => {
            0
        }
    }
}

#[cfg(not(unix))]
pub fn run_cli(action: CliAction) -> i32 {
    match action {
        CliAction::RunGui => 0,
        CliAction::Help => {
            print!("{}", help_text());
            0
        }
        CliAction::Version => {
            println!("twitty {}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => {
            eprintln!("twitty: control CLI is unsupported on this platform");
            1
        }
    }
}
