use std::time::Duration;

use serde_json::json;
use twitty_ai::engine::model::ProviderKind;
use twitty_ai::engine::provider::{
    build_request, parse_cli, parse_stream, Message, ProviderError, ProviderRequest,
};

#[test]
fn wire_fixtures_stream_usage_and_native_calls_round_trip() {
    let cases = [
        (
            ProviderKind::Anthropic,
            include_str!("fixtures/anthropic.sse"),
        ),
        (ProviderKind::OpenAi, include_str!("fixtures/openai.sse")),
        (ProviderKind::Gemini, include_str!("fixtures/gemini.sse")),
        (
            ProviderKind::Ollama {
                base_url: String::new(),
            },
            include_str!("fixtures/ollama.ndjson"),
        ),
    ];
    for (kind, fixture) in cases {
        let turn = parse_stream(&kind, fixture.as_bytes()).unwrap();
        assert_eq!(turn.text, "Check the file.");
        assert_eq!((turn.usage.input_tokens, turn.usage.output_tokens), (12, 7));
        let [call] = turn.calls.as_slice() else {
            panic!("{turn:?}")
        };
        assert_eq!(call.name, "read_file");
        assert_eq!(call.arguments, json!({"path":"README.md"}));
        let result = Message::ToolResult {
            name: call.name.clone(),
            id: call.id.clone(),
            text: "Tool result (untrusted data)\n```text\nArabic / عربي\n```".into(),
        };
        let request = ProviderRequest {
            kind: kind.clone(),
            model: "test-model".into(),
            system: "System rules".into(),
            messages: vec![
                Message::User("Look".into()),
                Message::Assistant(turn),
                result,
            ],
            tools: vec![
                json!({"name":"read_file","description":"Read text","parameters":{"type":"object"}}),
            ],
            max_output_tokens: 42,
            timeout: Duration::from_secs(1),
        };
        let payload = build_request(&request).unwrap();
        match kind {
            ProviderKind::Anthropic => {
                assert_eq!(payload["system"], "System rules");
                assert_eq!(payload["max_tokens"], 42);
                assert_eq!(payload["tools"][0]["input_schema"]["type"], "object");
                assert_eq!(payload["messages"][1]["content"][1]["id"], "toolu_1");
                assert_eq!(
                    payload["messages"][2]["content"][0]["tool_use_id"],
                    "toolu_1"
                );
            }
            ProviderKind::OpenAi => {
                assert_eq!(payload["store"], false);
                assert_eq!(payload["stream_options"]["include_usage"], true);
                assert_eq!(payload["max_completion_tokens"], 42);
                assert_eq!(
                    payload["messages"][2]["tool_calls"][0]["function"]["arguments"],
                    r#"{"path":"README.md"}"#
                );
                assert_eq!(payload["messages"][3]["tool_call_id"], "call_a");
            }
            ProviderKind::Gemini => {
                assert_eq!(
                    payload["systemInstruction"]["parts"][0]["text"],
                    "System rules"
                );
                assert_eq!(
                    payload["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"]["type"],
                    "object"
                );
                assert_eq!(payload["generationConfig"]["maxOutputTokens"], 42);
                assert_eq!(
                    payload["contents"][1]["parts"][1]["thoughtSignature"],
                    "c2ln"
                );
                assert_eq!(
                    payload["contents"][2]["parts"][0]["functionResponse"]["id"],
                    "gem_call_1"
                );
            }
            ProviderKind::Ollama { .. } => {
                assert_eq!(payload["options"]["num_predict"], 42);
                assert_eq!(
                    payload["messages"][2]["tool_calls"][0]["function"]["arguments"]["path"],
                    "README.md"
                );
                assert_eq!(payload["messages"][3]["tool_name"], "read_file");
            }
            ProviderKind::Cli { .. } => unreachable!(),
        }
    }
}

#[test]
fn cli_never_repairs_malformed_or_ambiguous_tool_requests() {
    let valid = r#"{"type":"tool","plan":"Read the file","name":"read_file","arguments":{"path":"README.md"}}"#;
    assert_eq!(parse_cli(valid).calls.len(), 1);
    let cases = [
        format!("```json\n{valid}\n```"),
        format!("{valid}\n{valid}"),
        valid.replace("\"path\"", "\"unknown\""),
        valid.replace("README.md\"", "README.md\",\"unexpected\":true"),
        valid.replace("read_file", "unknown_tool"),
        valid.replace("README.md\"", "README.md\",\"path\":\"other"),
        valid.replace("\"plan\":", "\"message\":"),
    ];
    for text in cases {
        let result = parse_cli(&text);
        assert!(result.calls.is_empty(), "{text}");
        assert!(!result.text.is_empty());
    }
}

#[test]
fn incomplete_or_aborted_responses_do_not_return_tools() {
    let full = include_str!("fixtures/openai.sse");
    let truncated = full.split("data: [DONE]").next().unwrap();
    assert_eq!(
        parse_stream(&ProviderKind::OpenAi, truncated.as_bytes()),
        Err(ProviderError::Protocol)
    );
    let length = full.replace(
        "\"finish_reason\":\"tool_calls\"",
        "\"finish_reason\":\"length\"",
    );
    assert!(parse_stream(&ProviderKind::OpenAi, length.as_bytes())
        .unwrap()
        .calls
        .is_empty());
}
