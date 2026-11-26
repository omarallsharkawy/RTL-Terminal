//! Per-agent non-interactive command-line table, read-only enforcement, and agent resolution.
//!
//! Each supported coding-agent CLI has a documented non-interactive invocation that prints its
//! response to stdout and exits instead of launching a TUI, plus read-only flags enforced on
//! every built-in AI invocation (even when the user configured non-interactive flags themselves):
//! - `claude --permission-mode plan --disallowedTools "Bash Edit Write NotebookEdit" -p <prompt>`
//! - `codex exec --sandbox read-only --skip-git-repo-check <prompt>`
//! - `gemini --approval-mode default -p <prompt>`
//! - `opencode run <prompt>` (isolated in an empty temporary directory)

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// One entry in the per-agent non-interactive flag table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NonInteractiveAgentSpec {
    /// Lowercase executable stem (without path or `.exe`).
    pub program: &'static str,
    /// Flags inserted before the prompt argument when not already present.
    pub flags: &'static [&'static str],
    /// Alternative flags that already select non-interactive mode (so we don't duplicate).
    pub already_non_interactive: &'static [&'static str],
    /// Read-only / sandbox flags always ensured for built-in AI invocations, independent of
    /// `flags` (e.g. even when a configured `agent_command` already includes `-p`).
    pub read_only: &'static [&'static str],
    /// Standalone dangerous flags stripped from configured argv for built-in AI invocations.
    pub dangerous_flags: &'static [&'static str],
    /// `(flag, disallowed_values)` pairs stripped from configured argv (both `--flag val` and
    /// `--flag=val` forms) for built-in AI invocations.
    pub dangerous_flag_values: &'static [(&'static str, &'static [&'static str])],
}

/// Documented non-interactive and read-only invocation flags for each supported local coding agent.
pub const NON_INTERACTIVE_AGENTS: &[NonInteractiveAgentSpec] = &[
    NonInteractiveAgentSpec {
        program: "claude",
        flags: &["-p"],
        already_non_interactive: &["-p", "--print"],
        read_only: &[
            "--permission-mode",
            "plan",
            "--disallowedTools",
            "Bash Edit Write NotebookEdit",
        ],
        dangerous_flags: &[
            "--dangerously-skip-permissions",
            "--allow-dangerously-skip-permissions",
        ],
        dangerous_flag_values: &[(
            "--permission-mode",
            &["bypassPermissions", "acceptEdits", "auto"],
        )],
    },
    NonInteractiveAgentSpec {
        program: "codex",
        flags: &["exec"],
        already_non_interactive: &["exec", "e"],
        read_only: &["--sandbox", "read-only", "--skip-git-repo-check"],
        dangerous_flags: &[
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-bypass-hook-trust",
            "--full-auto",
            "--approve-for-me",
        ],
        dangerous_flag_values: &[
            ("-s", &["danger-full-access", "workspace-write"]),
            ("--sandbox", &["danger-full-access", "workspace-write"]),
        ],
    },
    NonInteractiveAgentSpec {
        program: "gemini",
        flags: &["-p"],
        already_non_interactive: &["-p", "--prompt"],
        read_only: &["--approval-mode", "default"],
        dangerous_flags: &["--yolo", "-y"],
        dangerous_flag_values: &[("--approval-mode", &["yolo", "auto_edit"])],
    },
    NonInteractiveAgentSpec {
        program: "opencode",
        flags: &["run"],
        already_non_interactive: &["run"],
        read_only: &[],
        dangerous_flags: &["--auto"],
        dangerous_flag_values: &[],
    },
];

/// Agents probed on `PATH` (in order) when neither `ai_agent` nor `agent_command` is configured.
pub const AI_KNOWN_AGENTS: &[&str] = &["claude", "codex", "gemini", "opencode"];

/// Why no coding agent could be resolved for a built-in AI request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiResolveError {
    /// `config.ai` is set to `false`.
    Disabled,
    /// No configured agent and none of `AI_KNOWN_AGENTS` is executable on `PATH`.
    NotFound,
}

/// Normalize the first element of an agent command vector to its lowercase program stem.
fn program_stem(agent: &[String]) -> String {
    agent
        .first()
        .and_then(|p| p.rsplit(['/', '\\']).next())
        .and_then(|base| Path::new(base).file_stem())
        .and_then(|n| n.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// Build the argument vector that runs `agent` non-interactively in read-only mode with `prompt`
/// as one argument.
///
/// Strips any configured dangerous permission/sandbox-bypass flags, ensures non-interactive flags,
/// and independently ensures every per-agent `read_only` flag is present. Never passes anything
/// through a shell; `prompt` is always appended as a single argv entry.
pub fn non_interactive_argv(agent: &[String], prompt: &str) -> Vec<String> {
    let mut argv = agent.to_vec();
    if argv.is_empty() {
        return argv;
    }
    let program = program_stem(&argv);
    if let Some(spec) = NON_INTERACTIVE_AGENTS
        .iter()
        .find(|spec| spec.program == program)
    {
        strip_dangerous_flags(&mut argv, spec);

        let has_flag = argv[1..]
            .iter()
            .any(|arg| spec.already_non_interactive.contains(&arg.as_str()));
        let is_subcommand = spec.flags.first().is_some_and(|f| !f.starts_with('-'));
        if !has_flag {
            // For subcommand-based CLIs (codex exec, opencode run), insert the subcommand right
            // after the binary if extra flags were configured, or append directly when bare.
            // For flag-based CLIs (claude -p, gemini -p), appending before the prompt works
            // cleanly with any configured model/profile flags.
            if is_subcommand {
                for (i, flag) in spec.flags.iter().enumerate() {
                    argv.insert(1 + i, (*flag).to_string());
                }
            } else {
                argv.extend(spec.flags.iter().map(|s| (*s).to_string()));
            }
        }

        ensure_read_only_flags(&mut argv, spec, is_subcommand);
    }
    argv.push(prompt.to_string());
    argv
}

/// Remove dangerous permission-bypass or writable-sandbox flags from `argv[1..]`, logging each
/// removal at debug level.
fn strip_dangerous_flags(argv: &mut Vec<String>, spec: &NonInteractiveAgentSpec) {
    let mut cleaned = Vec::with_capacity(argv.len());
    cleaned.push(argv[0].clone());

    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];

        if spec.dangerous_flags.contains(&arg.as_str()) {
            tracing::debug!(
                agent = spec.program,
                flag = arg.as_str(),
                "stripped dangerous flag from built-in AI agent argv"
            );
            i += 1;
            continue;
        }

        // Check `--flag=value` form.
        if let Some((flag, val)) = arg.split_once('=') {
            let is_dangerous = spec
                .dangerous_flag_values
                .iter()
                .any(|(f, vals)| *f == flag && vals.contains(&val));
            if is_dangerous {
                tracing::debug!(
                    agent = spec.program,
                    flag = arg.as_str(),
                    "stripped dangerous flag from built-in AI agent argv"
                );
                i += 1;
                continue;
            }
        }

        // Check two-token `--flag value` form.
        if let Some(next) = argv.get(i + 1) {
            let is_dangerous = spec
                .dangerous_flag_values
                .iter()
                .any(|(f, vals)| *f == arg.as_str() && vals.contains(&next.as_str()));
            if is_dangerous {
                tracing::debug!(
                    agent = spec.program,
                    flag = arg.as_str(),
                    value = next.as_str(),
                    "stripped dangerous flag from built-in AI agent argv"
                );
                i += 2;
                continue;
            }
        }

        cleaned.push(arg.clone());
        i += 1;
    }

    *argv = cleaned;
}

/// Always ensure every flag (or flag+value pair) in `spec.read_only` is present in `argv`,
/// independent of whether non-interactive flags were already configured.
fn ensure_read_only_flags(
    argv: &mut Vec<String>,
    spec: &NonInteractiveAgentSpec,
    is_subcommand: bool,
) {
    if spec.read_only.is_empty() {
        return;
    }

    let mut missing = Vec::new();
    let mut i = 0;
    while i < spec.read_only.len() {
        let flag = spec.read_only[i];
        let value = spec
            .read_only
            .get(i + 1)
            .copied()
            .filter(|next| !next.starts_with('-'));
        if let Some(val) = value {
            if !has_flag_value(&argv[1..], flag, val) {
                missing.push(flag.to_string());
                missing.push(val.to_string());
            }
            i += 2;
        } else {
            if !argv[1..].iter().any(|arg| arg == flag) {
                missing.push(flag.to_string());
            }
            i += 1;
        }
    }

    if missing.is_empty() {
        return;
    }

    if is_subcommand {
        // Subcommand-based CLIs (e.g. `codex exec`): the subcommand is already before the end of
        // `argv`, so appending keeps read-only flags after the subcommand and before `prompt`.
        argv.extend(missing);
    } else if let Some(pos) = argv[1..]
        .iter()
        .position(|arg| spec.already_non_interactive.contains(&arg.as_str()))
        .map(|offset| offset + 1)
    {
        // Flag-based CLIs (e.g. `claude -p`, `gemini -p`): insert right before `-p`/`--print`/
        // `--prompt` so `-p` stays adjacent to the prompt (required by `gemini -p <prompt>`)
        // and terminates variadic options like `claude --disallowedTools <tools...>`.
        argv.splice(pos..pos, missing);
    } else {
        argv.extend(missing);
    }
}

fn matches_flag_alias(token: &str, canonical: &str) -> bool {
    if token == canonical {
        return true;
    }
    match canonical {
        "--sandbox" => token == "-s",
        "--disallowedTools" => token == "--disallowed-tools",
        _ => false,
    }
}

fn has_flag_value(args: &[String], flag: &str, value: &str) -> bool {
    for (i, arg) in args.iter().enumerate() {
        if matches_flag_alias(arg, flag) && args.get(i + 1).map(String::as_str) == Some(value) {
            return true;
        }
        if let Some((k, v)) = arg.split_once('=') {
            if matches_flag_alias(k, flag) && v == value {
                return true;
            }
        }
    }
    false
}

/// Resolve the agent argv to run for built-in AI.
///
/// Priority:
/// 1. If `!ai_enabled`, returns `Err(AiResolveError::Disabled)`.
/// 2. `ai_agent` (split on whitespace) if set and non-empty.
/// 3. `agent_command` (split on whitespace) if set and non-empty.
/// 4. First executable in `AI_KNOWN_AGENTS` found on `path`.
pub fn resolve_ai_agent(
    ai_enabled: bool,
    ai_agent: Option<&str>,
    agent_command: Option<&str>,
    path: Option<&OsStr>,
) -> Result<Vec<String>, AiResolveError> {
    if !ai_enabled {
        return Err(AiResolveError::Disabled);
    }
    for configured in [ai_agent, agent_command].into_iter().flatten() {
        let argv: Vec<String> = configured.split_whitespace().map(str::to_string).collect();
        if !argv.is_empty() {
            return Ok(argv);
        }
    }
    let Some(path) = path else {
        return Err(AiResolveError::NotFound);
    };
    let dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
    AI_KNOWN_AGENTS
        .iter()
        .find(|name| dirs.iter().any(|d| is_executable(&d.join(exe_name(name)))))
        .map(|name| vec![(*name).to_string()])
        .ok_or(AiResolveError::NotFound)
}

/// Human-readable, cleaned display name for an agent argv (e.g. `"claude"`).
pub fn agent_display_name(argv: &[String]) -> String {
    let raw = argv
        .first()
        .and_then(|p| p.rsplit(['/', '\\']).next())
        .unwrap_or("agent");
    let cleaned = crate::shell_integration::clean_text(raw, 64);
    if cleaned.is_empty() {
        "agent".to_string()
    } else {
        cleaned
    }
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}
