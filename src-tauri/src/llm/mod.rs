//! LLM API layer — multi-provider HTTP client with streaming support.
//!
//! Supports OpenAI-compatible APIs (DeepSeek, OpenAI, Grok, Ollama) and
//! Anthropic's native API. Each provider implements the `LlmProvider` trait.
//!
//! Sub-modules:
//! - `client` — the main `LlmClient` wrapping HTTP + provider routing
//! - `circuit_breaker` — per-provider circuit breaker preventing cascade failures
//! - `sampler` — doom-loop detection shared with the agent loop
//! - `retry` — exponential backoff with error classification
//! - `streaming` — SSE parser for OpenAI-compatible and Anthropic formats

pub mod circuit_breaker;
pub mod client;
pub mod models;
pub mod provider;
pub mod retry;
pub mod sampler;
pub mod streaming;
pub mod vcr;

#[cfg(test)]
mod live_smoke;
