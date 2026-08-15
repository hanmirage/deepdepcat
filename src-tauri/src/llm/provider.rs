//! LLM provider abstraction — each provider (OpenAI-compatible, Anthropic)
//! implements the `LlmProvider` trait to handle API-specific request/response
//! translation.

use crate::core::error::{AppError, AppResult};
use crate::core::types::{ConversationItem, ToolDefinition};
use crate::llm::streaming::StreamChunk;
use async_trait::async_trait;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// A boxed async stream of chunks from the LLM.
pub type ChunkStream = Pin<Box<dyn Stream<Item = Result<StreamChunk, AppError>> + Send>>;

/// The response format requested from the model.
///
/// Controls whether the model returns plain text, a JSON object,
/// or a JSON object conforming to a specific schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema {
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        schema: Option<serde_json::Value>,
    },
}

/// The request sent to an LLM provider.
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub model: String,
    /// The provider name to route this request to (e.g. "deepseek", "openai").
    /// When `Some`, takes priority over prefix-based model→provider mapping.
    pub provider: Option<String>,
    pub messages: Vec<ConversationItem>,
    pub tools: Vec<ToolDefinition>,
    pub system_prompt: String,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub max_tokens: Option<u64>,
    pub stream: bool,
    pub reasoning_effort: Option<String>,
    pub response_format: Option<ResponseFormat>,
    /// Prompt cache control mode. When `Some`, overrides the default.
    /// `None` means use the client's default (`prompt_caching_enabled`).
    pub cache_control: Option<CacheControlMode>,
    /// Business-side user identifier for per-user isolation (DeepSeek).
    /// Sent only to the DeepSeek provider: OpenAI `user_id`, Anthropic
    /// `metadata.user_id`, Responses `user`. Must match `[a-zA-Z0-9\-_]+`
    /// and carry no private info (the session id satisfies both).
    pub user_id: Option<String>,
}

/// TTL mode for prompt cache breakpoints.
///
/// Controls how long the cached prompt prefix stays valid on the provider side.
/// Longer TTL reduces API latency and cost but risks serving stale context
/// when the prompt structure changes between turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CacheControlMode {
    /// Short-lived cache (~5 minutes). Safe default — re-caches each turn.
    #[default]
    Ephemeral,
}

impl CacheControlMode {
    /// The Anthropic `cache_control.type` string value.
    pub fn as_anthropic_type(self) -> &'static str {
        match self {
            Self::Ephemeral => "ephemeral",
        }
    }

    /// The Anthropic `cache_control.ttl` string value (only for 1h).
    pub fn as_anthropic_ttl(self) -> Option<&'static str> {
        match self {
            Self::Ephemeral => None,
        }
    }

    /// Serialize into a JSON value for Anthropic API body construction.
    pub fn to_anthropic_json(self) -> serde_json::Value {
        let mut v = serde_json::json!({ "type": self.as_anthropic_type() });
        if let Some(ttl) = self.as_anthropic_ttl() {
            v["ttl"] = serde_json::json!(ttl);
        }
        v
    }
}

impl Default for LlmRequest {
    fn default() -> Self {
        Self {
            model: String::new(),
            provider: None,
            messages: vec![],
            tools: vec![],
            system_prompt: String::new(),
            temperature: None,
            top_p: None,
            max_tokens: None,
            stream: true,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        }
    }
}

/// The trait that every LLM provider implements.
///
/// This abstracts away the differences between OpenAI-compatible APIs
/// (DeepSeek, OpenAI, Grok, Ollama) and Anthropic's native API format.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Send a streaming request and return a chunk stream.
    async fn stream(&self, request: &LlmRequest) -> AppResult<ChunkStream>;

    /// Send a non-streaming request and return the complete response.
    async fn complete(&self, request: &LlmRequest) -> AppResult<LlmResponse>;
}

/// A complete (non-streaming) LLM response.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub content: String,
    /// Token usage from the non-streaming response. Previously dropped on
    /// the floor — compaction/prefire/reflexion/dream/hook-eval calls were
    /// invisible in usage accounting (#88 audit H7: only the streaming path
    /// recorded usage, so a big chunk of LLM spend never showed up).
    pub usage: crate::core::types::TokenUsage,
}
