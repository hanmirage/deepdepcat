//! OpenAI Responses API — body construction and response parsing.
//!
//! The Responses API (`POST /responses`) is OpenAI's successor to chat
//! completions. Key differences handled here:
//! - `input` array instead of `messages`; tool results are
//!   `function_call_output` items referencing a `call_id`
//! - assistant history uses `tool_calls` inline + content as
//!   `[{"type": "output_text", ...}]`
//! - `max_output_tokens` instead of `max_tokens`
//! - reasoning effort lives under `reasoning.effort` (low/medium/high)
//!   and GPT-5 thinking is excluded from the visible output via
//!   `reasoning.exclude: ["thinking"]` (thinking is still used internally,
//!   it just never costs output tokens to stream back)
//! - response carries `output: [{type: message|function_call|reasoning}]`

use crate::core::error::AppResult;
use crate::llm::provider::LlmResponse;
use serde_json::{json, Value};

use super::LlmClient;

impl LlmClient {
    /// Build the request body in OpenAI Responses API format.
    pub(super) fn build_responses_body(
        &self,
        request: &super::super::provider::LlmRequest,
        provider_name: &str,
    ) -> Value {
        let mut input: Vec<Value> = Vec::new();
        let mut pending: Vec<(String, usize)> = Vec::new();
        // System messages emitted while a tool group is still being answered
        // are deferred until the group's outputs are flushed (mirrors the
        // chat-completions builder).
        let mut deferred_systems: Vec<Value> = Vec::new();

        if !request.system_prompt.is_empty() {
            input.push(json!({
                "role": "system",
                "content": [{"type": "input_text", "text": request.system_prompt}],
            }));
        }

        for item in &request.messages {
            match item {
                crate::core::types::ConversationItem::System(s) => {
                    // Defer system messages while a tool group is being
                    // answered — function_call_output items must stay
                    // adjacent to the assistant that declared the calls.
                    let sys = json!({
                        "role": "system",
                        "content": [{"type": "input_text", "text": s.content}],
                    });
                    if pending.is_empty() {
                        input.push(sys);
                    } else {
                        deferred_systems.push(sys);
                    }
                }
                crate::core::types::ConversationItem::User(u) => {
                    let content: Vec<Value> = u
                        .content
                        .iter()
                        .map(|part| match part {
                            crate::core::types::ContentPart::Text { text } => {
                                json!({"type": "input_text", "text": text})
                            }
                            crate::core::types::ContentPart::Image {
                                media_type, data, ..
                            } => {
                                json!({
                                    "type": "input_image",
                                    "image_url": format!("data:{};base64,{}", media_type, data),
                                })
                            }
                        })
                        .collect();
                    input.push(json!({"role": "user", "content": content}));
                }
                crate::core::types::ConversationItem::Assistant(a) => {
                    // Assistant message WITHOUT inline tool_calls: the
                    // Responses API (OpenAI and DeepSeek) matches
                    // function_call_output items against function_call
                    // ITEMS, not against assistant-embedded tool_calls.
                    // DeepSeek ignores the inline field entirely, so history
                    // is emitted as standalone function_call items right
                    // after the assistant message (the server merges them).
                    let msg = json!({
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": a.content}],
                    });
                    input.push(msg);
                    for tc in &a.tool_calls {
                        let call_index = input.len();
                        pending.push((tc.id.clone(), call_index));
                        input.push(json!({
                            "type": "function_call",
                            "call_id": tc.id,
                            "name": tc.name,
                            "arguments": tc.arguments,
                        }));
                    }
                    // reasoning_content is deliberately dropped: the
                    // Responses API re-thinks each turn and never accepts
                    // prior chain-of-thought as input.
                }
                crate::core::types::ConversationItem::ToolResult(tr) => {
                    // Only keep results that answer a declared call.
                    if let Some(pos) = pending.iter().position(|(id, _)| id == &tr.tool_call_id) {
                        pending.remove(pos);
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": tr.tool_call_id,
                            "output": tr.content,
                        }));
                        // The tool group is fully answered — flush deferred
                        // system messages after the last output.
                        if pending.is_empty() {
                            input.append(&mut deferred_systems);
                        }
                    }
                }
                crate::core::types::ConversationItem::Reasoning(_) => {
                    // Legacy persistence format — never valid in Responses input.
                }
            }
        }

        // Still-unanswered tool calls get a synthetic output inserted
        // immediately after their declaring assistant message.
        for (id, assistant_idx) in pending.iter().rev() {
            input.insert(
                assistant_idx + 1,
                json!({
                    "type": "function_call_output",
                    "call_id": id,
                    "output": "[Tool execution was interrupted — the tool did not produce a result.]",
                }),
            );
        }

        // Leftover deferred system messages (group never completed) — append
        // after the synthetics so nothing is lost.
        input.append(&mut deferred_systems);

        let mut body = json!({
            "model": request.model,
            "input": input,
            "stream": request.stream,
        });

        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(max) = request.max_tokens {
            body["max_output_tokens"] = json!(max);
        }

        // Reasoning effort — per provider (DeepSeek V4 manual, responses_api.md):
        // - DeepSeek Responses: `reasoning.effort` ∈ {none,minimal,low,medium,
        //   high,xhigh,max}, folded to effective low/high/max (minimal/low→low;
        //   medium/high/xhigh→high; max→max; none→thinking off). `summary` is
        //   accepted but never generated; `exclude` is NOT in the supported set
        //   (DeepSeek streams thinking back as reasoning items regardless), so
        //   only `effort` is sent.
        // - OpenAI Responses (GPT-5/o3): `reasoning.effort` ∈ {low,medium,high};
        //   DeepSeek-style max/xhigh fold to high. `exclude:["thinking"]` keeps
        //   GPT-5 thinking internal (used, never streamed back as output tokens).
        if let Some(ref effort) = request.reasoning_effort {
            if super::openai::is_deepseek_provider(provider_name) {
                let mapped = match effort.as_str() {
                    "minimal" | "low" => "low",
                    "medium" | "high" | "xhigh" => "high",
                    "max" => "max",
                    other => other, // "none" passes through (thinking off)
                };
                body["reasoning"] = json!({ "effort": mapped });
            } else {
                let mapped = match effort.as_str() {
                    "max" | "xhigh" => "high",
                    "minimal" => "low",
                    other => other,
                };
                if mapped != "none" {
                    body["reasoning"] = json!({ "effort": mapped, "exclude": ["thinking"] });
                }
            }
        }

        // deepseek-native: per-user isolation via the top-level `user` field
        // (responses_api.md: user supported, refers to rate-limit/isolation).
        // OpenAI Responses also has `user` (abuse tracking); only DeepSeek's
        // isolation semantics apply here, so gate on the provider.
        if super::openai::is_deepseek_provider(provider_name) {
            if let Some(ref uid) = request.user_id {
                body["user"] = json!(uid);
            }
        }

        if !request.tools.is_empty() {
            // Responses API tools are FLAT — name/description/parameters sit
            // at the top level next to `type`, NOT nested under `function`
            // like chat completions. Nested shape fails server-side
            // deserialization with "tools[0]: missing field name".
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "name": t.function.name,
                        "description": t.function.description,
                        "parameters": t.function.parameters,
                    })
                })
                .collect();
            body["tools"] = json!(tools);
        }

        if let Some(ref rf) = request.response_format {
            match rf {
                crate::llm::provider::ResponseFormat::Text => {}
                crate::llm::provider::ResponseFormat::JsonObject => {
                    body["text"] = json!({"format": {"type": "json_object"}});
                }
                crate::llm::provider::ResponseFormat::JsonSchema { name, schema } => {
                    let mut v = json!({
                        "format": {
                            "type": "json_schema",
                            "name": name,
                        }
                    });
                    if let Some(s) = schema {
                        v["format"]["schema"] = json!(s);
                    }
                    body["text"] = v;
                }
            }
        }

        body
    }
}

/// Parse a non-streaming Responses API response.
pub(super) fn parse_responses_response(json: &Value, _model: &str) -> AppResult<LlmResponse> {
    let mut content = String::new();

    if let Some(output) = json.get("output").and_then(|o| o.as_array()) {
        for item in output {
            if item.get("type").and_then(|t| t.as_str()) == Some("message") {
                if let Some(blocks) = item.get("content").and_then(|c| c.as_array()) {
                    for block in blocks {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            if !content.is_empty() {
                                content.push('\n');
                            }
                            content.push_str(text);
                        }
                    }
                }
            }
        }
    }

    // Non-streaming usage — previously dropped (#88 audit H7). Responses
    // reports input_tokens/output_tokens with input_tokens_details.
    // cached_tokens (KV cache hits).
    let usage = json
        .get("usage")
        .map(crate::llm::streaming::parse_usage_object)
        .unwrap_or_default();

    Ok(LlmResponse { content, usage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{ConversationItem, ToolCall};
    use crate::llm::client::LlmClient;
    use crate::llm::provider::LlmRequest;
    use crate::llm::retry::RetryConfig;
    use std::sync::Arc;

    fn client() -> LlmClient {
        LlmClient::new(
            vec![],
            RetryConfig::default(),
            true,
            Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
            )),
        )
    }

    fn base_request() -> LlmRequest {
        LlmRequest {
            model: "gpt-5".into(),
            provider: Some("openai".into()),
            stream: false,
            max_tokens: Some(2048),
            ..Default::default()
        }
    }

    #[test]
    fn body_uses_responses_shape() {
        let request = LlmRequest {
            system_prompt: "be helpful".into(),
            messages: vec![ConversationItem::user("hi")],
            ..base_request()
        };
        let body = client().build_responses_body(&request, "openai");
        assert!(body.get("messages").is_none());
        assert!(body.get("input").is_some());
        assert_eq!(body["max_output_tokens"], json!(2048));
        assert!(body.get("max_tokens").is_none());
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[1]["role"], "user");
    }

    #[test]
    fn body_maps_tool_history_and_results() {
        let request = LlmRequest {
            messages: vec![
                ConversationItem::Assistant(crate::core::types::AssistantMessage {
                    content: "thinking".into(),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        arguments: r#"{"path":"a.txt"}"#.into(),
                    }],
                    model: None,
                    usage: None,
                    reasoning_content: None,
                }),
                ConversationItem::tool_result("call_1", "file content"),
            ],
            ..base_request()
        };
        let body = client().build_responses_body(&request, "openai");
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "assistant");
        // History tool calls are STANDALONE function_call items (the server
        // merges them into the adjacent assistant; inline tool_calls on the
        // assistant message are ignored — DeepSeek rejects the result
        // otherwise with "No tool call found for tool output").
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_1");
        assert_eq!(input[1]["name"], "read_file");
        assert!(input[0].get("tool_calls").is_none(), "no inline tool_calls");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[2]["output"], "file content");
    }

    #[test]
    fn dangling_calls_get_synthetic_output() {
        let request = LlmRequest {
            messages: vec![ConversationItem::Assistant(
                crate::core::types::AssistantMessage {
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "call_x".into(),
                        name: "bash".into(),
                        arguments: "{}".into(),
                    }],
                    model: None,
                    usage: None,
                    reasoning_content: None,
                },
            )],
            ..base_request()
        };
        let body = client().build_responses_body(&request, "openai");
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_x");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call_x");
    }

    #[test]
    fn reasoning_effort_maps_to_responses_shape() {
        let request = LlmRequest {
            reasoning_effort: Some("max".into()),
            ..base_request()
        };
        let body = client().build_responses_body(&request, "openai");
        assert_eq!(body["reasoning"]["effort"], json!("high"));
        assert_eq!(body["reasoning"]["exclude"], json!(["thinking"]));
    }

    #[test]
    fn deepseek_responses_effort_maps_to_max_without_exclude() {
        // DeepSeek's Responses API folds max→max (not high), and `exclude` is
        // not in its supported reasoning set — thinking streams back as items.
        let request = LlmRequest {
            model: "deepseek-v4-flash".into(),
            provider: Some("deepseek".into()),
            reasoning_effort: Some("max".into()),
            ..base_request()
        };
        let body = client().build_responses_body(&request, "deepseek");
        assert_eq!(body["reasoning"]["effort"], json!("max"));
        assert!(body["reasoning"].get("exclude").is_none());
    }

    #[test]
    fn deepseek_responses_effort_folding() {
        // Table-driven per the V4 manual: minimal/low→low, medium/high/xhigh→high,
        // max→max, none→thinking off (passed through).
        let cases = [
            ("minimal", "low"),
            ("low", "low"),
            ("medium", "high"),
            ("high", "high"),
            ("xhigh", "high"),
            ("max", "max"),
            ("none", "none"),
        ];
        for (input, expected) in cases {
            let request = LlmRequest {
                model: "deepseek-v4-flash".into(),
                provider: Some("deepseek".into()),
                reasoning_effort: Some(input.into()),
                ..base_request()
            };
            let body = client().build_responses_body(&request, "deepseek");
            assert_eq!(
                body["reasoning"]["effort"],
                json!(expected),
                "input {input}"
            );
            assert!(body["reasoning"].get("exclude").is_none(), "input {input}");
        }
    }

    #[test]
    fn no_effort_omits_reasoning_block() {
        for provider in ["deepseek", "openai"] {
            let request = LlmRequest {
                model: "deepseek-v4-flash".into(),
                provider: Some("deepseek".into()),
                reasoning_effort: None,
                ..base_request()
            };
            let body = client().build_responses_body(&request, provider);
            assert!(body.get("reasoning").is_none(), "provider {provider}");
        }
    }

    #[test]
    fn openai_responses_effort_keeps_exclude() {
        let request = LlmRequest {
            reasoning_effort: Some("low".into()),
            ..base_request()
        };
        let body = client().build_responses_body(&request, "openai");
        assert_eq!(body["reasoning"]["effort"], json!("low"));
        assert_eq!(body["reasoning"]["exclude"], json!(["thinking"]));
    }

    #[test]
    fn deepseek_responses_user_id_at_top_level() {
        let request = LlmRequest {
            model: "deepseek-v4-flash".into(),
            provider: Some("deepseek".into()),
            user_id: Some("sess_abc123".into()),
            ..base_request()
        };
        let body = client().build_responses_body(&request, "deepseek");
        assert_eq!(body["user"], json!("sess_abc123"));
    }

    #[test]
    fn openai_responses_skips_user_id() {
        let request = LlmRequest {
            user_id: Some("sess_abc123".into()),
            ..base_request()
        };
        let body = client().build_responses_body(&request, "openai");
        assert!(body.get("user").is_none());
    }

    #[test]
    fn body_tools_are_flat() {
        use crate::core::types::{FunctionTool, ToolDefinition, ToolType};

        // Responses API requires flat tool shape: name at the TOP level next
        // to type — the nested {function:{...}} shape (chat completions)
        // fails server deserialization with "tools[0]: missing field name".
        let request = LlmRequest {
            tools: vec![ToolDefinition {
                kind: ToolType::Function,
                function: FunctionTool {
                    name: "read_file".into(),
                    description: Some("Read a file".into()),
                    parameters: json!({"type": "object"}),
                },
            }],
            ..base_request()
        };
        let body = client().build_responses_body(&request, "openai");
        let tools = body["tools"].as_array().unwrap();
        let tool = &tools[0];
        assert_eq!(tool["type"], "function");
        assert_eq!(tool["name"], "read_file");
        assert_eq!(tool["description"], "Read a file");
        assert_eq!(tool["parameters"], json!({"type": "object"}));
        assert!(tool.get("function").is_none(), "no nested function object");
    }

    #[test]
    fn system_between_tool_group_is_deferred() {
        let request = LlmRequest {
            messages: vec![
                ConversationItem::Assistant(crate::core::types::AssistantMessage {
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        arguments: "{}".into(),
                    }],
                    model: None,
                    usage: None,
                    reasoning_content: None,
                }),
                ConversationItem::system("reminder"),
                ConversationItem::tool_result("call_1", "content"),
            ],
            ..base_request()
        };
        let body = client().build_responses_body(&request, "openai");
        let input = body["input"].as_array().unwrap();
        // assistant → function_call → function_call_output → deferred system
        // (in that order, the system message never breaks the tool group).
        assert_eq!(input[0]["role"], "assistant");
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[3]["role"], "system");
        assert_eq!(input[3]["content"][0]["text"], "reminder");
    }

    #[test]
    fn parses_non_streaming_response() {
        let json = json!({
            "id": "resp_1",
            "status": "completed",
            "output": [
                {"type": "message", "content": [{"type": "output_text", "text": "Hello"}]},
                {"type": "function_call", "id": "fc_1", "call_id": "call_9", "name": "grep", "arguments": "{\"pattern\":\"fn\"}"},
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "thinking..."}]}
            ],
            "usage": {
                "input_tokens": 10,
                "output_tokens": 5,
                "input_tokens_details": {"cached_tokens": 4},
                "output_tokens_details": {"reasoning_tokens": 2}
            }
        });
        let resp = parse_responses_response(&json, "gpt-5").unwrap();
        assert_eq!(resp.content, "Hello");
    }

    #[test]
    fn deepseek_responses_url_keeps_explicit_v1() {
        use crate::core::config::ProviderConfig;

        // A base configured WITH /v1 (a legacy-compatible alias) resolves to
        // /v1/responses as-is — the base is used exactly as configured.
        let provider = ProviderConfig {
            name: "deepseek".into(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            api_key: None,
            base_url: "https://api.deepseek.com/v1".into(),
            enabled: true,
            protocol: Some("responses".into()),
        };
        let client = LlmClient::new(
            vec![provider],
            RetryConfig::default(),
            true,
            Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
            )),
        );
        assert_eq!(
            client.request_url("deepseek", true).unwrap(),
            "https://api.deepseek.com/v1/responses"
        );
        assert_eq!(
            client.request_url("deepseek", false).unwrap(),
            "https://api.deepseek.com/v1/responses"
        );
    }

    #[test]
    fn deepseek_root_base_does_not_force_v1() {
        use crate::core::config::ProviderConfig;

        // DeepSeek's OpenAI-compatible endpoint is
        // https://api.deepseek.com/chat/completions — a bare-host base
        // resolves WITHOUT an injected /v1 (the /v1 prefix is only a
        // legacy-compatible alias, not a requirement).
        let provider = ProviderConfig {
            name: "deepseek".into(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            api_key: None,
            base_url: "https://api.deepseek.com".into(),
            enabled: true,
            protocol: Some("responses".into()),
        };
        let client = LlmClient::new(
            vec![provider],
            RetryConfig::default(),
            true,
            Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
            )),
        );
        assert_eq!(
            client.request_url("deepseek", true).unwrap(),
            "https://api.deepseek.com/responses"
        );
        // Chat completions resolves the same way — no /v1 injected.
        let provider = ProviderConfig {
            name: "deepseek".into(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            api_key: None,
            base_url: "https://api.deepseek.com".into(),
            enabled: true,
            protocol: None,
        };
        let client = LlmClient::new(
            vec![provider],
            RetryConfig::default(),
            true,
            Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
            )),
        );
        assert_eq!(
            client.request_url("deepseek", true).unwrap(),
            "https://api.deepseek.com/chat/completions"
        );
    }

    #[test]
    fn deepseek_anthropic_url_uses_anthropic_compat_endpoint() {
        use crate::core::config::ProviderConfig;

        // DeepSeek's Anthropic-compatible endpoint is
        // https://api.deepseek.com/anthropic/v1/messages. A default base
        // (…/v1) must resolve there; a manually configured /anthropic base
        // is used as-is.
        let provider = ProviderConfig {
            name: "deepseek".into(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            api_key: None,
            base_url: "https://api.deepseek.com/v1".into(),
            enabled: true,
            protocol: Some("anthropic".into()),
        };
        let client = LlmClient::new(
            vec![provider],
            RetryConfig::default(),
            true,
            Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
            )),
        );
        assert_eq!(
            client.request_url("deepseek", true).unwrap(),
            "https://api.deepseek.com/anthropic/v1/messages"
        );
        assert_eq!(
            client.request_url("deepseek", true).unwrap(),
            "https://api.deepseek.com/anthropic/v1/messages"
        );

        // User already pointed the base at the /anthropic endpoint.
        let provider = ProviderConfig {
            name: "deepseek".into(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            api_key: None,
            base_url: "https://api.deepseek.com/anthropic".into(),
            enabled: true,
            protocol: Some("anthropic".into()),
        };
        let client = LlmClient::new(
            vec![provider],
            RetryConfig::default(),
            true,
            Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
            )),
        );
        assert_eq!(
            client.request_url("deepseek", true).unwrap(),
            "https://api.deepseek.com/anthropic/v1/messages"
        );

        // Root base (no /v1, no /anthropic) — the shape that previously fell
        // through to the non-existent /v1/messages (404).
        let provider = ProviderConfig {
            name: "deepseek".into(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            api_key: None,
            base_url: "https://api.deepseek.com".into(),
            enabled: true,
            protocol: Some("anthropic".into()),
        };
        let client = LlmClient::new(
            vec![provider],
            RetryConfig::default(),
            true,
            Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
            )),
        );
        assert_eq!(
            client.request_url("deepseek", true).unwrap(),
            "https://api.deepseek.com/anthropic/v1/messages"
        );
    }

    #[test]
    fn openai_responses_url_keeps_v1() {
        use crate::core::config::ProviderConfig;

        let provider = ProviderConfig {
            name: "openai".into(),
            api_key_env: "OPENAI_API_KEY".into(),
            api_key: None,
            base_url: "https://api.openai.com/v1".into(),
            enabled: true,
            protocol: Some("responses".into()),
        };
        let client = LlmClient::new(
            vec![provider],
            RetryConfig::default(),
            true,
            Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
            )),
        );
        assert_eq!(
            client.request_url("openai", true).unwrap(),
            "https://api.openai.com/v1/responses"
        );
    }

    #[test]
    fn auto_protocol_keeps_openai_url() {
        use crate::core::config::ProviderConfig;

        let provider = ProviderConfig {
            name: "deepseek".into(),
            api_key_env: "DEEPSEEK_API_KEY".into(),
            api_key: None,
            base_url: "https://api.deepseek.com/v1".into(),
            enabled: true,
            protocol: None,
        };
        let client = LlmClient::new(
            vec![provider],
            RetryConfig::default(),
            true,
            Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
            )),
        );
        assert_eq!(
            client.request_url("deepseek", true).unwrap(),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }
}
