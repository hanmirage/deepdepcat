//! REAL-API streaming smoke tests (ignored by default — require a live
//! `DEEPSEEK_API_KEY`).
//!
//! These exercise the provider streaming layer exactly as the agent loop
//! consumes it (`LlmClient::stream` → `StreamChunk` deltas) with the
//! project's production settings: DeepSeek `deepseek-v4-flash`, reasoning
//! enabled, and BOTH wire protocols the app can be configured with
//! (OpenAI-compatible chat completions + Responses). The 2026-08-08
//! `Duplicate 'call_id'` Responses regression is covered by a tool-enabled
//! request, which previously 400'd mid-stream.
//!
//! Run: `cargo test --lib -- --ignored real_deepseek_stream_smoke
//! --nocapture` with `DEEPSEEK_API_KEY` set.

use crate::core::config::ProviderConfig;
use crate::core::types::{ConversationItem, TokenUsage};
use crate::llm::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};
use crate::llm::client::LlmClient;
use crate::llm::provider::{LlmProvider, LlmRequest};
use crate::llm::retry::RetryConfig;
use crate::llm::streaming::StreamChunk;
use futures_util::StreamExt;
use serde_json::json;
use std::sync::Arc;

fn live_client(key: String, protocol: Option<&str>) -> LlmClient {
    let provider = ProviderConfig {
        name: "deepseek".to_string(),
        api_key_env: String::new(),
        api_key: Some(key),
        base_url: "https://api.deepseek.com/v1".to_string(),
        enabled: true,
        protocol: protocol.map(str::to_string),
    };
    LlmClient::new(
        vec![provider],
        RetryConfig {
            max_retries: 1,
            base_delay: std::time::Duration::from_millis(300),
            max_delay: std::time::Duration::from_secs(3),
            fallback_models: vec![],
        },
        true,
        Arc::new(CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 3,
            open_timeout_secs: 10,
        })),
    )
}

/// Drain one stream attempt; returns `(text, tool_starts, usage, finish)`.
async fn drain_attempt(
    client: &LlmClient,
    request: &LlmRequest,
    label: &str,
) -> (String, usize, Option<TokenUsage>, Option<String>) {
    let mut stream = client.stream(request).await.unwrap_or_else(|e| {
        panic!("[{label}] stream start failed: {e}");
    });
    let mut text = String::new();
    let mut reasoning_chunks = 0usize;
    let mut tool_starts = 0usize;
    let mut usage: Option<TokenUsage> = None;
    let mut finish: Option<String> = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(StreamChunk::TextDelta { text: t }) => text.push_str(&t),
            Ok(StreamChunk::ReasoningDelta { .. }) => reasoning_chunks += 1,
            Ok(StreamChunk::ToolCallStart { .. }) => tool_starts += 1,
            Ok(StreamChunk::Usage { usage: u }) => usage = Some(u),
            Ok(StreamChunk::Finish { reason }) => finish = Some(reason),
            Ok(StreamChunk::ToolCallDelta { .. }) | Ok(StreamChunk::ToolCallEnd { .. }) => {}
            Ok(StreamChunk::Error { message }) => panic!("[{label}] stream error: {message}"),
            Err(e) => panic!("[{label}] transport error: {e}"),
        }
    }
    eprintln!(
        "[{label}] text_chars={} reasoning_chunks={reasoning_chunks} tool_starts={tool_starts} finish={finish:?} usage={usage:?}",
        text.len()
    );
    (text, tool_starts, usage, finish)
}

async fn drain_stream(client: &LlmClient, request: LlmRequest, label: &str) {
    // DeepSeek reasoning models occasionally end a single attempt with a
    // reasoning-only `stop` (no text, no tool call). The production loop
    // recovers with an empty-response nudge, so the smoke test tolerates
    // the same transient once and re-samples — a REAL regression (e.g. the
    // `Duplicate 'call_id'` 400, dropped deltas, missing usage) still fails.
    let mut attempts = 0usize;
    loop {
        attempts += 1;
        let (text, tool_starts, usage, finish) = drain_attempt(client, &request, label).await;
        let reasoning_only_stop = text.trim().is_empty()
            && tool_starts == 0
            && finish.as_deref() == Some("stop");
        if !reasoning_only_stop || attempts >= 3 {
            assert!(
                !text.trim().is_empty() || tool_starts > 0,
                "[{label}] expected text or a tool call"
            );
            assert!(usage.is_some(), "[{label}] expected usage");
            assert!(
                usage.is_some_and(|u| u.total() > 0),
                "[{label}] expected non-zero tokens"
            );
            return;
        }
        eprintln!("[{label}] reasoning-only stop — re-sampling (attempt {attempts}/3)");
    }
}

fn request(model: &str, with_tools: bool) -> LlmRequest {
    LlmRequest {
        model: model.to_string(),
        provider: Some("deepseek".to_string()),
        messages: vec![ConversationItem::user(
            "必须调用 get_weather 工具查询北京今天的天气，然后简要回复。",
        )],
        tools: if with_tools {
            vec![crate::core::types::ToolDefinition::function(
                "get_weather",
                Some("查询指定城市的天气"),
                json!({
                    "type": "object",
                    "properties": { "city": { "type": "string" } },
                    "required": ["city"]
                }),
            )]
        } else {
            vec![]
        },
        system_prompt: "你是一个简洁的助手。".to_string(),
        temperature: Some(0.2),
        top_p: None,
        max_tokens: Some(1024),
        stream: true,
        reasoning_effort: Some("high".to_string()),
        response_format: None,
        cache_control: None,
        user_id: None,
    }
}

#[tokio::test]
#[ignore = "requires a real DEEPSEEK_API_KEY"]
async fn real_deepseek_stream_smoke() {
    let Ok(key) = std::env::var("DEEPSEEK_API_KEY") else {
        eprintln!("SKIP: DEEPSEEK_API_KEY not set");
        return;
    };

    // Pass 1 — OpenAI-compatible streaming (the default wire).
    let client = live_client(key.clone(), None);
    drain_stream(&client, request("deepseek-v4-flash", false), "openai").await;

    // Pass 2 — Responses protocol with a tool (Duplicate 'call_id'
    // regression guard).
    let responses_client = live_client(key, Some("responses"));
    drain_stream(&responses_client, request("deepseek-v4-flash", true), "responses").await;
}

#[tokio::test]
#[ignore = "requires a real DEEPSEEK_API_KEY"]
async fn real_deepseek_effort_ab_smoke() {
    let Ok(key) = std::env::var("DEEPSEEK_API_KEY") else {
        eprintln!("SKIP: DEEPSEEK_API_KEY not set");
        return;
    };
    let client = live_client(key, None);
    // A realistic MEDIUM heavy task — the exact class that burned ~27k
    // reasoning tokens at max effort in a real session (single-file CSS
    // polish). Compares cost (reasoning tokens) and output presence.
    let prompt = "这是一个单文件 CSS 优化任务：请审阅一段网页样式，给出 5 条让页面更自然美观的具体修改建议，每条一句话，不要写代码。";
    for effort in ["high", "max"] {
        let request = LlmRequest {
            model: "deepseek-v4-flash".to_string(),
            provider: Some("deepseek".to_string()),
            messages: vec![ConversationItem::user(prompt)],
            tools: vec![],
            system_prompt: "你是一个克制、注重自然美感的前端设计师。".to_string(),
            temperature: Some(0.2),
            top_p: None,
            max_tokens: Some(2000),
            stream: false,
            reasoning_effort: Some(effort.to_string()),
            response_format: None,
            cache_control: None,
            user_id: None,
        };
        let resp = client.complete(&request).await.expect("complete must succeed");
        eprintln!(
            "[effort-ab] {effort}: reasoning={} completion={} text_chars={}",
            resp.usage.reasoning_tokens.unwrap_or(0),
            resp.usage.completion_tokens,
            resp.content.trim().len()
        );
        assert!(!resp.content.trim().is_empty(), "{effort} must produce output");
    }
}
