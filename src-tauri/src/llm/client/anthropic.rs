//! Anthropic API request body construction and response parsing.

use crate::core::error::AppResult;
use crate::core::types::ConversationItem;
use crate::llm::provider::{LlmRequest, LlmResponse};
use serde_json::{json, Value};
use tracing::warn;

use super::LlmClient;

/// Anthropic only accepts tool-call IDs in `[a-zA-Z0-9_-]` — anything else
/// (colons, dots, unicode) is rejected with HTTP 400. Sanitize every id
/// written into a `tool_use` / `tool_result` block. Mirrors Cat's
/// `sanitize_tool_call_id`.
pub(super) fn sanitize_tool_call_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Flush a pending content-block buffer into a message of the given role.
fn flush_blocks(blocks: &mut Vec<Value>, messages: &mut Vec<Value>, role: &str) {
    if !blocks.is_empty() {
        messages.push(json!({"role": role, "content": std::mem::take(blocks)}));
    }
}

impl LlmClient {
    /// Build the request body in Anthropic format.
    ///
    /// Mirrors Cat's conversation conversion: system blocks accumulate in the
    /// top-level `system` param; assistant text + tool_use blocks merge into
    /// ONE assistant message; tool results merge into ONE user message placed
    /// immediately after the assistant that declared the calls; tool-call ids
    /// are sanitized; dangling calls get a synthetic result.
    pub(super) fn build_anthropic_body(&self, request: &LlmRequest, provider_name: &str) -> Value {
        let mut messages: Vec<Value> = Vec::new();
        let mut system_blocks: Vec<Value> = Vec::new();
        let mut pending_assistant: Vec<Value> = Vec::new();
        let mut pending_tool_results: Vec<Value> = Vec::new();
        // Declared tool_use ids (Anthropic requires a matching tool_result).
        let mut declared: Vec<(String, usize)> = Vec::new();

        if !request.system_prompt.is_empty() {
            system_blocks.push(json!({"type": "text", "text": request.system_prompt}));
        }

        for item in &request.messages {
            match item {
                ConversationItem::System(s) => {
                    // System messages (transient reminders, compaction
                    // summaries) accumulate in the top-level system param —
                    // they never break the assistant/tool_result adjacency.
                    flush_blocks(&mut pending_assistant, &mut messages, "assistant");
                    flush_blocks(&mut pending_tool_results, &mut messages, "user");
                    system_blocks.push(json!({"type": "text", "text": s.content}));
                }
                ConversationItem::User(u) => {
                    flush_blocks(&mut pending_assistant, &mut messages, "assistant");
                    flush_blocks(&mut pending_tool_results, &mut messages, "user");
                    let content: Vec<Value> = u
                        .content
                        .iter()
                        .map(|part| match part {
                            crate::core::types::ContentPart::Text { text } => {
                                json!({"type": "text", "text": text})
                            }
                            crate::core::types::ContentPart::Image {
                                media_type, data, ..
                            } => {
                                json!({
                                    "type": "image",
                                    "source": {
                                        "type": "base64",
                                        "media_type": media_type,
                                        "data": data,
                                    }
                                })
                            }
                        })
                        .collect();
                    messages.push(json!({"role": "user", "content": content}));
                }
                ConversationItem::Assistant(a) => {
                    // A new assistant turn closes any pending tool-results
                    // group first — tool results must precede the next turn.
                    flush_blocks(&mut pending_tool_results, &mut messages, "user");
                    if !a.content.is_empty() {
                        pending_assistant.push(json!({"type": "text", "text": a.content}));
                    }
                    for tc in &a.tool_calls {
                        let safe_id = sanitize_tool_call_id(&tc.id);
                        declared.push((safe_id.clone(), messages.len()));
                        let input = match serde_json::from_str::<Value>(&tc.arguments) {
                            Ok(v) => v,
                            Err(e) => {
                                warn!(
                                    tool = %tc.name,
                                    error = %e,
                                    raw = %tc.arguments,
                                    "Invalid JSON arguments in conversation history — forwarding raw string to API"
                                );
                                json!({ "_malformed_arguments_raw": tc.arguments })
                            }
                        };
                        pending_assistant.push(json!({
                            "type": "tool_use",
                            "id": safe_id,
                            "name": tc.name,
                            "input": input,
                        }));
                    }
                }
                ConversationItem::ToolResult(tr) => {
                    flush_blocks(&mut pending_assistant, &mut messages, "assistant");
                    let safe_id = sanitize_tool_call_id(&tr.tool_call_id);
                    // Only keep results that answer a DECLARED tool call —
                    // orphan results (removed by compaction/repair) are
                    // dropped, they can only trigger provider errors.
                    if let Some(pos) = declared.iter().position(|(id, _)| *id == safe_id) {
                        declared.remove(pos);
                        pending_tool_results.push(json!({
                            "type": "tool_result",
                            "tool_use_id": safe_id,
                            "content": tr.content,
                            "is_error": tr.is_error,
                        }));
                    }
                }
                ConversationItem::Reasoning(_) => {
                    // Reasoning is deliberately dropped: Anthropic re-thinks
                    // each turn and thinking-block input requires a beta
                    // header we do not send.
                }
            }
        }
        flush_blocks(&mut pending_assistant, &mut messages, "assistant");
        flush_blocks(&mut pending_tool_results, &mut messages, "user");

        // Any still-unanswered tool calls get a synthetic result appended —
        // Anthropic rejects tool_use blocks without a matching tool_result.
        if !declared.is_empty() {
            let mut synthetics: Vec<Value> = Vec::new();
            for (id, _) in &declared {
                synthetics.push(json!({
                    "type": "tool_result",
                    "tool_use_id": id,
                    "content": "[Tool execution was interrupted — the tool did not produce a result.]",
                    "is_error": true,
                }));
            }
            messages.push(json!({"role": "user", "content": synthetics}));
        }

        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
            "stream": request.stream,
        });

        // System — array form with a cache_control breakpoint on the FIRST
        // block so the system prefix is served from cache on later turns.
        if !system_blocks.is_empty() {
            let cache_mode = request
                .cache_control
                .unwrap_or(crate::llm::provider::CacheControlMode::Ephemeral);
            if self.prompt_caching_enabled || request.cache_control.is_some() {
                system_blocks[0]["cache_control"] = cache_mode.to_anthropic_json();
                body["system"] = json!(system_blocks);
            } else if system_blocks.len() == 1 {
                body["system"] = system_blocks[0]["text"].clone();
            } else {
                body["system"] = json!(system_blocks);
            }
        }

        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(p) = request.top_p {
            body["top_p"] = json!(p);
        }

        // Reasoning effort — per provider (DeepSeek V4 manual, anthropic_api.md):
        // - DeepSeek's Anthropic-compatible endpoint honors `output_config.effort`
        //   (low/high/max) — the only supported reasoning field. Thinking mode is
        //   on by default, so emitting an effort implies thinking is enabled.
        // - Real Anthropic (claude) has no such param (`thinking` + budget_tokens
        //   is its control, a different mechanism we deliberately don't touch). A
        //   DeepSeek-only field would be meaningless there, so nothing is sent.
        // Values arrive pre-folded from intent_effort / explicit tiers
        // ({low,high,max}) and match output_config.effort's vocabulary verbatim —
        // the server applies the flash/pro mapping table (pro folds low→high).
        if let Some(ref effort) = request.reasoning_effort {
            if super::openai::is_deepseek_provider(provider_name) {
                body["output_config"] = json!({ "effort": effort });
            }
        }

        // deepseek-native: per-user isolation via metadata.user_id (the only
        // supported metadata field on DeepSeek's Anthropic-compatible
        // endpoint). Real Anthropic ignores the field; skip it there.
        if super::openai::is_deepseek_provider(provider_name) {
            if let Some(ref uid) = request.user_id {
                body["metadata"] = json!({ "user_id": uid });
            }
        }

        // Tools — add a cache_control breakpoint on the last tool definition
        // so the entire tools array is cached across turns (tools rarely change
        // within a session).
        if !request.tools.is_empty() {
            let mut tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.function.name,
                        "description": t.function.description,
                        "input_schema": t.function.parameters,
                    })
                })
                .collect();

            let cache_mode = request
                .cache_control
                .unwrap_or(crate::llm::provider::CacheControlMode::Ephemeral);

            if self.prompt_caching_enabled || request.cache_control.is_some() {
                let last_idx = tools.len() - 1;
                tools[last_idx]["cache_control"] = cache_mode.to_anthropic_json();
            }

            body["tools"] = json!(tools);
        }

        // Messages — place a cache breakpoint on the second-to-last message's
        // final content block. This caches the stable conversation prefix;
        // only the latest user message or tool result is uncached on each turn.
        let msg_cache_mode = request
            .cache_control
            .unwrap_or(crate::llm::provider::CacheControlMode::Ephemeral);

        if self.prompt_caching_enabled || request.cache_control.is_some() {
            if let Some(msgs) = body.get_mut("messages").and_then(|m| m.as_array_mut()) {
                if msgs.len() >= 2 {
                    let cache_idx = msgs.len() - 2;
                    if let Some(content) = msgs[cache_idx]
                        .get_mut("content")
                        .and_then(|c| c.as_array_mut())
                    {
                        if let Some(last_block) = content.last_mut() {
                            last_block["cache_control"] = msg_cache_mode.to_anthropic_json();
                        }
                    }
                }
            }
        }

        body
    }
}

/// Parse an Anthropic response.
pub(super) fn parse_anthropic_response(json: &Value, _model: &str) -> AppResult<LlmResponse> {
    let mut content = String::new();

    if let Some(content_blocks) = json.get("content").and_then(|c| c.as_array()) {
        for block in content_blocks {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if block_type == "text" {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    content.push_str(text);
                }
            }
        }
    }

    // Non-streaming usage — previously dropped (#88 audit H7). Anthropic's
    // usage object carries input_tokens/output_tokens/cache_creation_input_
    // tokens/cache_read_input_tokens; map the read side onto the cache-hit
    // field so the usage ring/aggregate sees the KV discount.
    let mut usage = crate::core::types::TokenUsage::default();
    if let Some(u) = json.get("usage") {
        usage.prompt_tokens = u.get("input_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
        usage.completion_tokens = u.get("output_tokens").and_then(|t| t.as_u64()).unwrap_or(0);
        let cache_read = u
            .get("cache_read_input_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        if cache_read > 0 {
            usage.prompt_cache_hit_tokens = Some(cache_read);
        }
    }

    Ok(LlmResponse { content, usage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{ConversationItem, ToolCall};
    use crate::llm::client::LlmClient;
    use crate::llm::retry::RetryConfig;
    use std::sync::Arc;

    fn client() -> LlmClient {
        LlmClient::new(
            vec![],
            RetryConfig::default(),
            false,
            Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
            )),
        )
    }

    fn request_with(messages: Vec<ConversationItem>) -> LlmRequest {
        LlmRequest {
            model: "claude-3-5-sonnet".into(),
            provider: Some("anthropic".into()),
            system_prompt: "be helpful".into(),
            messages,
            stream: false,
            ..Default::default()
        }
    }

    /// A DeepSeek request headed to the Anthropic-compatible endpoint.
    fn deepseek_request(effort: Option<&str>) -> LlmRequest {
        let mut r = request_with(vec![]);
        r.model = "deepseek-v4-pro".into();
        r.provider = Some("deepseek".into());
        r.reasoning_effort = effort.map(|s| s.to_string());
        r
    }

    #[test]
    fn sanitize_tool_call_id_strips_illegal_chars() {
        assert_eq!(sanitize_tool_call_id("call_abc-123"), "call_abc-123");
        assert_eq!(sanitize_tool_call_id("a.b:c,d"), "a_b_c_d");
        assert_eq!(sanitize_tool_call_id("工具调用"), "____");
    }

    #[test]
    fn system_goes_to_top_level_param() {
        let body = client().build_anthropic_body(&request_with(vec![]), "anthropic");
        assert_eq!(body["system"], json!("be helpful"));
        assert!(body.get("messages").unwrap().as_array().unwrap().is_empty());
    }

    #[test]
    fn parallel_tool_results_merge_into_one_user_message() {
        // Assistant declares TWO tool calls; both results must land in ONE
        // user message immediately after the assistant (Anthropic rejects
        // split results / non-adjacent tool_result).
        let body = client().build_anthropic_body(
            &request_with(vec![
                ConversationItem::Assistant(crate::core::types::AssistantMessage {
                    content: "checking".into(),
                    tool_calls: vec![
                        ToolCall {
                            id: "call_1".into(),
                            name: "read_file".into(),
                            arguments: r#"{"path":"a.txt"}"#.into(),
                        },
                        ToolCall {
                            id: "call_2".into(),
                            name: "grep".into(),
                            arguments: r#"{"pattern":"fn"}"#.into(),
                        },
                    ],
                    model: None,
                    usage: None,
                    reasoning_content: None,
                }),
                ConversationItem::tool_result("call_1", "content-a"),
                ConversationItem::tool_result("call_2", "content-b"),
            ]),
            "anthropic",
        );
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2, "assistant + one merged user message");
        assert_eq!(msgs[0]["role"], "assistant");
        let tool_uses = msgs[0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|b| b["type"] == "tool_use")
            .count();
        assert_eq!(tool_uses, 2);
        assert_eq!(msgs[1]["role"], "user");
        let results = msgs[1]["content"].as_array().unwrap();
        assert_eq!(results.len(), 2, "both tool_results in one user message");
        assert_eq!(results[0]["tool_use_id"], "call_1");
        assert_eq!(results[1]["tool_use_id"], "call_2");
    }

    #[test]
    fn dangling_tool_calls_get_synthetic_results() {
        let body = client().build_anthropic_body(
            &request_with(vec![ConversationItem::Assistant(
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
            )]),
            "anthropic",
        );
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1]["role"], "user");
        let results = msgs[1]["content"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["tool_use_id"], "call_x");
        assert_eq!(results[0]["is_error"], true);
    }

    #[test]
    fn orphan_tool_results_are_dropped() {
        let body = client().build_anthropic_body(
            &request_with(vec![ConversationItem::tool_result("ghost_call", "orphan")]),
            "anthropic",
        );
        let msgs = body["messages"].as_array().unwrap();
        assert!(msgs.is_empty(), "orphan result must not reach the API");
    }

    #[test]
    fn system_between_tool_group_does_not_break_adjacency() {
        let body = client().build_anthropic_body(
            &request_with(vec![
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
                ConversationItem::system("reminder text"),
                ConversationItem::tool_result("call_1", "content"),
            ]),
            "anthropic",
        );
        let msgs = body["messages"].as_array().unwrap();
        // assistant(tool_use) then user(tool_result) — the system message
        // goes to the top-level system param, never between them.
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "assistant");
        assert_eq!(msgs[1]["role"], "user");
        let system = body["system"].as_array().unwrap();
        assert!(system.iter().any(|b| b["text"] == "reminder text"));
    }

    #[test]
    fn tool_use_ids_are_sanitized_in_body() {
        let body = client().build_anthropic_body(
            &request_with(vec![
                ConversationItem::Assistant(crate::core::types::AssistantMessage {
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "a:b.c".into(),
                        name: "read_file".into(),
                        arguments: "{}".into(),
                    }],
                    model: None,
                    usage: None,
                    reasoning_content: None,
                }),
                ConversationItem::tool_result("a:b.c", "content"),
            ]),
            "anthropic",
        );
        let msgs = body["messages"].as_array().unwrap();
        let tool_use = &msgs[0]["content"].as_array().unwrap()[0];
        assert_eq!(tool_use["id"], "a_b_c");
        assert_eq!(
            msgs[1]["content"].as_array().unwrap()[0]["tool_use_id"],
            "a_b_c"
        );
    }

    #[test]
    fn parses_tool_use_response() {
        let json = json!({
            "content": [
                {"type": "text", "text": "Let me check."},
                {"type": "tool_use", "id": "call_9", "name": "grep", "input": {"pattern": "fn"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_read_input_tokens": 4}
        });
        let resp = parse_anthropic_response(&json, "claude-3-5-sonnet").unwrap();
        assert_eq!(resp.content, "Let me check.");
    }

    #[test]
    fn deepseek_anthropic_effort_sets_output_config() {
        // DeepSeek's Anthropic-compatible endpoint honors output_config.effort
        // (the only supported reasoning field) — the user's tier must reach it.
        // "DeepSeek" mixed case verifies the provider check is case-insensitive.
        let body = client().build_anthropic_body(&deepseek_request(Some("max")), "DeepSeek");
        assert_eq!(body["output_config"]["effort"], json!("max"));
    }

    #[test]
    fn real_anthropic_effort_is_ignored() {
        // Real Anthropic (claude) has no output_config — sending the
        // DeepSeek-only field there would be meaningless.
        let mut r = request_with(vec![]);
        r.reasoning_effort = Some("max".into());
        let body = client().build_anthropic_body(&r, "anthropic");
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn no_effort_skips_output_config() {
        let body = client().build_anthropic_body(&deepseek_request(None), "deepseek");
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn deepseek_anthropic_user_id_in_metadata() {
        let mut r = deepseek_request(None);
        r.user_id = Some("sess_abc123".into());
        let body = client().build_anthropic_body(&r, "deepseek");
        assert_eq!(body["metadata"]["user_id"], json!("sess_abc123"));
    }

    #[test]
    fn real_anthropic_skips_user_id_metadata() {
        let mut r = request_with(vec![]);
        r.user_id = Some("sess_abc123".into());
        let body = client().build_anthropic_body(&r, "anthropic");
        assert!(body.get("metadata").is_none());
    }
}
