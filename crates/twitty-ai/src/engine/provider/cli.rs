//! The isolated read-only CLI runner and its single-object response protocol.

use serde::Deserialize;

use super::{
    Arguments, Cancellation, Message, ProviderError, ProviderRequest, ProviderTurn, ToolCall,
};
use crate::ai::{non_interactive_argv, AiError, MAX_AI_OUTPUT_BYTES, NON_INTERACTIVE_AGENTS};
use crate::engine::model::Usage;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Response {
    Message {
        text: String,
    },
    Tool {
        plan: String,
        name: String,
        arguments: Arguments,
    },
}

pub fn parse_cli(output: &str) -> ProviderTurn {
    // A parse failure is ordinary display text. No repair, Markdown extraction, or execution.
    let mut turn = match serde_json::from_str(output.trim()) {
        Ok(Response::Message { text }) => ProviderTurn {
            text,
            ..ProviderTurn::default()
        },
        Ok(Response::Tool {
            plan,
            name,
            arguments,
        }) => {
            let call = ToolCall::new(name, arguments.0);
            if call.decode().is_some() {
                ProviderTurn {
                    text: plan,
                    calls: vec![call],
                    ..ProviderTurn::default()
                }
            } else {
                ProviderTurn {
                    text: output.into(),
                    ..ProviderTurn::default()
                }
            }
        }
        Err(_) => ProviderTurn {
            text: output.into(),
            ..ProviderTurn::default()
        },
    };
    turn.sanitize(None);
    turn
}

pub(super) fn prompt(request: &ProviderRequest) -> String {
    let mut prompt = request.system.clone();
    prompt.push_str("\nReturn exactly one JSON object, with no Markdown or surrounding text. ");
    prompt.push_str(r#"For a final answer use {"type":"message","text":"…"}. To propose one tool use {"type":"tool","plan":"…","name":"…","arguments":{…}}."#);
    prompt.push_str("\nTool schemas\n");
    prompt.push_str(&serde_json::to_string(&request.tools).unwrap_or_default());
    for message in &request.messages {
        use std::fmt::Write;
        match message {
            Message::User(text) => {
                let _ = write!(prompt, "\n\nUser message\n{text}");
            }
            Message::Assistant(turn) => {
                let _ = write!(
                    prompt,
                    "\n\n{}",
                    crate::engine::fenced("Previous model response", &turn.text)
                );
                for call in &turn.calls {
                    let request = serde_json::json!({"name":call.name,"arguments":call.arguments});
                    let _ = write!(
                        prompt,
                        "\n{}",
                        crate::engine::fenced("Previous tool proposal", &request.to_string())
                    );
                }
            }
            Message::ToolResult { name, text, .. } => {
                let _ = write!(
                    prompt,
                    "\n\n{}\n{text}",
                    crate::engine::fenced("Tool name", name)
                );
            }
        }
    }
    super::sanitized(&prompt, None)
}

pub(super) fn complete(
    program: &str,
    request: &ProviderRequest,
    cancel: &Cancellation,
) -> Result<ProviderTurn, ProviderError> {
    let stem = std::path::Path::new(program)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    if !NON_INTERACTIVE_AGENTS
        .iter()
        .any(|spec| stem.as_deref() == Some(spec.program))
    {
        return Err(ProviderError::Configuration);
    }
    let mut argv = vec![program.to_owned()];
    let confinement: &[&str] = match stem.as_deref() {
        Some("claude") => &[
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            r#"{"mcpServers":{}}"#,
            "--no-session-persistence",
        ],
        Some("codex") => &[
            "--ignore-user-config",
            "--ignore-rules",
            "--ephemeral",
            "-c",
            "web_search=\"disabled\"",
            "-c",
            "features.shell_tool=false",
            "-c",
            "features.shell_snapshot=false",
        ],
        _ => &[],
    };
    argv.extend(confinement.iter().map(|flag| (*flag).to_owned()));
    if !request.model.is_empty() {
        argv.push(format!("--model={}", request.model));
    }
    let prompt = prompt(request);
    // Linux limits each single argv string to MAX_ARG_STRLEN (128 KiB). Keep small prompts on
    // argv for maximum compatibility, and pipe larger multi-turn prompts via stdin to avoid E2BIG.
    const CLI_ARGV_PROMPT_LIMIT: usize = 64 * 1024;
    let output_limit = MAX_AI_OUTPUT_BYTES.min(request.max_output_tokens as usize * 4);
    let output = if prompt.len() > CLI_ARGV_PROMPT_LIMIT {
        let placeholder = match stem.as_deref() {
            Some("codex") => "-",
            _ => "",
        };
        let argv = non_interactive_argv(&argv, placeholder);
        crate::ai::runner::run_agent_command_with_stdin(
            &argv,
            prompt.as_bytes(),
            request.timeout,
            output_limit,
            &cancel.flag,
        )
    } else {
        let argv = non_interactive_argv(&argv, &prompt);
        crate::ai::runner::run_agent_command_blocking(
            &argv,
            request.timeout,
            output_limit,
            &cancel.flag,
        )
    }
    .map_err(|error| match error {
        AiError::Cancelled => ProviderError::Cancelled,
        AiError::TimedOut(_) => ProviderError::Timeout,
        AiError::OutputTooLarge => ProviderError::TooLarge,
        _ => ProviderError::Unavailable,
    })?;
    let mut turn = parse_cli(&output);
    turn.usage = Usage {
        input_tokens: (prompt.len() as u64).div_ceil(4),
        output_tokens: (output.len() as u64).div_ceil(4),
        cost_micros_usd: None,
    };
    Ok(turn)
}
