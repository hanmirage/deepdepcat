//! Compaction sampler — the LLM call seam that produces summaries,
//! plus error classification for retry decisions.

use std::time::Duration;

use async_trait::async_trait;

use crate::core::error::{AppError, AppResult};
use crate::core::types::ConversationItem;
use crate::llm::client::LlmClient;
use crate::llm::provider::{LlmProvider, LlmRequest};

/// Raw text captured from a compaction LLM call, plus the billed usage.
#[derive(Debug, Default, Clone)]
pub struct LlmCompactionOutput {
    /// Text from the response channel — the actual compaction summary.
    pub response: String,
    /// Usage reported by the provider for this sampling call. Recorded into
    /// the session total by the compactor so internal summary calls are not
    /// invisible to usage stats and the session token/cost limits.
    pub usage: crate::core::types::TokenUsage,
}

/// Error types for compaction sampling.
#[derive(Debug)]
pub enum CompactionSampleError {
    /// The sampler hit its end-to-end timeout. Transient.
    Timeout {
        timeout_secs: u64,
        collected_bytes: usize,
    },
    /// Sampler construction failed. Deterministic.
    Build(String),
    /// The sampling call could not be started.
    Start(String),
    /// The model produced no response. Transient.
    EmptyResponse,
    /// Anything else.
    Other(String),
}

impl std::fmt::Display for CompactionSampleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout {
                timeout_secs,
                collected_bytes,
            } => write!(
                f,
                "Compaction sampling timed out after {}s (collected {} bytes)",
                timeout_secs, collected_bytes
            ),
            Self::Build(msg) => write!(f, "Compaction sampler build failed: {}", msg),
            Self::Start(msg) => write!(f, "Compaction sampler start failed: {}", msg),
            Self::EmptyResponse => write!(f, "Compaction sampler returned no response"),
            Self::Other(msg) => write!(f, "{}", msg),
        }
    }
}

impl CompactionSampleError {
    /// Whether this error is deterministic — retrying produces the same failure.
    pub fn is_deterministic(&self) -> bool {
        match self {
            Self::Timeout { .. } | Self::EmptyResponse => false,
            Self::Build(_) | Self::Start(_) => true,
            Self::Other(_) => false,
        }
    }
}

/// Interface for the LLM call that produces compaction summaries.
#[async_trait]
pub trait CompactionSampler: Send + Sync {
    /// Run an LLM compaction call on the given items.
    ///
    /// `tools` are the SESSION's tool definitions — the summarization call
    /// carries the same tool set as the main request so its prompt prefix
    /// (system + tools + leading messages) hits the session's KV cache
    /// instead of starting cold (dsh cache discipline).
    async fn sample_compaction(
        &self,
        items: &[ConversationItem],
        system_prompt: &str,
        user_prompt: &str,
        tools: &[crate::core::types::ToolDefinition],
        timeout: Duration,
    ) -> Result<LlmCompactionOutput, CompactionSampleError>;
}

/// LLM-based sampler using the existing `LlmClient`.
pub struct LlmCompactionSampler {
    llm_client: LlmClient,
    model: String,
}

impl LlmCompactionSampler {
    pub fn new(llm_client: LlmClient, model: impl Into<String>) -> Self {
        Self {
            llm_client,
            model: model.into(),
        }
    }
}

#[async_trait]
impl CompactionSampler for LlmCompactionSampler {
    async fn sample_compaction(
        &self,
        items: &[ConversationItem],
        system_prompt: &str,
        user_prompt: &str,
        tools: &[crate::core::types::ToolDefinition],
        timeout: Duration,
    ) -> Result<LlmCompactionOutput, CompactionSampleError> {
        let mut messages = items.to_vec();
        if !user_prompt.is_empty() {
            messages.push(ConversationItem::user(user_prompt));
        }

        let request = LlmRequest {
            model: self.model.clone(),
            provider: None,
            messages,
            tools: tools.to_vec(),
            system_prompt: system_prompt.to_string(),
            temperature: Some(0.3),
            top_p: None,
            max_tokens: Some(2000),
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };

        let result = tokio::time::timeout(timeout, self.llm_client.complete(&request))
            .await
            .map_err(|_| CompactionSampleError::Timeout {
                timeout_secs: timeout.as_secs(),
                collected_bytes: 0,
            })?
            .map_err(|e| CompactionSampleError::Other(e.to_string()))?;

        if result.content.trim().is_empty() {
            return Err(CompactionSampleError::EmptyResponse);
        }

        Ok(LlmCompactionOutput {
            response: result.content,
            usage: result.usage,
        })
    }
}

/// Convert a `CompactionSampleError` to an `AppError`.
impl From<CompactionSampleError> for AppError {
    fn from(e: CompactionSampleError) -> Self {
        match e {
            CompactionSampleError::Timeout { timeout_secs, .. } => {
                AppError::LlmStreaming(format!("Compaction timeout after {}s", timeout_secs))
            }
            CompactionSampleError::Build(msg) => {
                AppError::Config(format!("Compaction build failed: {}", msg))
            }
            CompactionSampleError::Start(msg) => {
                AppError::LlmStreaming(format!("Compaction start failed: {}", msg))
            }
            CompactionSampleError::EmptyResponse => {
                AppError::LlmStreaming("Compaction returned empty response".into())
            }
            CompactionSampleError::Other(msg) => AppError::Internal(msg),
        }
    }
}

/// Maximum attempts when a compaction summary is degenerate.
pub const MAX_COMPACTION_ATTEMPTS: usize = 2;

/// Run a compaction LLM call, rejecting degenerate summaries.
///
/// The raw summary is sanitized (control-token neutralization) then
/// quality-classified. Empty / too-short / degenerate-marker summaries are
/// retried (up to `max_attempts`). Transient sampler errors (timeout, empty
/// response) are also retried; deterministic errors fail fast. When every
/// attempt is degenerate, returns `Ok(None)` so the caller keeps the recent
/// conversation instead of committing a lossy summary.
///
/// The returned usage is the SUM of every billed attempt — degenerate
/// summaries and retries were real API calls and must count toward the
/// session limits, not just the final successful one.
pub async fn run_compaction_summary(
    sampler: &dyn CompactionSampler,
    items: &[ConversationItem],
    system_prompt: &str,
    user_prompt: &str,
    tools: &[crate::core::types::ToolDefinition],
    timeout: Duration,
    max_attempts: usize,
) -> AppResult<Option<(String, crate::core::types::TokenUsage)>> {
    let mut total_usage = crate::core::types::TokenUsage::default();
    for attempt in 0..max_attempts {
        let raw = match sampler
            .sample_compaction(items, system_prompt, user_prompt, tools, timeout)
            .await
        {
            Ok(o) => {
                total_usage.add(&o.usage);
                o.response
            }
            Err(e) if e.is_deterministic() => return Err(AppError::from(e)),
            Err(e) => {
                tracing::warn!(error = %e, attempt, "Compaction sample failed — retrying");
                continue;
            }
        };
        let sanitized = crate::agent::compaction::templates::sanitize_summary(&raw);
        match crate::agent::compaction::templates::classify_summary(&sanitized) {
            crate::agent::compaction::templates::SummaryQuality::Ok => {
                return Ok(Some((sanitized, total_usage)));
            }
            quality => {
                tracing::warn!(
                    ?quality,
                    attempt,
                    "Degenerate compaction summary — retrying"
                );
            }
        }
    }
    tracing::warn!(
        max_attempts,
        "Compaction summary degenerate after all attempts — keeping conversation"
    );
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_is_not_deterministic() {
        assert!(!CompactionSampleError::Timeout {
            timeout_secs: 30,
            collected_bytes: 100
        }
        .is_deterministic());
    }

    #[test]
    fn build_is_deterministic() {
        assert!(CompactionSampleError::Build("bad config".into()).is_deterministic());
    }

    #[test]
    fn empty_response_is_not_deterministic() {
        assert!(!CompactionSampleError::EmptyResponse.is_deterministic());
    }

    #[test]
    fn error_display_contains_context() {
        let e = CompactionSampleError::Timeout {
            timeout_secs: 30,
            collected_bytes: 500,
        };
        let msg = e.to_string();
        assert!(msg.contains("30"));
        assert!(msg.contains("500"));
    }

    /// A scripted sampler used to verify usage aggregation across retries.
    struct ScriptedSampler {
        attempts: std::sync::Mutex<Vec<LlmCompactionOutput>>,
    }

    #[async_trait]
    impl CompactionSampler for ScriptedSampler {
        async fn sample_compaction(
            &self,
            _items: &[ConversationItem],
            _system_prompt: &str,
            _user_prompt: &str,
            _tools: &[crate::core::types::ToolDefinition],
            _timeout: Duration,
        ) -> Result<LlmCompactionOutput, CompactionSampleError> {
            let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
            Ok(attempts.remove(0))
        }
    }

    #[tokio::test]
    async fn compaction_usage_sums_every_billed_attempt() {
        // The first attempt is degenerate (too short) and gets retried; the
        // second is usable. Both were billed API calls, so the returned
        // usage must be their SUM — the session total and limits otherwise
        // undercount every internal summary call (audit H7 residual).
        let good = "A thorough compaction summary that preserves the original \
                    user intents, decisions, and unresolved items for the next \
                    turns of this conversation.";
        let sampler = ScriptedSampler {
            attempts: std::sync::Mutex::new(vec![
                LlmCompactionOutput {
                    response: "short".to_string(),
                    usage: crate::core::types::TokenUsage {
                        prompt_tokens: 100,
                        completion_tokens: 10,
                        ..Default::default()
                    },
                },
                LlmCompactionOutput {
                    response: good.to_string(),
                    usage: crate::core::types::TokenUsage {
                        prompt_tokens: 200,
                        completion_tokens: 20,
                        ..Default::default()
                    },
                },
            ]),
        };

        let (summary, usage) = run_compaction_summary(
            &sampler,
            &[],
            "system",
            "user",
            &[],
            Duration::from_secs(5),
            MAX_COMPACTION_ATTEMPTS,
        )
        .await
        .expect("usable summary")
        .expect("non-None after a usable attempt");

        assert!(summary.contains("thorough"));
        assert_eq!(usage.prompt_tokens, 300, "both billed attempts counted");
        assert_eq!(usage.completion_tokens, 30, "both billed attempts counted");
    }
}
