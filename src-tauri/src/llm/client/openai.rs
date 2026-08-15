//! OpenAI-compatible request body construction and response parsing.

use crate::core::error::AppResult;
use crate::core::types::ConversationItem;
use crate::llm::provider::{LlmRequest, LlmResponse, ResponseFormat};
use serde_json::{json, Value};

use super::LlmClient;

/// Whether a provider is DeepSeek.
///
/// Comparison is case-insensitive: provider names come from the user-editable
/// `ProviderConfig.name` (e.g. "DeepSeek") while internal references use the
/// lowercase id ("deepseek"). Shared with the anthropic/responses builders via
/// `super::openai::is_deepseek_provider`.
pub(super) fn is_deepseek_provider(provider_name: &str) -> bool {
    provider_name.eq_ignore_ascii_case("deepseek")
}

/// Whether a provider+model pair belongs to the DeepSeek V4 family — the
/// official "deepseek" provider OR any model id that is a DeepSeek/DSpark
/// checkpoint (self-hosted DSpark servers serve ids like
/// `deepseek-v4-flash-dspark` or `deepseek-ai/DeepSeek-V4-Pro-DSpark`).
///
/// Self-hosted DSpark endpoints are OpenAI-compatible but still DeepSeek V4
/// under the hood: they need the same `reasoning_content` echo on tool-call
/// turns (HTTP 400 otherwise) and the same stream `include_usage` opt-in
/// (else cache-hit/reasoning stats never arrive). Official-only isolation
/// fields (`user_id`) intentionally stay gated on `is_deepseek_provider` —
/// self-hosted vLLM has no DeepSeek content-safety semantics and unknown
/// body fields can be rejected.
pub(super) fn is_deepseek_family(provider_name: &str, model_id: &str) -> bool {
    is_deepseek_provider(provider_name) || is_deepseek_model(model_id)
}

/// Model-id rule for the DeepSeek V4 family (official V4 + DSpark
/// checkpoints). Case-insensitive.
fn is_deepseek_model(model_id: &str) -> bool {
    let lower = model_id.to_lowercase();
    lower.starts_with("deepseek") || lower.contains("dspark")
}

/// Whether an OpenAI-hosted model natively supports `reasoning_effort`
/// (GPT-5 / o3 family). Older models (GPT-4o…) reject the parameter.
fn supports_openai_effort(model_id: &str) -> bool {
    let lower = model_id.to_lowercase();
    lower.starts_with("gpt-5") || lower.starts_with("o3")
}

/// Merge orphaned reasoning text into an assistant message.
///
/// deepseek-native: legacy conversations persist reasoning as a separate
/// item next to the assistant message. DeepSeek rejects tool-call turns that
/// do not echo `reasoning_content` back on requests carrying `tools` (HTTP
/// 400), so the text must land on the assistant message itself.
fn merge_reasoning_into_assistant(msg: &mut Value, reasoning: &str) {
    let merged = match msg.get("reasoning_content").and_then(|v| v.as_str()) {
        Some(existing) if !existing.is_empty() => format!("{}\n{}", existing, reasoning),
        _ => reasoning.to_string(),
    };
    msg["reasoning_content"] = json!(merged);
}

impl LlmClient {
    /// Build the request body in OpenAI-compatible format.
    pub(super) fn build_openai_body(&self, request: &LlmRequest, provider_name: &str) -> Value {
        let mut messages: Vec<Value> = Vec::new();
        let mut last_tool_assistant: Option<usize> = None;
        // Hard structural guarantee: every assistant tool-call id must be
        // answered by a tool message, and tool messages must reference only
        // declared ids. OpenAI-compatible providers (DeepSeek included)
        // reject conversations violating this with HTTP 400, so the body
        // builder enforces it at the API boundary — even if a caller path
        // ever misses `repair_dangling_tool_calls`.
        // Entries: (declared id, index of the declaring assistant message in
        // `messages`) — synthetics are inserted IMMEDIATELY after that
        // assistant (adjacency rule), never appended at the end.
        let mut pending: Vec<(String, usize)> = Vec::new();
        // System messages emitted while a tool group is still being answered
        // (e.g. the per-tool-call DeepSeek anti-duplicate reminder). DeepSeek
        // requires tool responses to be CONSECUTIVE after the assistant that
        // declared the calls — a system message in between makes the group
        // look incomplete (HTTP 400 "insufficient tool messages"). These are
        // flushed right after the group's last response instead.
        let mut deferred_systems: Vec<Value> = Vec::new();

        if !request.system_prompt.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": request.system_prompt,
            }));
        }

        for item in &request.messages {
            match item {
                ConversationItem::System(s) => {
                    let sys = json!({"role": "system", "content": s.content});
                    if pending.is_empty() {
                        messages.push(sys);
                    } else {
                        // Tool group in flight — defer until it is answered.
                        deferred_systems.push(sys);
                    }
                }
                ConversationItem::User(u) => {
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
                                    "type": "image_url",
                                    "image_url": {
                                        "url": format!("data:{};base64,{}", media_type, data)
                                    }
                                })
                            }
                        })
                        .collect();
                    messages.push(json!({"role": "user", "content": content}));
                }
                ConversationItem::Assistant(a) => {
                    let assistant_idx = messages.len();
                    let mut msg = json!({"role": "assistant", "content": a.content});
                    if !a.tool_calls.is_empty() {
                        let tool_calls: Vec<Value> = a
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                pending.push((tc.id.clone(), assistant_idx));
                                json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.arguments,
                                    }
                                })
                            })
                            .collect();
                        msg["tool_calls"] = json!(tool_calls);
                    }
                    // deepseek-native: reasoning_content must be a field on the
                    // assistant message (alongside content/tool_calls), not a
                    // separate message. Required for tool-call turn round-trip.
                    if let Some(ref rc) = a.reasoning_content {
                        msg["reasoning_content"] = json!(rc);
                    }
                    messages.push(msg);
                    if !a.tool_calls.is_empty() {
                        last_tool_assistant = Some(messages.len() - 1);
                    }
                }
                ConversationItem::ToolResult(tr) => {
                    // Only keep results that answer a DECLARED tool call.
                    // Orphan results (call removed by compaction/repair) are
                    // dropped — they can only ever trigger provider errors.
                    if let Some(pos) = pending.iter().position(|(id, _)| id == &tr.tool_call_id) {
                        pending.remove(pos);
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": tr.tool_call_id,
                            "content": tr.content,
                        }));
                        // The current tool group is fully answered — flush any
                        // deferred system messages AFTER the last response so
                        // the responses stay consecutive.
                        if pending.is_empty() {
                            messages.append(&mut deferred_systems);
                        }
                    }
                }
                ConversationItem::Reasoning(r) => {
                    // deepseek-native: standalone Reasoning items are a legacy
                    // persistence format (pre-embedding conversations). DeepSeek
                    // requires tool-call turns to echo reasoning_content back on
                    // every request carrying tools — HTTP 400 otherwise. Merge
                    // it into the preceding assistant-with-tool-calls instead of
                    // dropping it. Other providers ignore reasoning_content, so
                    // they keep skipping these items untouched. DSpark family
                    // (self-hosted V4) has the same round-trip requirement.
                    if is_deepseek_family(provider_name, &request.model) {
                        if let Some(idx) = last_tool_assistant {
                            let msg = &mut messages[idx];
                            if msg["tool_calls"]
                                .as_array()
                                .map(|a| !a.is_empty())
                                .unwrap_or(false)
                            {
                                merge_reasoning_into_assistant(msg, &r.content);
                            }
                        }
                    }
                }
            }
        }

        // Any still-unanswered tool calls get a synthetic error result
        // inserted IMMEDIATELY after their declaring assistant message
        // (reverse order keeps per-assistant ordering stable). DeepSeek/
        // OpenAI require tool responses to be adjacent to the assistant
        // message — end-appended responses do NOT satisfy that.
        for (id, assistant_idx) in pending.iter().rev() {
            messages.insert(
                assistant_idx + 1,
                json!({
                    "role": "tool",
                    "tool_call_id": id,
                    "content": "[Tool execution was interrupted — the tool did not produce a result.]",
                }),
            );
        }

        // Leftover deferred system messages (group never completed) — append
        // after the synthetics so nothing is lost.
        messages.append(&mut deferred_systems);

        let mut body = json!({
            "model": request.model,
            "messages": messages,
            "stream": request.stream,
        });

        if let Some(t) = request.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(p) = request.top_p {
            body["top_p"] = json!(p);
        }
        if let Some(max) = request.max_tokens {
            body["max_tokens"] = json!(max);
        }
        // Reasoning effort routing, per provider capability:
        // - DeepSeek: effort + thinking (DeepSeek-native thinking mode).
        // - GPT-5 / o3 (OpenAI): native `reasoning_effort` (low/medium/high)
        //   — the input-bar effort picker works for them too.
        // - Everything else (Grok, Ollama, GPT-4o, custom): nothing — the
        //   parameter would be rejected or ignored.
        if let Some(ref effort) = request.reasoning_effort {
            if is_deepseek_provider(provider_name) {
                body["reasoning_effort"] = json!(effort);
                // deepseek-native: thinking field enables DeepSeek's thinking mode.
                body["thinking"] = json!({ "type": "enabled" });
            } else if supports_openai_effort(&request.model) {
                // OpenAI's enum is low/medium/high — map the DeepSeek-style
                // "max" (and auto-resolved "high") into its vocabulary.
                let mapped = match effort.as_str() {
                    "max" => "high",
                    other => other,
                };
                body["reasoning_effort"] = json!(mapped);
            }
        }

        // deepseek-native: streamed requests must opt into the final usage
        // block via stream_options.include_usage — otherwise DeepSeek never
        // sends prompt_cache_hit/miss_tokens or reasoning_tokens on the wire,
        // and the usage page's cache-hit-rate stats stay frozen at zero.
        if is_deepseek_family(provider_name, &request.model) && request.stream {
            body["stream_options"] = json!({ "include_usage": true });
        }

        // deepseek-native: per-user isolation (content-safety / KVCache /
        // scheduling). Only DeepSeek honors `user_id`; other providers get
        // nothing (OpenAI's `user` has a different abuse-tracking meaning).
        if is_deepseek_provider(provider_name) {
            if let Some(ref uid) = request.user_id {
                body["user_id"] = json!(uid);
            }
        }

        if !request.tools.is_empty() && provider_name != "ollama" {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.function.name,
                            "description": t.function.description,
                            "parameters": t.function.parameters,
                        }
                    })
                })
                .collect();
            body["tools"] = json!(tools);
        }

        if let Some(ref rf) = request.response_format {
            match rf {
                ResponseFormat::Text => {
                    body["response_format"] = json!({"type": "text"});
                }
                ResponseFormat::JsonObject => {
                    body["response_format"] = json!({"type": "json_object"});
                }
                ResponseFormat::JsonSchema { name, schema } => {
                    let mut v = json!({
                        "type": "json_schema",
                        "json_schema": { "name": name }
                    });
                    if let Some(s) = schema {
                        v["json_schema"]["schema"] = json!(s);
                    }
                    body["response_format"] = v;
                }
            }
        }

        body
    }
}

/// Parse an OpenAI-compatible response.
pub(super) fn parse_openai_response(json: &Value, _model: &str) -> AppResult<LlmResponse> {
    let choice = json
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|arr| arr.first())
        .ok_or_else(|| crate::core::error::AppError::LlmApi {
            source: "No choices in response".into(),
            status_code: None,
        })?;

    let content = choice
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    // Non-streaming usage — previously dropped, making complete() calls
    // invisible in usage accounting (#88 audit H7).
    let usage = json
        .get("usage")
        .map(crate::llm::streaming::parse_usage_object)
        .unwrap_or_default();

    Ok(LlmResponse { content, usage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{ReasoningMessage, ToolCall};
    use crate::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
    use crate::llm::retry::RetryConfig;
    use std::sync::Arc;

    fn client() -> LlmClient {
        LlmClient::new(
            vec![],
            RetryConfig::default(),
            false,
            Arc::new(CircuitBreaker::new(CircuitBreakerConfig::default())),
        )
    }

    fn tool_call_request() -> LlmRequest {
        LlmRequest {
            model: "deepseek-v4-pro".into(),
            provider: Some("deepseek".into()),
            tools: vec![],
            messages: vec![
                ConversationItem::user("hi"),
                ConversationItem::Assistant(crate::core::types::AssistantMessage {
                    content: "checking".into(),
                    tool_calls: vec![ToolCall {
                        id: "tc-1".into(),
                        name: "read_file".into(),
                        arguments: "{}".into(),
                    }],
                    model: None,
                    usage: None,
                    reasoning_content: None,
                }),
                ConversationItem::Reasoning(ReasoningMessage {
                    content: "thinking...".into(),
                    encrypted_content: None,
                }),
                ConversationItem::tool_result("tc-1", "result"),
            ],
            system_prompt: String::new(),
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        }
    }

    #[test]
    fn deepseek_provider_match_is_case_insensitive() {
        assert!(is_deepseek_provider("deepseek"));
        assert!(is_deepseek_provider("DeepSeek"));
        assert!(is_deepseek_provider("DEEPSEEK"));
        assert!(!is_deepseek_provider("openai"));
        assert!(!is_deepseek_provider("anthropic"));
    }

    #[test]
    fn deepseek_model_rule_covers_dspark_checkpoints() {
        // Self-hosted DSpark servers are OpenAI-compatible but the model is
        // still DeepSeek V4 — the family rule must catch official V4 ids,
        // DSpark checkpoints and HF-style full ids.
        assert!(is_deepseek_model("deepseek-v4-flash"));
        assert!(is_deepseek_model("deepseek-v4-pro-dspark"));
        assert!(is_deepseek_model("deepseek-ai/DeepSeek-V4-Flash-DSpark"));
        assert!(is_deepseek_model("DEEPSEEK-V4-PRO-DSPARK"));
        // Non-DeepSeek models must stay outside the family.
        assert!(!is_deepseek_model("gpt-5"));
        assert!(!is_deepseek_model("qwen3-14b"));
        assert!(!is_deepseek_model("claude-sonnet-4-20250514"));
        assert!(!is_deepseek_model(""));
    }

    #[test]
    fn dspark_self_hosted_merges_reasoning_and_usage_without_official_fields() {
        // A custom provider serving a DSpark checkpoint is DeepSeek family:
        // reasoning echo + stream usage opt-in apply (both prevent broken
        // tool turns / empty stats), but official-only fields (user_id,
        // thinking/effort) are NOT invented for a self-hosted vLLM.
        let mut request = tool_call_request();
        request.model = "deepseek-v4-flash-dspark".into();
        request.provider = Some("local-dspark".into());
        request.stream = true;
        request.user_id = Some("sess_abc123".into());
        request.reasoning_effort = Some("max".into());
        let body = client().build_openai_body(&request, "Local DSpark");

        let messages = body["messages"].as_array().unwrap();
        let assistant = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant message present");
        assert_eq!(assistant["reasoning_content"], "thinking...");
        assert_eq!(body["stream_options"]["include_usage"], json!(true));
        assert!(body.get("user_id").is_none(), "user_id stays official-only");
        assert!(
            body.get("reasoning_effort").is_none(),
            "effort stays official-only"
        );
        assert!(
            body.get("thinking").is_none(),
            "thinking stays official-only"
        );
    }

    #[test]
    fn deepseek_merges_legacy_reasoning_into_tool_assistant() {
        let body = client().build_openai_body(&tool_call_request(), "DeepSeek");
        let messages = body["messages"].as_array().unwrap();
        let assistant = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant message present");
        assert_eq!(assistant["reasoning_content"], "thinking...");
        assert!(assistant["tool_calls"].as_array().unwrap().len() == 1);
        // Legacy Reasoning item must not leak as its own message.
        assert!(!messages.iter().any(|m| m["role"] == "reasoning"));
    }

    #[test]
    fn deepseek_thinking_injected_with_any_case_provider_name() {
        let mut request = tool_call_request();
        request.reasoning_effort = Some("high".into());
        let body = client().build_openai_body(&request, "DeepSeek");
        assert_eq!(body["thinking"], json!({ "type": "enabled" }));
        assert_eq!(body["reasoning_effort"], json!("high"));
    }

    #[test]
    fn deepseek_stream_requests_opt_into_usage() {
        // The usage page's cache-hit rate depends on this flag: without it
        // DeepSeek never sends prompt_cache_hit/miss_tokens on a stream.
        let mut request = tool_call_request();
        request.stream = true;
        let body = client().build_openai_body(&request, "deepseek");
        assert_eq!(body["stream_options"]["include_usage"], json!(true));
    }

    #[test]
    fn deepseek_non_stream_requests_skip_usage_opt_in() {
        // Non-streaming completions return usage unconditionally — no flag.
        let mut request = tool_call_request();
        request.stream = false;
        let body = client().build_openai_body(&request, "deepseek");
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn non_deepseek_providers_skip_usage_opt_in() {
        // The flag is DeepSeek-specific — other OpenAI-compatible providers
        // are not verified and do not emit the cache fields we parse.
        let mut request = tool_call_request();
        request.model = "gpt-4o".into();
        request.stream = true;
        let body = client().build_openai_body(&request, "openai");
        assert!(body.get("stream_options").is_none());
    }

    #[test]
    fn deepseek_user_id_injected_for_isolation() {
        let mut request = tool_call_request();
        request.user_id = Some("sess_abc123".into());
        let body = client().build_openai_body(&request, "deepseek");
        assert_eq!(body["user_id"], json!("sess_abc123"));
    }

    #[test]
    fn user_id_skipped_for_other_providers() {
        // OpenAI's `user` has a different abuse-tracking meaning — the
        // DeepSeek isolation field must never leak to non-DeepSeek.
        let mut request = tool_call_request();
        request.user_id = Some("sess_abc123".into());
        let body = client().build_openai_body(&request, "openai");
        assert!(body.get("user_id").is_none());
    }

    #[test]
    fn reasoning_effort_is_provider_capability_based() {
        // DeepSeek gets effort + thinking; GPT-5/o3 get the OpenAI-native
        // effort (mapped, no thinking); everyone else (Grok, Ollama, GPT-4o,
        // custom) gets nothing — the parameter would be rejected/ignored.
        let mut request = tool_call_request();
        request.reasoning_effort = Some("max".into());
        let body = client().build_openai_body(&request, "openai");
        assert!(body.get("reasoning_effort").is_none());
        assert!(body.get("thinking").is_none());
        let body_grok = client().build_openai_body(&request, "grok");
        assert!(body_grok.get("reasoning_effort").is_none());
        let body_ollama = client().build_openai_body(&request, "ollama");
        assert!(body_ollama.get("reasoning_effort").is_none());
        // GPT-4o does NOT support the parameter.
        request.model = "gpt-4o".into();
        let body_4o = client().build_openai_body(&request, "openai");
        assert!(body_4o.get("reasoning_effort").is_none());
        // GPT-5 does — "max" maps into the low/medium/high vocabulary.
        request.model = "gpt-5".into();
        let body_gpt5 = client().build_openai_body(&request, "openai");
        assert_eq!(body_gpt5["reasoning_effort"], json!("high"));
        assert!(
            body_gpt5.get("thinking").is_none(),
            "no thinking field for OpenAI"
        );
        // "high" passes through unchanged.
        request.reasoning_effort = Some("high".into());
        let body_gpt5_high = client().build_openai_body(&request, "openai");
        assert_eq!(body_gpt5_high["reasoning_effort"], json!("high"));
    }

    #[test]
    fn non_deepseek_provider_skips_legacy_reasoning_untouched() {
        let mut request = tool_call_request();
        request.model = "gpt-4o".into();
        request.provider = Some("openai".into());
        let body = client().build_openai_body(&request, "openai");
        let messages = body["messages"].as_array().unwrap();
        let assistant = messages
            .iter()
            .find(|m| m["role"] == "assistant")
            .expect("assistant message present");
        assert!(assistant.get("reasoning_content").is_none());
        assert!(body.get("thinking").is_none());
        // Other providers keep the historical skip behavior.
        assert!(!messages.iter().any(|m| m["role"] == "reasoning"));
    }

    #[test]
    fn merge_appends_to_existing_reasoning() {
        let mut msg = json!({ "role": "assistant", "content": "", "reasoning_content": "first" });
        merge_reasoning_into_assistant(&mut msg, "second");
        assert_eq!(msg["reasoning_content"], "first\nsecond");
    }

    #[test]
    fn dangling_tool_calls_get_synthetic_results() {
        // Reproduces the DeepSeek HTTP 400 ("insufficient tool messages
        // following tool_calls message") scenario: an interrupted turn leaves
        // an assistant tool-call message with no results at all.
        let request = LlmRequest {
            model: "deepseek-v4-flash".into(),
            provider: Some("deepseek".into()),
            tools: vec![],
            messages: vec![
                ConversationItem::user("fix the bug"),
                ConversationItem::Assistant(crate::core::types::AssistantMessage {
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "read_file".into(),
                        arguments: "{\"path\":\"src/lib.rs\"}".into(),
                    }],
                    model: None,
                    usage: None,
                    reasoning_content: None,
                }),
            ],
            system_prompt: String::new(),
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };

        let body = client().build_openai_body(&request, "deepseek");
        let messages = body["messages"].as_array().unwrap();
        let tool_responses: Vec<&Value> = messages.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(
            tool_responses.len(),
            1,
            "dangling tool call must be answered by a synthetic tool response"
        );
        assert_eq!(tool_responses[0]["tool_call_id"], "call_1");
        assert_eq!(messages[messages.len() - 1]["role"], "tool");
    }

    #[test]
    fn system_messages_between_tool_responses_are_deferred() {
        // Reproduces the exact failing pattern from production logs: a turn
        // with MULTIPLE serial tool calls, where the DeepSeek anti-duplicate
        // reminder is pushed after EACH result. Interleaved system messages
        // break DeepSeek's consecutive-tool-response rule → HTTP 400.
        let mut messages: Vec<ConversationItem> = vec![
            ConversationItem::user("apply edits"),
            ConversationItem::Assistant(crate::core::types::AssistantMessage {
                content: "".into(),
                tool_calls: (0..6)
                    .map(|i| ToolCall {
                        id: format!("call_{i}"),
                        name: "search_replace".into(),
                        arguments: "{}".into(),
                    })
                    .collect(),
                model: None,
                usage: None,
                reasoning_content: None,
            }),
        ];
        for i in 0..6 {
            messages.push(ConversationItem::tool_result(format!("call_{i}"), "ok"));
            messages.push(ConversationItem::system(format!(
                "[SYSTEM REMINDER] result {i}"
            )));
        }

        let request = LlmRequest {
            model: "deepseek-v4-flash".into(),
            provider: Some("deepseek".into()),
            messages,
            system_prompt: String::new(),
            tools: vec![],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };

        let body = client().build_openai_body(&request, "deepseek");
        let msgs = body["messages"].as_array().unwrap();

        // [0]=user, [1]=assistant, [2..7]=6 consecutive tool responses.
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["tool_calls"].as_array().unwrap().len(), 6);
        for (i, msg) in msgs.iter().enumerate().skip(2).take(6) {
            assert_eq!(msg["role"], "tool", "tool responses must be consecutive");
            assert_eq!(msg["tool_call_id"], format!("call_{}", i - 2));
        }
        // Then the 6 deferred reminders, in original order.
        assert_eq!(msgs[8]["role"], "system");
        assert_eq!(msgs[13]["role"], "system");
        assert_eq!(msgs[13]["content"], "[SYSTEM REMINDER] result 5");
        assert_eq!(msgs.len(), 14);
    }

    #[test]
    fn mid_conversation_dangling_call_inserts_in_place() {
        // A dangling call in the MIDDLE (followed by another user message)
        // must be answered adjacent to its assistant — end-appended responses
        // would still violate the adjacency rule.
        let request = LlmRequest {
            model: "deepseek-v4-flash".into(),
            provider: Some("deepseek".into()),
            messages: vec![
                ConversationItem::user("first"),
                ConversationItem::Assistant(crate::core::types::AssistantMessage {
                    content: "".into(),
                    tool_calls: vec![ToolCall {
                        id: "mid".into(),
                        name: "bash".into(),
                        arguments: "{}".into(),
                    }],
                    model: None,
                    usage: None,
                    reasoning_content: None,
                }),
                ConversationItem::user("interrupt message"),
            ],
            system_prompt: String::new(),
            tools: vec![],
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };

        let body = client().build_openai_body(&request, "deepseek");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4);
        // tool response sits between its assistant and the next user message.
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["role"], "tool");
        assert_eq!(messages[2]["tool_call_id"], "mid");
        assert_eq!(messages[3]["role"], "user");
        assert_eq!(messages[3]["content"][0]["text"], "interrupt message");
    }

    #[test]
    fn orphan_tool_results_are_dropped() {
        // A result referencing an undeclared call id must not leak into the
        // body (it would reference a non-existent tool_call_id).
        let request = LlmRequest {
            model: "deepseek-v4-flash".into(),
            provider: Some("deepseek".into()),
            tools: vec![],
            messages: vec![
                ConversationItem::user("hello"),
                ConversationItem::tool_result("call_ghost", "orphan result"),
            ],
            system_prompt: String::new(),
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };

        let body = client().build_openai_body(&request, "deepseek");
        let messages = body["messages"].as_array().unwrap();
        assert!(
            !messages.iter().any(|m| m["role"] == "tool"),
            "orphan tool result must be dropped"
        );
    }

    #[test]
    fn answered_tool_calls_keep_exact_order() {
        // Normal well-formed conversation: no synthetic padding, order intact.
        let request = LlmRequest {
            model: "deepseek-v4-flash".into(),
            provider: Some("deepseek".into()),
            tools: vec![],
            messages: vec![
                ConversationItem::user("check the code"),
                ConversationItem::Assistant(crate::core::types::AssistantMessage {
                    content: "".into(),
                    tool_calls: vec![
                        ToolCall {
                            id: "c1".into(),
                            name: "read_file".into(),
                            arguments: "{}".into(),
                        },
                        ToolCall {
                            id: "c2".into(),
                            name: "grep".into(),
                            arguments: "{}".into(),
                        },
                    ],
                    model: None,
                    usage: None,
                    reasoning_content: None,
                }),
                ConversationItem::tool_result("c1", "file content"),
                ConversationItem::tool_result("c2", "matches"),
                ConversationItem::Assistant(crate::core::types::AssistantMessage {
                    content: "found it".into(),
                    tool_calls: vec![],
                    model: None,
                    usage: None,
                    reasoning_content: None,
                }),
            ],
            system_prompt: String::new(),
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };

        let body = client().build_openai_body(&request, "deepseek");
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[1]["role"], "assistant");
        assert_eq!(messages[2]["tool_call_id"], "c1");
        assert_eq!(messages[3]["tool_call_id"], "c2");
        assert_eq!(messages[4]["role"], "assistant");
    }
}
