//! LLM HTTP client — the main entry point for making LLM API requests.
//!
//! Supports three wire protocols, selected per provider via the
//! `protocol` config field (auto-detected when unset):
//! - OpenAI-compatible chat completions (`/chat/completions`)
//! - Anthropic Messages API (`/v1/messages`)
//! - OpenAI Responses API (`/responses`)
//!
//! Submodules:
//! - `openai` — OpenAI chat-completions body construction and parsing
//! - `anthropic` — Anthropic body construction and parsing
//! - `responses` — OpenAI Responses API body construction and parsing
//! - `routing` — provider routing, retry logic, error classification

mod anthropic;
mod openai;
mod responses;
mod routing;

use crate::core::config::ProviderConfig;
use crate::core::error::{AppError, AppResult};
use crate::llm::circuit_breaker::CircuitBreaker;
use crate::llm::provider::{ChunkStream, LlmProvider, LlmRequest};
use crate::llm::retry::{should_attempt_fallback, RetryConfig};
use crate::llm::streaming::{StreamChunk, StreamFormat, StreamParser};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client as HttpClient;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// The wire protocol a provider speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderProtocol {
    /// OpenAI-compatible chat completions (DeepSeek, OpenAI, Grok, Ollama).
    OpenAi,
    /// Anthropic native Messages API.
    Anthropic,
    /// OpenAI Responses API (`/responses`).
    Responses,
}

/// The main LLM client — wraps an HTTP client and manages provider configs.
pub struct LlmClient {
    http: HttpClient,
    retry_config: RetryConfig,
    /// Shared provider configs — every clone (main agent, coordinator,
    /// subagents, compaction) sees the same snapshot, so a runtime config
    /// change (e.g. a newly saved API key) can be hot-applied via
    /// [`LlmClient::refresh_providers`] without rebuilding any agent.
    providers: Arc<std::sync::RwLock<Vec<ProviderConfig>>>,
    prompt_caching_enabled: bool,
    /// Per-provider circuit breaker — prevents cascade failures when an API
    /// endpoint is down. Wrapped in `Arc` so all clones share one breaker.
    circuit_breaker: Arc<CircuitBreaker>,
    /// LLM VCR — records/replays API calls when `DEEPDEPCAT_VCR` is set.
    vcr: Option<Arc<crate::llm::vcr::LlmVcr>>,
    /// Consecutive-529 tracker — feeds the model-fallback heuristic.
    overload: Arc<tokio::sync::Mutex<crate::llm::retry::OverloadTracker>>,
}

impl LlmClient {
    /// Create a new LLM client with a shared circuit breaker.
    ///
    /// All clients sharing the same `Arc<CircuitBreaker>` will see the same
    /// per-provider failure state — so a tripped circuit in one agent prevents
    /// all other agents from hitting the same dead endpoint.
    pub fn new(
        providers: Vec<ProviderConfig>,
        retry_config: RetryConfig,
        prompt_caching_enabled: bool,
        circuit_breaker: Arc<CircuitBreaker>,
    ) -> Self {
        // No blanket request timeout: a single 300s deadline on the shared
        // client silently kills long SSE streams (thinking mode + long
        // output routinely exceeds 300s) even while data keeps flowing.
        // Non-streaming calls carry their own per-request timeout
        // (NON_STREAM_TIMEOUT in routing.rs); streaming relies on the
        // stream idle watchdog instead.
        let http = HttpClient::builder()
            .build()
            .expect("Failed to build HTTP client");

        Self {
            http,
            retry_config,
            providers: Arc::new(std::sync::RwLock::new(providers)),
            prompt_caching_enabled,
            circuit_breaker,
            vcr: None,
            overload: Arc::new(tokio::sync::Mutex::new(
                crate::llm::retry::OverloadTracker::new(),
            )),
        }
    }

    /// Record a final (post-retry) failure into the circuit breaker.
    ///
    /// In HalfOpen the failure is a probe outcome: ANY failure — including
    /// 429/400 that normally never trip the circuit — means the probe failed
    /// and the circuit must go back Open, releasing the probe slot. In
    /// Closed state only breaker-tripping errors (5xx/529/transport/auth)
    /// count, preserving the original rate-limit-storm semantics.
    fn record_circuit_outcome(&self, provider: &str, err: &AppError) {
        use crate::llm::circuit_breaker::{trips_circuit_breaker, CircuitState};
        let is_probe = self.circuit_breaker.state(provider) == CircuitState::HalfOpen;
        if is_probe || trips_circuit_breaker(err) {
            self.circuit_breaker.record_failure(provider);
        }
    }

    /// Record a final (post-retry) failure into the overload tracker.
    /// Logs a warning when the consecutive-529 threshold is crossed.
    async fn record_overload(&self, err: &AppError) {
        let mut tracker = self.overload.lock().await;
        if tracker.record(err) {
            warn!("Consecutive 529 errors — consider switching to a fallback model");
        }
    }

    /// Attach an LLM VCR (record/replay test tooling).
    pub fn with_vcr(mut self, vcr: crate::llm::vcr::LlmVcr) -> Self {
        self.vcr = Some(Arc::new(vcr));
        self
    }

    /// Build a chunk stream from already-parsed chunks (VCR replay path).
    fn chunk_stream_from(chunks: Vec<StreamChunk>) -> ChunkStream {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<StreamChunk, AppError>>(128);
        tokio::spawn(async move {
            for chunk in chunks {
                if tx.send(Ok(chunk)).await.is_err() {
                    return;
                }
            }
        });
        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }

    /// Find the provider config for a given provider name.
    fn find_provider(&self, provider_name: &str) -> Option<ProviderConfig> {
        self.providers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(provider_name) && p.enabled)
            .cloned()
    }

    /// Hot-apply a new provider list (e.g. after the user saves settings).
    /// All clones of this client share the underlying store, so one refresh
    /// updates the main agent, the subagent coordinator, and compaction.
    pub fn refresh_providers(&self, providers: Vec<ProviderConfig>) {
        *self.providers.write().unwrap_or_else(|e| e.into_inner()) = providers;
    }

    /// Get the API key for a provider.
    fn api_key(&self, provider_name: &str) -> AppResult<String> {
        let provider = self.find_provider(provider_name).ok_or_else(|| {
            AppError::LlmAuth(format!("Provider '{}' not configured", provider_name))
        })?;

        if let Some(key) = provider.api_key {
            Ok(key)
        } else if !provider.api_key_env.is_empty() {
            std::env::var(&provider.api_key_env).map_err(|_| {
                AppError::LlmAuth(format!(
                    "API key env var '{}' not set for provider '{}'",
                    provider.api_key_env, provider.name
                ))
            })
        } else {
            // Ollama and other local providers may not need a key
            Ok(String::new())
        }
    }

    /// Build the request URL for a provider, based on its wire protocol.
    fn request_url(&self, provider_name: &str, _stream: bool) -> AppResult<String> {
        let provider = self.find_provider(provider_name).ok_or_else(|| {
            AppError::LlmAuth(format!("Provider '{}' not configured", provider_name))
        })?;

        let base = provider.base_url.trim_end_matches('/').to_string();
        match self.resolve_protocol(provider_name) {
            // DeepSeek's Anthropic-compatible endpoint is
            // https://api.deepseek.com/anthropic/v1/messages. Normalize every
            // base shape onto it:
            //   .../anthropic          → as-is
            //   .../v1                  → strip /v1 → .../anthropic/v1/messages
            //   https://api.deepseek.com → .../anthropic/v1/messages
            ProviderProtocol::Anthropic => {
                if provider_name.eq_ignore_ascii_case("deepseek") {
                    if base.ends_with("/anthropic") {
                        return Ok(Self::append_protocol_path(&base, "/v1/messages"));
                    }
                    let root = base.strip_suffix("/v1").unwrap_or(&base);
                    return Ok(format!("{root}/anthropic/v1/messages"));
                }
                Ok(Self::append_protocol_path(&base, "/v1/messages"))
            }
            // DeepSeek's OpenAI-compatible endpoint is
            // https://api.deepseek.com/chat/completions — the `/v1` prefix is
            // a legacy-compatible alias, NOT a requirement. Use the base as
            // configured and append the protocol path; the user's base_url is
            // the truth (a full endpoint pasted in dedupes correctly).
            ProviderProtocol::Responses => {
                Ok(Self::append_protocol_path(&base, "/responses"))
            }
            ProviderProtocol::OpenAi => {
                Ok(Self::append_protocol_path(&base, "/chat/completions"))
            }
        }
    }

    /// Append a protocol path to a base URL, deduplicating when the base
    /// already carries the exact suffix. Users often paste the FULL endpoint
    /// from a provider's docs (e.g. "https://open.bigmodel.cn/api/paas/v4/
    /// chat/completions") into the base-url field — without the dedupe the
    /// request goes to ".../chat/completions/chat/completions" → HTTP 404.
    fn append_protocol_path(base: &str, suffix: &str) -> String {
        if base.ends_with(suffix) {
            base.to_string()
        } else {
            format!("{base}{suffix}")
        }
    }

    /// Resolve the wire protocol for a provider: explicit `protocol` config
    /// wins; otherwise auto-detect — the "anthropic" provider name speaks
    /// the Anthropic Messages API, everything else OpenAI chat completions.
    fn resolve_protocol(&self, provider_name: &str) -> ProviderProtocol {
        if let Some(provider) = self.find_provider(provider_name) {
            if let Some(protocol) = provider.protocol.as_deref() {
                return match protocol {
                    "anthropic" => ProviderProtocol::Anthropic,
                    "responses" => ProviderProtocol::Responses,
                    _ => ProviderProtocol::OpenAi,
                };
            }
        }
        if provider_name.eq_ignore_ascii_case("anthropic") {
            ProviderProtocol::Anthropic
        } else {
            ProviderProtocol::OpenAi
        }
    }
}

#[async_trait]
impl LlmProvider for LlmClient {
    async fn stream(&self, request: &LlmRequest) -> AppResult<ChunkStream> {
        // VCR replay path — serve recorded chunks without any HTTP call.
        if let Some(ref vcr) = self.vcr {
            if vcr.replaying() {
                let key = vcr.fingerprint(request);
                if let Some(chunks) = vcr.replay_chunks(&key) {
                    return Ok(Self::chunk_stream_from(chunks));
                }
                warn!(key = %key, "VCR replay miss — falling through to live API");
            }
        }

        match self.stream_with_model(request).await {
            Ok(stream) => Ok(stream),
            Err(primary_err) => {
                // Model fallback for the STREAMING path (the main agent
                // loop): previously fallback_models only applied to
                // non-streaming `complete()` calls, so a DeepSeek 529 storm
                // on the main loop retried, logged, and gave up — the
                // documented "熔断后模型回退" never reached the user-facing
                // turn. Fallback attempts run only AFTER the primary's own
                // retry budget is exhausted (a stream that already started
                // delivering output is never re-issued — that would
                // duplicate billed work).
                if self.retry_config.fallback_models.is_empty()
                    || !should_attempt_fallback(&primary_err)
                {
                    return Err(primary_err);
                }
                for fallback_model in &self.retry_config.fallback_models {
                    let fallback = fallback_request(request, fallback_model);
                    match self.stream_with_model(&fallback).await {
                        Ok(stream) => {
                            info!(
                                primary = %request.model,
                                fallback = %fallback_model,
                                "Fallback model stream succeeded"
                            );
                            return Ok(stream);
                        }
                        Err(e) => {
                            warn!(
                                primary = %request.model,
                                fallback = %fallback_model,
                                error = %e,
                                "Fallback model stream failed"
                            );
                        }
                    }
                }
                Err(primary_err)
            }
        }
    }

    async fn complete(&self, request: &LlmRequest) -> AppResult<crate::llm::provider::LlmResponse> {
        info!(model = %request.model, "Sending non-streaming LLM request");

        let mut request = request.clone();
        request.stream = false;

        // VCR replay path — serve the recorded response without HTTP.
        if let Some(ref vcr) = self.vcr {
            if vcr.replaying() {
                let key = vcr.fingerprint(&request);
                if let Some(text) = vcr.replay_response(&key) {
                    return Ok(crate::llm::provider::LlmResponse {
                        content: text,
                        usage: crate::core::types::TokenUsage::default(),
                    });
                }
                warn!(key = %key, "VCR response replay miss — falling through to live API");
            }
        }

        let result = self.complete_with_model(&request).await;

        // Feed the overload tracker on final failure (529 diagnostics).
        if let Err(e) = &result {
            self.record_overload(e).await;
        }

        // VCR record path — persist successful non-streaming responses.
        if let Some(ref vcr) = self.vcr {
            if vcr.recording() {
                if let Ok(resp) = &result {
                    let key = vcr.fingerprint(&request);
                    if let Err(e) = vcr.record_response(&key, &resp.content) {
                        warn!(error = %e, "VCR record failed");
                    }
                }
            }
        }

        // If the primary model failed with a provider-side error (overload /
        // network) and fallback models are configured, try them. Client-side
        // errors (prompt-too-long, max-tokens, auth) skip fallback — they are
        // deterministic and the caller's recovery path handles them.
        if result.as_ref().is_err_and(should_attempt_fallback)
            && !self.retry_config.fallback_models.is_empty()
        {
            for fallback_model in &self.retry_config.fallback_models {
                warn!(
                    primary = %request.model,
                    fallback = %fallback_model,
                    "Primary model failed after retries — trying fallback model"
                );
                let fallback = fallback_request(&request, fallback_model);
                match self.complete_with_model(&fallback).await {
                    Ok(response) => {
                        info!(fallback = %fallback_model, "Fallback model succeeded");
                        return Ok(response);
                    }
                    Err(e) => {
                        warn!(fallback = %fallback_model, error = %e, "Fallback model failed");
                    }
                }
            }
        }

        result
    }
}

impl LlmClient {
    /// Stream against ONE resolved model/provider — no VCR replay, no model
    /// fallback. Shared by the primary attempt and each fallback attempt.
    async fn stream_with_model(&self, request: &LlmRequest) -> AppResult<ChunkStream> {
        let provider_name = self
            .find_provider_for_model(&request.model, request.provider.as_deref())
            .ok_or_else(|| AppError::ModelNotFound(request.model.clone()))?
            .name;

        let api_key = self.api_key(&provider_name)?;
        let url = self.request_url(&provider_name, true)?;
        let protocol = self.resolve_protocol(&provider_name);
        let body = match protocol {
            ProviderProtocol::Anthropic => self.build_anthropic_body(request, &provider_name),
            ProviderProtocol::Responses => self.build_responses_body(request, &provider_name),
            ProviderProtocol::OpenAi => self.build_openai_body(request, &provider_name),
        };

        // Circuit breaker check — reject immediately if the provider circuit
        // is Open. Placed AFTER all fallible preparation (api_key/url/body)
        // so that a probe slot, once granted, has no unrecorded error path
        // before the retry machinery takes over.
        self.circuit_breaker.check(&provider_name)?;

        debug!(provider = %provider_name, model = %request.model, url = %url, "Starting LLM stream");

        // The streaming path had NO retry while the non-streaming path did:
        // a transient 429/5xx/529/connection error immediately ended the
        // turn. The HTTP status check lives INSIDE the retry closure so the
        // shared machinery (exponential backoff, jitter, retryable
        // classification) applies to status errors too — mirroring
        // `complete_with_model` in routing.rs.
        let http = self.http.clone();
        let api_key_clone = api_key.clone();
        let url_clone = url.clone();
        let body_clone = body.clone();
        let anthropic_auth = protocol == ProviderProtocol::Anthropic;
        let send_request = move || {
            let http = http.clone();
            let api_key = api_key_clone.clone();
            let url = url_clone.clone();
            let body = body_clone.clone();
            async move {
                let mut req_builder = http.post(&url).json(&body);
                if anthropic_auth {
                    req_builder = req_builder
                        .header("x-api-key", &api_key)
                        .header("anthropic-version", "2023-06-01")
                        .header("anthropic-dangerous-direct-browser-access", "true");
                } else if !api_key.is_empty() {
                    req_builder =
                        req_builder.header("Authorization", format!("Bearer {}", api_key));
                }

                let response = req_builder
                    .header("Content-Type", "application/json")
                    .send()
                    .await
                    .map_err(crate::core::error::AppError::Http)?;

                let status = response.status();
                if !status.is_success() {
                    let retry_after_header = routing::parse_retry_after_header(response.headers());
                    let body_text = response.text().await.unwrap_or_default();
                    return Err(routing::classify_http_error(
                        status.as_u16(),
                        &body_text,
                        retry_after_header,
                    ));
                }
                Ok(response)
            }
        };

        let response = match crate::llm::retry::with_retry(&self.retry_config, send_request).await {
            Ok(resp) => {
                self.circuit_breaker.record_success(&provider_name);
                resp
            }
            Err(e) => {
                // Final (post-retry) failure — feed the overload tracker so
                // repeated 529s are visible and can drive model fallback.
                self.record_overload(&e).await;
                // Count toward the circuit breaker exactly once (not once per
                // retry attempt); in HalfOpen the failure releases the probe
                // slot regardless of error class.
                self.record_circuit_outcome(&provider_name, &e);
                return Err(e);
            }
        };

        let format = match protocol {
            ProviderProtocol::Anthropic => StreamFormat::Anthropic,
            ProviderProtocol::Responses => StreamFormat::Responses,
            ProviderProtocol::OpenAi => StreamFormat::OpenAi,
        };

        // VCR record path — buffer the full response, parse it, persist the
        // chunks, then serve them. Recording runs are test/CI runs, so the
        // buffered (non-live) streaming is acceptable.
        if let Some(ref vcr) = self.vcr {
            if vcr.recording() {
                let key = vcr.fingerprint(request);
                let mut full = String::new();
                let mut byte_stream = response.bytes_stream();
                while let Some(result) = byte_stream.next().await {
                    match result {
                        Ok(bytes) => full.push_str(&String::from_utf8_lossy(&bytes)),
                        Err(e) => return Err(AppError::LlmStreaming(e.to_string())),
                    }
                }
                let mut parser = StreamParser::new(format);
                let mut chunks: Vec<StreamChunk> = Vec::new();
                for chunk in parser.feed(&full) {
                    chunks.push(chunk);
                }
                chunks.push(StreamChunk::Finish {
                    reason: "stop".to_string(),
                });
                if let Err(e) = vcr.record_chunks(&key, &chunks) {
                    warn!(error = %e, "VCR record failed");
                }
                return Ok(Self::chunk_stream_from(chunks));
            }
        }

        // Channel-based streaming: spawn a task that feeds bytes into the parser
        // and sends parsed chunks through the channel. The parser state is maintained
        // across all byte chunks — a single StreamParser instance lives for the entire stream.
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<StreamChunk, AppError>>(128);

        // Stream-stall watchdog (ronx): proxies (v2rayN/sing-box) and flaky
        // networks drop SSE connections silently — the stream simply stops
        // delivering bytes with no error. Wrap each `next()` in a fresh
        // timeout so every received chunk resets the clock; 120s of silence
        // aborts the stream with a clear, retryable error instead of hanging
        // the agent loop forever.
        let stream_idle_timeout = std::time::Duration::from_secs(
            std::env::var("DDC_STREAM_IDLE_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(120),
        );

        tokio::spawn(async move {
            let mut parser = StreamParser::new(format);
            let mut byte_stream = response.bytes_stream();
            // Whether a real Finish event was already parsed from the wire.
            // Providers emit their true finish_reason ("stop", "length",
            // "insufficient_system_resource", ...) inside the SSE stream;
            // the synthetic trailing Finish below must NOT overwrite it —
            // the truncation-recovery and resource-backoff paths in the
            // agent loop key off the real reason (#88 audit H4).
            let mut finish_seen = false;
            // Held-back incomplete UTF-8 bytes from the previous HTTP chunk.
            // Decoding each reqwest byte chunk independently with
            // from_utf8_lossy would corrupt a CJK/emoji character whose bytes
            // straddle a chunk boundary (→ U+FFFD); buffering the incomplete
            // trailing sequence and prepending it to the next chunk keeps the
            // stream lossless.
            let mut pending: Vec<u8> = Vec::new();

            loop {
                let next = tokio::time::timeout(stream_idle_timeout, byte_stream.next()).await;
                match next {
                    Ok(Some(Ok(bytes))) => {
                        let mut buf = std::mem::take(&mut pending);
                        buf.extend_from_slice(&bytes);
                        let cut = buf.len() - incomplete_utf8_suffix_len(&buf);
                        let text = String::from_utf8_lossy(&buf[..cut]).into_owned();
                        pending.extend_from_slice(&buf[cut..]);
                        let chunks = parser.feed(&text);
                        for chunk in chunks {
                            if matches!(chunk, StreamChunk::Finish { .. }) {
                                finish_seen = true;
                            }
                            if tx.send(Ok(chunk)).await.is_err() {
                                return; // Receiver dropped — cancel streaming
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        let _ = tx.send(Err(AppError::LlmStreaming(e.to_string()))).await;
                        return;
                    }
                    Ok(None) => break, // Stream ended cleanly.
                    Err(_) => {
                        // 120s without a single byte — connection stalled
                        // (proxy idle-drop / network blackout). Surface as a
                        // streaming error so the agent loop can retry.
                        let _ = tx
                            .send(Err(AppError::LlmStreaming(
                                "stream stalled — no data for 120s, connection likely dropped"
                                    .to_string(),
                            )))
                            .await;
                        return;
                    }
                }
            }

            // Flush any final incomplete sequence (the stream ended mid-way
            // through a multi-byte character) — surfaced lossily rather than
            // silently dropped.
            if !pending.is_empty() {
                let text = String::from_utf8_lossy(&pending).into_owned();
                let chunks = parser.feed(&text);
                for chunk in chunks {
                    if matches!(chunk, StreamChunk::Finish { .. }) {
                        finish_seen = true;
                    }
                    if tx.send(Ok(chunk)).await.is_err() {
                        return;
                    }
                }
            }

            // Synthetic close ONLY when the provider never sent one — a
            // stream that ended without a finish reason still needs a
            // well-formed stop marker for the parser consumers.
            if !finish_seen {
                let _ = tx
                    .send(Ok(StreamChunk::Finish {
                        reason: "stop".to_string(),
                    }))
                    .await;
            }
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Box::pin(stream))
    }
}

/// Length of an incomplete UTF-8 sequence at the end of `bytes`, or 0.
///
/// Mirrors `agent/streaming.rs::incomplete_utf8_suffix_len` (the LLM client
/// must not depend on the agent module). Returns the trailing byte count when
/// `bytes` ends mid-way through a multi-byte sequence; 0 when it ends on a
/// complete boundary or contains a hard encoding error.
fn incomplete_utf8_suffix_len(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => 0,
        Err(e) if e.error_len().is_none() => bytes.len() - e.valid_up_to(),
        Err(_) => 0,
    }
}

/// Build the request for a model-fallback attempt.
///
/// The primary's provider hint is CLEARED: the hint (e.g. "deepseek") must
/// not pin a fallback model from another provider (grok / claude / a custom
/// endpoint) to the primary's endpoint — the fallback's provider is
/// re-resolved from the model's own prefix instead. Without this, a
/// deepseek-primary session with a grok fallback sent the grok model to the
/// DeepSeek URL and got HTTP 400.
fn fallback_request(request: &LlmRequest, fallback_model: &str) -> LlmRequest {
    let mut fallback = request.clone();
    fallback.model = fallback_model.to_string();
    fallback.provider = None;
    fallback
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_protocol_path_dedupes_full_endpoint() {
        // A base URL that already carries the full endpoint must not get the
        // suffix appended twice (".../chat/completions/chat/completions" → 404).
        assert_eq!(
            LlmClient::append_protocol_path(
                "https://open.bigmodel.cn/api/paas/v4/chat/completions",
                "/chat/completions",
            ),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        assert_eq!(
            LlmClient::append_protocol_path("https://api.deepseek.com/v1", "/chat/completions"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            LlmClient::append_protocol_path("https://api.anthropic.com", "/v1/messages"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            LlmClient::append_protocol_path(
                "https://api.anthropic.com/v1/messages",
                "/v1/messages"
            ),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn deepseek_chat_completions_has_no_v1_prefix() {
        // DeepSeek's OpenAI-compatible endpoint is
        // https://api.deepseek.com/chat/completions — `/v1` is a compatible
        // alias, not a requirement. A bare-host base must resolve WITHOUT an
        // injected /v1, and a pasted full endpoint must dedupe, not corrupt.
        fn url(base: &str, protocol: Option<&str>) -> String {
            let client = LlmClient::new(
                vec![crate::core::config::ProviderConfig {
                    name: "deepseek".into(),
                    api_key_env: "DEEPSEEK_API_KEY".into(),
                    api_key: None,
                    base_url: base.into(),
                    enabled: true,
                    protocol: protocol.map(str::to_string),
                }],
                crate::llm::retry::RetryConfig::default(),
                true,
                Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
                    crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
                )),
            );
            client.request_url("deepseek", true).unwrap()
        }
        assert_eq!(
            url("https://api.deepseek.com", None),
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(
            url("https://api.deepseek.com/v1", None),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            url("https://api.deepseek.com/v1/chat/completions", None),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            url("https://api.deepseek.com", Some("responses")),
            "https://api.deepseek.com/responses"
        );
    }

    #[test]
    fn find_provider_matches_case_insensitively() {
        let client = LlmClient::new(
            vec![ProviderConfig {
                name: "DeepSeek".to_string(),
                api_key_env: "DEEPSEEK_API_KEY".to_string(),
                api_key: None,
                base_url: "https://api.deepseek.com".to_string(),
                enabled: true,
                protocol: None,
            }],
            RetryConfig::default(),
            true,
            Arc::new(CircuitBreaker::new(Default::default())),
        );
        assert!(client.find_provider("deepseek").is_some());
        assert!(client.find_provider("DeepSeek").is_some());
        assert!(client.find_provider("openai").is_none());
    }

    #[test]
    fn fallback_request_swaps_model_and_clears_provider_hint() {
        // A deepseek-primary session with a grok fallback must NOT keep the
        // deepseek provider hint — the fallback model would be pinned to the
        // DeepSeek URL (HTTP 400). Clearing the hint lets the fallback's own
        // model prefix re-resolve its provider.
        let request = LlmRequest {
            model: "deepseek-v4-pro".into(),
            provider: Some("deepseek".into()),
            messages: vec![crate::core::types::ConversationItem::user("hi")],
            system_prompt: "sys".into(),
            reasoning_effort: Some("max".into()),
            ..Default::default()
        };

        let fb = fallback_request(&request, "grok-3");
        assert_eq!(fb.model, "grok-3");
        assert_eq!(
            fb.provider, None,
            "fallback must re-resolve provider by model prefix"
        );
        // Everything else is preserved for the fallback attempt.
        assert_eq!(fb.messages.len(), 1);
        assert_eq!(fb.system_prompt, "sys");
        assert_eq!(fb.reasoning_effort.as_deref(), Some("max"));
        // The primary request stays untouched.
        assert_eq!(request.provider.as_deref(), Some("deepseek"));
        assert_eq!(request.model, "deepseek-v4-pro");
    }
}
