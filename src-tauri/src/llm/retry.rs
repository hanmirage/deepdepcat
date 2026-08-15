//! Retry logic with exponential backoff and error classification.
//!
//! Error classification: 429 (rate limit), 500/502/503 (server), 529 (overloaded),
//! timeout, prompt-too-long, max-tokens-exceeded.
//!
//! Exponential backoff: base 500ms, max 32s, jitter ±25%.
//! Model fallback after consecutive 529/503 errors (default: 3 consecutive).
//! Max-tokens escalation: 8k → 16k → 32k → 64k.

use crate::core::error::{AppError, AppResult};
use rand::Rng;
use std::time::Duration;
use tracing::warn;

/// The escalation ladder for output token limits when `MaxTokensExceeded` fires.
/// The agent loop tries each value in order until the API accepts the request.
pub const MAX_TOKENS_LADDER: &[u64] = &[8_192, 16_384, 32_768, 65_536];

/// Number of consecutive 529 errors before triggering model fallback.
const CONSECUTIVE_529_THRESHOLD: u32 = 3;

/// Retry configuration.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    /// Models to fall back to (in order) if the primary model fails repeatedly.
    pub fallback_models: Vec<String>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(32),
            fallback_models: vec![],
        }
    }
}

impl RetryConfig {
    pub fn from_llm_config(config: &crate::core::config::LlmSection) -> Self {
        Self {
            max_retries: config.max_retries,
            base_delay: Duration::from_millis(config.retry_base_delay_ms),
            max_delay: Duration::from_millis(config.retry_max_delay_ms),
            fallback_models: config.fallback_model.iter().cloned().collect(),
        }
    }
}

/// Tracks consecutive 529 (overloaded) errors across calls within a session.
/// When the counter reaches the threshold, the caller should switch to a fallback model.
#[derive(Debug, Clone, Default)]
pub struct OverloadTracker {
    consecutive_529: u32,
    /// Whether a fallback has already been triggered.
    fallback_triggered: bool,
}

impl OverloadTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a result and return whether model fallback should be triggered.
    pub fn record(&mut self, err: &AppError) -> bool {
        if err.is_overloaded() {
            self.consecutive_529 += 1;
            if self.consecutive_529 >= CONSECUTIVE_529_THRESHOLD && !self.fallback_triggered {
                self.fallback_triggered = true;
                warn!(
                    consecutive = self.consecutive_529,
                    threshold = CONSECUTIVE_529_THRESHOLD,
                    "Consecutive 529 errors — triggering model fallback"
                );
                return true;
            }
        } else if self.consecutive_529 > 0 {
            self.consecutive_529 = 0;
        }
        false
    }
}

/// Classify an error and determine the retry delay.
pub fn classify_error(err: &AppError) -> ErrorClass {
    match err {
        AppError::LlmRateLimited { retry_after_secs } => ErrorClass::RateLimited {
            retry_after: retry_after_secs.map(Duration::from_secs),
        },
        AppError::LlmApi { status_code, .. } => match status_code {
            Some(429) => ErrorClass::RateLimited { retry_after: None },
            Some(500) | Some(502) | Some(503) | Some(504) | Some(529) => ErrorClass::ServerError,
            Some(401) | Some(403) => ErrorClass::Auth,
            // Deterministic client-side failures — retrying re-sends the
            // (potentially multi-MB) request on a problem the bytes won't
            // fix, and a 413 additionally trips the circuit breaker.
            Some(400) | Some(404) | Some(408) | Some(409) | Some(413) | Some(415) | Some(422)
            | Some(426) | Some(505) => ErrorClass::ClientError,
            // Unknown or status-less (e.g. a 200 with an empty choices
            // array) — deterministic, never a transient server blip; do not
            // burn retries on it.
            _ => ErrorClass::ClientError,
        },
        AppError::Http(e) => {
            if e.is_timeout() {
                ErrorClass::Timeout
            } else if e.is_connect() {
                ErrorClass::Connection
            } else {
                ErrorClass::ServerError
            }
        }
        AppError::Timeout(_) => ErrorClass::Timeout,
        AppError::PromptTooLong { .. } => ErrorClass::PromptTooLong,
        AppError::MaxTokensExceeded { .. } => ErrorClass::MaxTokensExceeded,
        _ => ErrorClass::NonRetryable,
    }
}

/// Whether a failed primary request should attempt a fallback model. Only
/// provider-side failures another model could actually absorb warrant
/// fallback — overload (429/5xx/529) and network timeouts/connections.
/// Client-side errors (PromptTooLong → compaction, MaxTokensExceeded →
/// max-tokens escalation) and auth/client errors are deterministic and would
/// fail identically on every model, so falling back only wastes tokens and
/// delays the real recovery path.
pub fn should_attempt_fallback(err: &AppError) -> bool {
    matches!(
        classify_error(err),
        ErrorClass::ServerError
            | ErrorClass::RateLimited { .. }
            | ErrorClass::Timeout
            | ErrorClass::Connection
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorClass {
    /// 429 — should retry with delay.
    RateLimited { retry_after: Option<Duration> },
    /// 500/502/503/529 — should retry.
    ServerError,
    /// Request timed out — should retry.
    Timeout,
    /// Connection failed — should retry.
    Connection,
    /// 401/403 — should not retry.
    Auth,
    /// 400/404 — should not retry.
    ClientError,
    /// Prompt exceeds context window — trigger compaction, not a retry.
    PromptTooLong,
    /// Output token limit exceeded — escalate max_tokens, not a retry.
    MaxTokensExceeded,
    /// Any other error — should not retry.
    NonRetryable,
}

impl ErrorClass {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. } | Self::ServerError | Self::Timeout | Self::Connection
        )
    }

    /// Whether this error requires a recovery action (compaction or token escalation)
    /// rather than a simple retry.
    pub fn requires_recovery(&self) -> bool {
        matches!(self, Self::PromptTooLong | Self::MaxTokensExceeded)
    }
}

/// Calculate the backoff delay for a given attempt (0-indexed).
pub fn backoff_delay(attempt: u32, config: &RetryConfig) -> Duration {
    let base = config.base_delay.as_millis() as u64;
    let exp = base
        .checked_shl(attempt)
        .unwrap_or(config.max_delay.as_millis() as u64);
    let capped = exp.min(config.max_delay.as_millis() as u64);
    // Add random jitter within ±25% of the capped delay to avoid thundering herd
    let jitter_range = capped / 4;
    let jitter = if jitter_range > 0 {
        rand::thread_rng().gen_range(0..jitter_range)
    } else {
        0
    };
    let jittered = capped.saturating_sub(jitter_range / 2) + jitter;
    Duration::from_millis(jittered)
}

/// Execute an async operation with retry logic.
pub async fn with_retry<F, Fut, T>(config: &RetryConfig, mut operation: F) -> AppResult<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = AppResult<T>>,
{
    let mut last_error: Option<AppError> = None;

    for attempt in 0..=config.max_retries {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(err) => {
                let class = classify_error(&err);
                warn!(
                    attempt = attempt + 1,
                    max_retries = config.max_retries,
                    error = %err,
                    class = ?class,
                    "LLM request failed"
                );

                // Recovery-class errors are returned immediately so the caller
                // can take corrective action (compaction, token escalation).
                if class.requires_recovery() {
                    return Err(err);
                }

                if !class.is_retryable() || attempt == config.max_retries {
                    return Err(err);
                }

                let delay = match class {
                    ErrorClass::RateLimited {
                        retry_after: Some(d),
                    } => d.min(config.max_delay),
                    _ => backoff_delay(attempt, config),
                };

                tokio::time::sleep(delay).await;
                last_error = Some(err);
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| AppError::Internal("Retry exhausted without error".to_string())))
}

/// Pick the next max-tokens value from the escalation ladder.
///
/// Returns `Some(next)` if escalation is possible, or `None` if already at the
/// highest tier and cannot escalate further.
pub fn escalate_max_tokens(current: u64) -> Option<u64> {
    MAX_TOKENS_LADDER
        .iter()
        .find(|&&step| step > current)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overload_tracker_triggers_fallback() {
        let mut tracker = OverloadTracker::new();
        let err = AppError::LlmApi {
            source: "overloaded".into(),
            status_code: Some(529),
        };

        assert!(!tracker.record(&err));
        assert!(!tracker.record(&err));
        assert!(tracker.record(&err)); // 3rd consecutive → trigger
    }

    #[test]
    fn overload_tracker_resets_on_success() {
        let mut tracker = OverloadTracker::new();
        let err_529 = AppError::LlmApi {
            source: "overloaded".into(),
            status_code: Some(529),
        };
        let err_500 = AppError::LlmApi {
            source: "server error".into(),
            status_code: Some(500),
        };

        tracker.record(&err_529);
        tracker.record(&err_529);
        assert_eq!(tracker.consecutive_529, 2);

        // Non-529 resets the counter.
        tracker.record(&err_500);
        assert_eq!(tracker.consecutive_529, 0);
    }

    #[test]
    fn classify_prompt_too_long() {
        let err = AppError::PromptTooLong { max_tokens: None };
        let class = classify_error(&err);
        assert_eq!(class, ErrorClass::PromptTooLong);
        assert!(class.requires_recovery());
        assert!(!class.is_retryable());
    }

    #[test]
    fn classify_max_tokens_exceeded() {
        let err = AppError::MaxTokensExceeded {
            requested: 8192,
            max: 4096,
        };
        let class = classify_error(&err);
        assert_eq!(class, ErrorClass::MaxTokensExceeded);
        assert!(class.requires_recovery());
    }

    #[test]
    fn fallback_only_on_provider_side_errors() {
        // Overload / server / network errors → fallback makes sense.
        assert!(should_attempt_fallback(&AppError::LlmApi {
            source: "overloaded".into(),
            status_code: Some(529),
        }));
        assert!(should_attempt_fallback(&AppError::LlmApi {
            source: "server error".into(),
            status_code: Some(500),
        }));
        assert!(should_attempt_fallback(&AppError::Timeout(30)));

        // Client-side errors are deterministic on every model → no fallback.
        assert!(!should_attempt_fallback(&AppError::PromptTooLong { max_tokens: None }));
        assert!(!should_attempt_fallback(&AppError::MaxTokensExceeded {
            requested: 8192,
            max: 4096,
        }));
        assert!(!should_attempt_fallback(&AppError::LlmApi {
            source: "unauthorized".into(),
            status_code: Some(401),
        }));
    }

    #[test]
    fn unknown_and_client_statuses_are_not_retried() {
        // 413 (payload too large) and a status-less parse error (200 with an
        // empty choices array) are DETERMINISTIC — retrying re-sends the
        // full multi-MB request on a problem the bytes won't fix, and 413
        // also trips the circuit breaker. Unknown statuses default to
        // non-retryable rather than assuming a transient server blip.
        for status in [Some(408), Some(409), Some(413), Some(415), Some(422), Some(426), Some(505)] {
            let class = classify_error(&AppError::LlmApi {
                source: "deterministic".into(),
                status_code: status,
            });
            assert_eq!(class, ErrorClass::ClientError, "status {status:?}");
            assert!(!class.is_retryable());
            assert!(!should_attempt_fallback(&AppError::LlmApi {
                source: "deterministic".into(),
                status_code: status,
            }));
        }
        let no_status = classify_error(&AppError::LlmApi {
            source: "empty choices".into(),
            status_code: None,
        });
        assert_eq!(no_status, ErrorClass::ClientError);
        assert!(!no_status.is_retryable());

        // 504 (gateway timeout) is genuinely transient — still retried.
        assert!(classify_error(&AppError::LlmApi {
            source: "gateway timeout".into(),
            status_code: Some(504),
        })
        .is_retryable());
    }

    #[test]
    fn escalate_max_tokens_ladder() {
        assert_eq!(escalate_max_tokens(4096), Some(8192));
        assert_eq!(escalate_max_tokens(8192), Some(16384));
        assert_eq!(escalate_max_tokens(16384), Some(32768));
        assert_eq!(escalate_max_tokens(32768), Some(65536));
        assert_eq!(escalate_max_tokens(65536), None);
    }

    #[tokio::test]
    async fn retry_after_is_capped_at_max_delay() {
        // A provider answering Retry-After: 3600 must not wedge the caller
        // for an hour — every retry sleep is capped at max_delay.
        let config = RetryConfig {
            max_retries: 2,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(50),
            fallback_models: vec![],
        };
        let started = std::time::Instant::now();
        let result: AppResult<()> = with_retry(&config, || async {
            Err(AppError::LlmRateLimited {
                retry_after_secs: Some(3600),
            })
        })
        .await;
        assert!(result.is_err());
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "retry-after sleep was not capped: {elapsed:?}"
        );
    }
}
