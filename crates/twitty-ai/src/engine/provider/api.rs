//! Wire formats for Anthropic Messages, OpenAI Chat Completions, Gemini and Ollama.

use std::collections::BTreeMap;
use std::io::BufRead;

use serde_json::{json, Value};

use super::{
    Arguments, Cancellation, Message, ProviderError, ProviderRequest, ProviderTurn, ToolCall,
};
use crate::engine::model::ProviderKind;

const MAX_WIRE_BYTES: u64 = 4 * 1024 * 1024;

pub fn build_request(request: &ProviderRequest) -> Result<Value, ProviderError> {
    let mut messages = Vec::new();
    let mut tools = request.tools.clone();
    let tokens = request.max_output_tokens;
    let model = &request.model;
    let mut result_parts = Vec::new();
    let (anthropic, gemini, ollama) = (
        request.kind == ProviderKind::Anthropic,
        request.kind == ProviderKind::Gemini,
        matches!(request.kind, ProviderKind::Ollama { .. }),
    );
    if !anthropic && !gemini {
        messages.push(json!({"role": "system", "content": request.system}));
    }
    for message in &request.messages {
        if !matches!(message, Message::ToolResult { .. }) && !result_parts.is_empty() {
            push_results(&mut messages, &mut result_parts, anthropic);
        }
        match message {
            Message::User(text) if gemini => {
                messages.push(json!({"role": "user", "parts": [{"text":text}]}))
            }
            Message::User(text) => messages.push(json!({"role": "user", "content": text})),
            Message::Assistant(turn) => {
                let mut content = Vec::new();
                if anthropic && !turn.text.is_empty() {
                    content.push(json!({"type": "text", "text": turn.text}));
                }
                if gemini && !turn.native_parts.is_empty() {
                    messages.push(json!({"role": "model", "parts": turn.native_parts}));
                    continue;
                }
                if gemini && !turn.text.is_empty() {
                    content.push(json!({"text": turn.text}));
                }
                for call in &turn.calls {
                    content.push(if anthropic {
                        json!({"type": "tool_use", "id":call.id, "name":call.name, "input":call.arguments})
                    } else if gemini {
                        json!({"functionCall": {"name":call.name, "args":call.arguments}})
                    } else if ollama {
                        json!({"function":{"name":call.name,"arguments":call.arguments}})
                    } else {
                        json!({"id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments.to_string()}})
                    });
                }
                if anthropic {
                    messages.push(json!({"role":"assistant","content":content}));
                } else if gemini {
                    messages.push(json!({"role":"model","parts":content}));
                } else {
                    let mut item = json!({"role":"assistant","content":turn.text});
                    if !content.is_empty() {
                        item["tool_calls"] = Value::Array(content);
                    }
                    messages.push(item);
                }
            }
            Message::ToolResult { id, text, .. } if anthropic => {
                result_parts.push(json!({"type":"tool_result","tool_use_id":id,"content":text}));
            }
            Message::ToolResult { id, name, text } if gemini => {
                let mut response = json!({"name":name,"response":{"result":text}});
                if !id.starts_with("twitty_gemini_") {
                    response["id"] = json!(id);
                }
                result_parts.push(json!({"functionResponse":response}));
            }
            Message::ToolResult { name, text, .. } if ollama => {
                messages.push(json!({"role":"tool","tool_name":name,"content":text}));
            }
            Message::ToolResult { id, text, .. } => {
                messages.push(json!({"role":"tool","tool_call_id":id,"content":text}));
            }
        }
    }
    if !result_parts.is_empty() {
        push_results(&mut messages, &mut result_parts, anthropic);
    }
    let mut body = match &request.kind {
        ProviderKind::Anthropic => {
            for tool in &mut tools {
                tool["input_schema"] = tool
                    .as_object_mut()
                    .and_then(|obj| obj.remove("parameters"))
                    .unwrap_or_default();
            }
            json!({"model":model,"system":request.system,"messages":messages,"tools":tools,"stream":true,"max_tokens":tokens})
        }
        ProviderKind::Gemini => {
            for tool in &mut tools {
                tool["parametersJsonSchema"] = tool
                    .as_object_mut()
                    .and_then(|obj| obj.remove("parameters"))
                    .unwrap_or_default();
            }
            json!({"systemInstruction":{"parts":[{"text":request.system}]},"contents":messages,"tools":[{"functionDeclarations":tools}],"generationConfig":{"maxOutputTokens":tokens}})
        }
        ProviderKind::OpenAi | ProviderKind::Ollama { .. } => {
            let tools: Vec<_> = tools
                .into_iter()
                .map(|tool| json!({"type":"function","function":tool}))
                .collect();
            if ollama {
                json!({"model":model,"messages":messages,"tools":tools,"stream":true,"options":{"num_predict":tokens}})
            } else {
                json!({"model":model,"messages":messages,"tools":tools,"stream":true,"stream_options":{"include_usage":true},"max_completion_tokens":tokens,"store":false})
            }
        }
        ProviderKind::Cli { .. } => return Err(ProviderError::Configuration),
    };
    super::scrub(&mut body, None);
    Ok(body)
}

fn push_results(messages: &mut Vec<Value>, results: &mut Vec<Value>, anthropic: bool) {
    let field = if anthropic { "content" } else { "parts" };
    messages.push(json!({"role":"user", (field):std::mem::take(results)}));
}

#[derive(Default)]
struct Fragment {
    id: String,
    name: String,
    arguments: String,
    initial: Option<Value>,
    closed: bool,
}

#[derive(Default)]
struct Stream {
    turn: ProviderTurn,
    fragments: BTreeMap<u64, Fragment>,
    complete: bool,
    calls_complete: bool,
}

impl Stream {
    fn ingest(&mut self, kind: &ProviderKind, frame: Value) -> Result<(), ProviderError> {
        if frame.get("error").is_some() || frame["type"] == "error" {
            return Err(ProviderError::Unavailable);
        }
        match kind {
            ProviderKind::Anthropic => self.anthropic(&frame)?,
            ProviderKind::OpenAi => self.openai(&frame)?,
            ProviderKind::Gemini => self.gemini(&frame)?,
            ProviderKind::Ollama { .. } => self.ollama(&frame)?,
            ProviderKind::Cli { .. } => return Err(ProviderError::Configuration),
        }
        if self.turn.calls.len() + self.fragments.len() > 32 {
            return Err(ProviderError::TooLarge);
        }
        Ok(())
    }

    fn anthropic(&mut self, frame: &Value) -> Result<(), ProviderError> {
        let stable_index = || frame["index"].as_u64().ok_or(ProviderError::Protocol);
        match frame["type"].as_str().unwrap_or_default() {
            "message_start" => {
                let usage = &frame["message"]["usage"];
                self.turn.usage.input_tokens = number(usage, "input_tokens")
                    .saturating_add(number(usage, "cache_creation_input_tokens"))
                    .saturating_add(number(usage, "cache_read_input_tokens"));
            }
            "content_block_start" => {
                let block = &frame["content_block"];
                match block["type"].as_str() {
                    Some("text") => self.text(&block["text"]),
                    Some("tool_use") => {
                        let fragment = Fragment {
                            id: string(block, "id"),
                            name: string(block, "name"),
                            initial: Some(block["input"].clone()),
                            ..Fragment::default()
                        };
                        if self.fragments.insert(stable_index()?, fragment).is_some() {
                            return Err(ProviderError::Protocol);
                        }
                    }
                    _ => {}
                }
            }
            "content_block_delta" => {
                let delta = &frame["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => self.text(&delta["text"]),
                    Some("input_json_delta") => {
                        let fragment = self
                            .fragments
                            .get_mut(&stable_index()?)
                            .ok_or(ProviderError::Protocol)?;
                        if fragment.closed {
                            return Err(ProviderError::Protocol);
                        }
                        fragment.arguments.push_str(
                            delta["partial_json"]
                                .as_str()
                                .ok_or(ProviderError::Protocol)?,
                        );
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(fragment) = self.fragments.get_mut(&stable_index()?) {
                    fragment.closed = true;
                }
            }
            "message_delta" => {
                self.calls_complete = frame["delta"]["stop_reason"] == "tool_use";
                self.turn.usage.output_tokens = number(&frame["usage"], "output_tokens");
            }
            "message_stop" => self.complete = true,
            _ => {}
        }
        Ok(())
    }

    fn openai(&mut self, frame: &Value) -> Result<(), ProviderError> {
        if let Some(choices) = frame["choices"].as_array() {
            for choice in choices {
                if choice["index"].as_u64().unwrap_or_default() != 0 {
                    continue;
                }
                self.text(&choice["delta"]["content"]);
                if let Some(calls) = choice["delta"]["tool_calls"].as_array() {
                    for call in calls {
                        let index = call["index"].as_u64().ok_or(ProviderError::Protocol)?;
                        let fragment = self.fragments.entry(index).or_default();
                        append(&mut fragment.id, &call["id"]);
                        append(&mut fragment.name, &call["function"]["name"]);
                        append(&mut fragment.arguments, &call["function"]["arguments"]);
                    }
                }
                if let Some(reason) = choice["finish_reason"].as_str() {
                    self.calls_complete = reason == "tool_calls";
                    for fragment in self.fragments.values_mut() {
                        fragment.closed = self.calls_complete;
                    }
                }
            }
        }
        if frame["usage"].is_object() {
            self.turn.usage.input_tokens = number(&frame["usage"], "prompt_tokens");
            self.turn.usage.output_tokens = number(&frame["usage"], "completion_tokens");
        }
        Ok(())
    }

    fn gemini(&mut self, frame: &Value) -> Result<(), ProviderError> {
        if let Some(candidate) = frame["candidates"].get(0) {
            if let Some(parts) = candidate["content"]["parts"].as_array() {
                for part in parts {
                    if part["thought"] != true {
                        self.text(&part["text"]);
                    }
                    if let Some(call) = part.get("functionCall") {
                        if !call.is_object()
                            || call.get("partialArgs").is_some()
                            || call["willContinue"] == true
                        {
                            return Err(ProviderError::Protocol);
                        }
                        let fallback = format!("twitty_gemini_{}", self.turn.calls.len());
                        self.turn.calls.push(ToolCall {
                            id: call["id"].as_str().unwrap_or(&fallback).into(),
                            name: string(call, "name"),
                            arguments: call.get("args").cloned().unwrap_or_else(|| json!({})),
                        });
                    }
                    // Keep the original parts and their order for Gemini's thought signatures.
                    self.turn.native_parts.push(part.clone());
                }
            }
            if let Some(reason) = candidate["finishReason"].as_str() {
                self.complete = true;
                self.calls_complete = reason == "STOP";
            }
        }
        if frame["usageMetadata"].is_object() {
            let usage = &frame["usageMetadata"];
            self.turn.usage.input_tokens = number(usage, "promptTokenCount");
            self.turn.usage.output_tokens = number(usage, "candidatesTokenCount")
                .saturating_add(number(usage, "thoughtsTokenCount"));
        }
        Ok(())
    }

    fn ollama(&mut self, frame: &Value) -> Result<(), ProviderError> {
        self.text(&frame["message"]["content"]);
        if let Some(calls) = frame["message"]["tool_calls"].as_array() {
            for call in calls {
                self.turn.calls.push(ToolCall {
                    id: format!("twitty_ollama_{}", self.turn.calls.len()),
                    name: string(&call["function"], "name"),
                    arguments: call["function"]["arguments"].clone(),
                });
            }
        }
        if frame["done"] == true {
            self.complete = true;
            self.calls_complete = !matches!(
                frame["done_reason"].as_str(),
                Some("length" | "load" | "unload")
            );
            self.turn.usage.input_tokens = number(frame, "prompt_eval_count");
            self.turn.usage.output_tokens = number(frame, "eval_count");
        }
        Ok(())
    }

    fn text(&mut self, value: &Value) {
        append(&mut self.turn.text, value);
    }

    fn finish(mut self) -> Result<ProviderTurn, ProviderError> {
        if !self.complete {
            return Err(ProviderError::Protocol);
        }
        if self.calls_complete && self.fragments.values().all(|fragment| fragment.closed) {
            for (_, fragment) in self.fragments {
                let arguments = if fragment.arguments.is_empty() {
                    fragment.initial.unwrap_or_default()
                } else {
                    serde_json::from_str::<Arguments>(&fragment.arguments)
                        .map(|arguments| arguments.0)
                        .unwrap_or_default()
                };
                self.turn.calls.push(ToolCall {
                    id: fragment.id,
                    name: fragment.name,
                    arguments,
                });
            }
        } else {
            self.turn.calls.clear();
            self.turn.native_parts.clear();
        }
        Ok(self.turn)
    }
}

fn string(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().into()
}

fn number(value: &Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or_default()
}

fn append(string: &mut String, value: &Value) {
    if let Some(text) = value.as_str() {
        string.push_str(text);
    }
}

/// Parse a complete SSE (cloud) or NDJSON (Ollama) response without making a network request.
pub fn parse_stream(
    kind: &ProviderKind,
    reader: impl BufRead,
) -> Result<ProviderTurn, ProviderError> {
    stream(kind, reader, &Cancellation::default(), None, &mut |_| {})
}

pub(super) fn stream(
    kind: &ProviderKind,
    reader: impl BufRead,
    cancel: &Cancellation,
    credential: Option<&str>,
    preview: &mut dyn FnMut(String),
) -> Result<ProviderTurn, ProviderError> {
    let mut state = Stream::default();
    let mut reader = reader.take(MAX_WIRE_BYTES + 1);
    let mut event = String::new();
    let mut line = String::new();
    let mut preview_len = 0;
    let ollama = matches!(kind, ProviderKind::Ollama { .. });
    loop {
        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }
        line.clear();
        let size = reader
            .read_line(&mut line)
            .map_err(|error| match error.kind() {
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
                    ProviderError::Timeout
                }
                _ => ProviderError::Protocol,
            })?;
        if reader.limit() == 0 {
            return Err(ProviderError::TooLarge);
        }
        if size == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if ollama {
            if !line.is_empty() {
                state.ingest(
                    kind,
                    serde_json::from_str(line).map_err(|_| ProviderError::Protocol)?,
                )?;
            }
        } else if let Some(data) = line.strip_prefix("data:") {
            if !event.is_empty() {
                event.push('\n');
            }
            event.push_str(data.strip_prefix(' ').unwrap_or(data));
        } else if line.is_empty() && !event.is_empty() {
            if event == "[DONE]" && *kind == ProviderKind::OpenAi {
                state.complete = true;
            } else {
                state.ingest(
                    kind,
                    serde_json::from_str(&event).map_err(|_| ProviderError::Protocol)?,
                )?;
            }
            event.clear();
        }
        // Only stream completed words with additional lookahead for split credentials.
        // The completed response is scrubbed as a whole before leaving the provider.
        let size = state
            .turn
            .text
            .len()
            .saturating_sub(credential.map_or(256, |key| key.len().max(256)));
        if size > preview_len + 128 {
            let prefix = crate::engine::bounded(&state.turn.text, size);
            let safe = prefix
                .rsplit_once(char::is_whitespace)
                .map_or("", |(prefix, _)| prefix);
            if safe.len() > preview_len {
                // Redact the full buffer first, so a credential spanning this preview boundary
                // cannot surface as a partial token.
                let clean = super::sanitized(&state.turn.text, credential);
                preview(crate::engine::bounded(&clean, safe.len()).into());
                preview_len = safe.len();
            }
        }
        if state.complete && *kind != ProviderKind::Gemini {
            break;
        }
    }
    if !event.is_empty() || cancel.is_cancelled() {
        return Err(if cancel.is_cancelled() {
            ProviderError::Cancelled
        } else {
            ProviderError::Protocol
        });
    }
    let mut turn = state.finish()?;
    turn.sanitize(credential);
    Ok(turn)
}
