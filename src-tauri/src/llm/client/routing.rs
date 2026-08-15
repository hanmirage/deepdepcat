//! Provider routing, retry logic, and error classification.

use crate::core::config::ProviderConfig;
use crate::core::error::{AppError, AppResult};
use crate::llm::provider::{LlmRequest, LlmResponse};
use crate::llm::retry::with_retry;
use serde_json::Value;
use tracing::warn;

use super::LlmClient;

impl Clone for LlmClient {
    fn clone(&self) -> Self {
        Self {
            http: self.http.clone(),
            retry_config: self.retry_config.clone(),
            providers: self.providers.clone(),
            prompt_caching_enabled: self.prompt_caching_enabled,
            circuit_breaker: self.circuit_breaker.clone(),
            vcr: self.vcr.clone(),
            overload: self.overload.clone(),
        }
    }
}

impl LlmClient {
    /// Find which provider a model belongs to.
    ///
    /// Resolution order:
    /// 1. Explicit `provider_hint` (from session/LlmRequest) — exact name match
    /// 2. Known model ID prefix mapping (deepseek→deepseek, gpt-→openai, etc.)
    /// 3. Fallback: first enabled provider
    pub(super) fn find_provider_for_model(
        &self,
        model_id: &str,
        provider_hint: Option<&str>,
    ) -> Option<ProviderConfig> {
        // 1. Explicit hint from the caller — highest priority
        if let Some(hint) = provider_hint {
            if let Some(p) = self.find_provider(hint) {
                return Some(p);
            }
        }

        // 2. Prefix-based mapping
        let provider_name = if model_id.starts_with("deepseek") {
            "deepseek"
        } else if model_id.starts_with("gpt-")
            || model_id.starts_with("o1")
            || model_id.starts_with("o3")
        {
            "openai"
        } else if model_id.starts_with("claude") {
            "anthropic"
        } else if model_id.starts_with("grok") {
            "grok"
        } else {
            return self
                .providers
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .find(|p| p.enabled)
                .cloned();
        };

        match self.find_provider(provider_name) {
            Some(p) => Some(p),
            None => {
                warn!(
                    model = %model_id,
                    expected_provider = %provider_name,
                    "Provider not found for model — falling back to first enabled provider"
                );
                self.providers
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .find(|p| p.enabled)
                    .cloned()
            }
        }
    }

    /// Send a non-streaming request with retry logic for a specific model.
    ///
    /// Resolves the provider, builds the request body, and delegates to
    /// [`with_retry`] for exponential backoff on transient errors.
    pub(super) async fn complete_with_model(&self, request: &LlmRequest) -> AppResult<LlmResponse> {
        let provider_name = self
            .find_provider_for_model(&request.model, request.provider.as_deref())
            .ok_or_else(|| AppError::ModelNotFound(request.model.clone()))?
            .name;

        let api_key = self.api_key(&provider_name)?;
        let url = self.request_url(&provider_name, false)?;
        let protocol = self.resolve_protocol(&provider_name);

        let body = match protocol {
            super::ProviderProtocol::Anthropic => {
                self.build_anthropic_body(request, &provider_name)
            }
            super::ProviderProtocol::Responses => {
                self.build_responses_body(request, &provider_name)
            }
            super::ProviderProtocol::OpenAi => self.build_openai_body(request, &provider_name),
        };

        // Circuit breaker check — reject immediately if the provider circuit
        // is Open. Placed AFTER all fallible preparation (api_key/url/body)
        // so that a probe slot, once granted, has no unrecorded error path
        // before the retry machinery takes over.
        self.circuit_breaker.check(&provider_name)?;

        let cb = self.circuit_breaker.clone();
        let anthropic_auth = protocol == super::ProviderProtocol::Anthropic;

        let result = with_retry(&self.retry_config, move || {
            let http = self.http.clone();
            let api_key = api_key.clone();
            let url = url.clone();
            let body = body.clone();
            let model = request.model.clone();

            async move {
                let mut req_builder = http.post(&url).json(&body);

                if anthropic_auth {
                    req_builder = req_builder
                        .header("x-api-key", &api_key)
                        .header("anthropic-version", "2023-06-01");
                } else if !api_key.is_empty() {
                    req_builder =
                        req_builder.header("Authorization", format!("Bearer {}", api_key));
                }

                // The shared client carries no blanket timeout (a single
                // deadline kills long SSE streams) — non-streaming calls get
                // their own hard deadline here instead.
                let response = req_builder
                    .header("Content-Type", "application/json")
                    .timeout(NON_STREAM_TIMEOUT)
                    .send()
                    .await?;

                let status = response.status();
                if !status.is_success() {
                    let retry_after_header = parse_retry_after_header(response.headers());
                    let body_text = response.text().await.unwrap_or_default();
                    return Err(classify_http_error(
                        status.as_u16(),
                        &body_text,
                        retry_after_header,
                    ));
                }

                let json: Value = response.json().await?;

                match protocol {
                    super::ProviderProtocol::Anthropic => {
                        super::anthropic::parse_anthropic_response(&json, &model)
                    }
                    super::ProviderProtocol::Responses => {
                        super::responses::parse_responses_response(&json, &model)
                    }
                    super::ProviderProtocol::OpenAi => {
                        super::openai::parse_openai_response(&json, &model)
                    }
                }
            }
        })
        .await;

        // Record the final outcome into the circuit breaker exactly once
        // (not once per retry attempt). In HalfOpen the failure releases the
        // probe slot regardless of error class; otherwise 429/400 never trip
        // it while 529 still counts.
        match &result {
            Ok(_) => cb.record_success(&provider_name),
            Err(e) => self.record_circuit_outcome(&provider_name, e),
        }
        result
    }
}

/// Hard deadline for non-streaming LLM calls (seconds).
///
/// Replaces the former blanket 300s client timeout: the shared client must
/// stay timeout-free so long SSE streams are not cut mid-generation, but a
/// non-streaming call (compaction, reflexion, vision transcription) still
/// needs a worst-case bound to avoid hanging the caller forever.
const NON_STREAM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Classify an HTTP error response into an AppError.
///
/// Detects prompt-too-long and max-tokens errors from the response body,
/// enabling the agent loop to trigger compaction or escalate output tokens.
pub(super) fn classify_http_error(
    status: u16,
    body: &str,
    retry_after_header: Option<u64>,
) -> AppError {
    let body_lower = body.to_lowercase();

    // Detect prompt-too-long errors across providers.
    // Anthropic returns 400 with "prompt is too long" or context_window_exceeded.
    // OpenAI returns 400 with "context_length_exceeded" or "maximum context length".
    if (status == 400 || status == 413)
        && (body_lower.contains("prompt is too long")
            || body_lower.contains("context_length_exceeded")
            || body_lower.contains("context window")
            || body_lower.contains("maximum context length")
            || body_lower.contains("too long")
            || body_lower.contains("context_window_exceeded"))
    {
        let max = extract_max_tokens(body);
        return AppError::PromptTooLong { max_tokens: max };
    }

    // Detect max-tokens / output-length exceeded.
    // Some providers return 400 with "max_tokens" or "output length" messages.
    if status == 400 && body_lower.contains("max_tokens") && body_lower.contains("exceed") {
        let requested = extract_requested_tokens(body).unwrap_or(0);
        let max = extract_max_tokens(body).unwrap_or(8192);
        return AppError::MaxTokensExceeded { requested, max };
    }

    match status {
        429 => {
            // Prefer the Retry-After response header (authoritative); fall
            // back to the response body when the header is absent.
            let retry_after = retry_after_header.or_else(|| parse_retry_after(body));
            AppError::LlmRateLimited {
                retry_after_secs: retry_after,
            }
        }
        401 | 403 => AppError::LlmAuth(format!("HTTP {}: {}", status, body)),
        400 | 404 => AppError::LlmApi {
            source: format!("HTTP {}: {}", status, body).into(),
            status_code: Some(status),
        },
        500 | 502 | 503 | 529 => AppError::LlmApi {
            source: format!("HTTP {}: {}", status, body).into(),
            status_code: Some(status),
        },
        _ => AppError::LlmApi {
            source: format!("HTTP {}: {}", status, body).into(),
            status_code: Some(status),
        },
    }
}

/// Try to extract the maximum token count from an API error response.
fn extract_max_tokens(body: &str) -> Option<u64> {
    // Try JSON parsing first.
    if let Ok(json) = serde_json::from_str::<Value>(body) {
        // Anthropic: error.message contains "max_tokens: N"
        if let Some(msg) = json
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
        {
            if let Some(n) = extract_number_after(msg, "max_tokens") {
                return Some(n);
            }
            if let Some(n) = extract_number_after(msg, "context") {
                return Some(n);
            }
        }
    }

    // Fallback: regex on raw body.
    extract_number_after(body, "max_tokens")
        .or_else(|| extract_number_after(body, "context_length"))
}

/// Try to extract the requested token count from an API error response.
fn extract_requested_tokens(body: &str) -> Option<u64> {
    extract_number_after(body, "requested")
        .or_else(|| extract_number_after(body, "requested_tokens"))
}

/// Extract the first number that appears after a keyword in text.
fn extract_number_after(text: &str, keyword: &str) -> Option<u64> {
    let lower = text.to_lowercase();
    let kw = keyword.to_lowercase();
    if let Some(pos) = lower.find(&kw) {
        let after = &text[pos + kw.len()..];
        let num_str: String = after
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if !num_str.is_empty() {
            return num_str.parse().ok();
        }
    }
    None
}

/// Try to parse the retry-after value from a rate limit response.
pub(super) fn parse_retry_after(body: &str) -> Option<u64> {
    if let Ok(json) = serde_json::from_str::<Value>(body) {
        if let Some(retry) = json
            .get("error")
            .and_then(|e| e.get("retry_after"))
            .and_then(|r| r.as_u64())
        {
            return Some(retry);
        }
    }
    None
}

/// Parse the `Retry-After` response header into a delay in seconds.
///
/// The header carries either a bare integer (seconds, the common form) or
/// an HTTP-date (`Wed, 21 Oct 2015 07:28:00 GMT`). Returns `None` when
/// absent or unparseable — callers fall back to the response body.
pub(super) fn parse_retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(secs);
    }
    chrono::DateTime::parse_from_rfc2822(value).ok().map(|dt| {
        dt.signed_duration_since(chrono::Utc::now())
            .num_seconds()
            .max(0) as u64
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_header_seconds_parsed() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "7".parse().unwrap());
        assert_eq!(parse_retry_after_header(&headers), Some(7));
    }

    #[test]
    fn retry_after_header_date_parsed() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            "Wed, 21 Oct 2037 07:28:00 GMT".parse().unwrap(),
        );
        let delay = parse_retry_after_header(&headers);
        assert!(delay.is_some() && delay.unwrap() > 0);
    }

    #[test]
    fn retry_after_missing_returns_none() {
        assert_eq!(
            parse_retry_after_header(&reqwest::header::HeaderMap::new()),
            None
        );
    }

    #[test]
    fn classify_prefers_header_over_body() {
        let err = classify_http_error(429, r#"{"error":{"retry_after":60}}"#, Some(12));
        match err {
            AppError::LlmRateLimited { retry_after_secs } => {
                assert_eq!(retry_after_secs, Some(12));
            }
            other => panic!("expected rate limited, got {other:?}"),
        }
    }

    #[test]
    fn classify_falls_back_to_body_when_header_missing() {
        let err = classify_http_error(429, r#"{"error":{"retry_after":60}}"#, None);
        match err {
            AppError::LlmRateLimited { retry_after_secs } => {
                assert_eq!(retry_after_secs, Some(60));
            }
            other => panic!("expected rate limited, got {other:?}"),
        }
    }
}
