//! Hook evaluator traits — abstractions for prompt and agent hook execution.
//!
//! The hooks module defines these traits so it never depends directly on the
//! LLM or agent modules. The wiring happens in [`AppState::initialize`]
//! where concrete implementations are passed to [`HookExecutor`].
//!
//! - [`PromptEvaluator`] — sends a prompt to an LLM and returns a gate decision.
//! - [`AgentEvaluator`] — spawns a subagent to evaluate an event.

use crate::hooks::types::{HookContext, HookDefinition, HookResult};
use crate::observability::usage::SessionUsageTracker;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

/// Evaluates a prompt-type hook by sending it to an LLM.
///
/// The implementor is responsible for:
/// 1. Sending the prompt to the LLM
/// 2. Parsing the response for allow/deny
/// 3. Returning a [`HookResult`]
#[async_trait]
pub trait PromptEvaluator: Send + Sync {
    /// Evaluate a prompt and return the gate decision.
    async fn evaluate(&self, prompt: &str, context: &HookContext) -> HookResult;
}

/// Evaluates an agent-type hook by spawning a subagent.
///
/// The implementor is responsible for:
/// 1. Constructing a subagent prompt from the hook context
/// 2. Running the subagent
/// 3. Parsing the subagent's response for allow/deny
#[async_trait]
pub trait AgentEvaluator: Send + Sync {
    /// Evaluate an event with a subagent and return the gate decision.
    async fn evaluate(&self, hook: &HookDefinition, context: &HookContext) -> HookResult;
}

/// A real LLM-backed prompt evaluator.
///
/// Sends the hook prompt (plus context) to the configured model and parses
/// the response for an allow/deny verdict:
/// - Response containing `ALLOW` (or no verdict) → allow
/// - Response containing `DENY:<reason>` → deny with reason
pub struct LlmPromptEvaluator {
    llm_client: crate::llm::client::LlmClient,
    model: String,
    /// Session-scoped usage trackers (AppState registry). Hook LLM calls
    /// used to be invisible in the usage stats — the safety gate's tokens
    /// now count toward the session that triggered the hook.
    usage_trackers: Option<Arc<tokio::sync::Mutex<HashMap<String, SessionUsageTracker>>>>,
}

impl LlmPromptEvaluator {
    /// Attach the session usage registry — hook verdict calls then record
    /// their billed tokens per session (turn-0 channel, no per-turn slot).
    pub fn with_usage_trackers(
        llm_client: crate::llm::client::LlmClient,
        model: impl Into<String>,
        usage_trackers: Option<Arc<tokio::sync::Mutex<HashMap<String, SessionUsageTracker>>>>,
    ) -> Self {
        Self {
            llm_client,
            model: model.into(),
            usage_trackers,
        }
    }
}

#[async_trait]
impl PromptEvaluator for LlmPromptEvaluator {
    async fn evaluate(&self, prompt: &str, context: &HookContext) -> HookResult {
        use crate::core::types::ConversationItem;
        use crate::llm::provider::{LlmProvider, LlmRequest};

        let system_prompt = "You are a safety gate for a coding agent. Given a prompt about \
            a proposed action, respond with exactly one line:\n\
            - `ALLOW` if the action is safe and appropriate.\n\
            - `DENY:<reason>` if the action should be blocked.\n\
            Do not output anything else.";

        let user_content = format!(
            "Event: {}\nSession: {}\nTool: {}\nArguments: {}\n\nHook prompt:\n{}",
            context.event.as_str(),
            context.session_id,
            context.tool_name.as_deref().unwrap_or("-"),
            context
                .tool_args
                .as_ref()
                .map(|v| v.to_string())
                .unwrap_or_else(|| "-".to_string()),
            prompt,
        );

        let request = LlmRequest {
            model: self.model.clone(),
            provider: None,
            messages: vec![ConversationItem::user(user_content)],
            system_prompt: system_prompt.to_string(),
            stream: false,
            ..Default::default()
        };

        match self.llm_client.complete(&request).await {
            Ok(response) => {
                record_hook_usage(&self.usage_trackers, &context.session_id, &response.usage).await;
                let verdict = response.content.trim().to_uppercase();
                info!(verdict = %verdict, "Prompt hook verdict");
                if verdict.starts_with("DENY") {
                    let reason = response
                        .content
                        .trim()
                        .trim_start_matches("DENY")
                        .trim_start_matches(':')
                        .trim();
                    HookResult::deny(if reason.is_empty() {
                        "Denied by prompt hook".to_string()
                    } else {
                        reason.to_string()
                    })
                } else {
                    HookResult::allow().with_output(response.content.trim().to_string())
                }
            }
            Err(e) => {
                // Fail-closed: the safety gate must never silently open when
                // the LLM is down (network jitter disables the gate). The
                // hook pipeline has no ask-channel, so the nearest safe
                // behavior is a denial with an explicit reason — the agent
                // stops and surfaces it, and the user decides manually.
                tracing::warn!(error = %e, "Prompt hook LLM call failed — failing closed");
                HookResult::deny(format!(
                    "Prompt hook unavailable (LLM error: {e}) — failing closed; \
                     ask the user before proceeding"
                ))
            }
        }
    }
}

/// A real LLM-backed agent evaluator.
///
/// Agent-type hooks are evaluated with the same verdict protocol as prompt
/// hooks (`ALLOW` / `DENY:<reason>`) — the hook's prompt field carries the
/// evaluation instruction, and the LLM decides whether the operation may
/// proceed. Registered at startup so agent hooks are no longer a silent
/// fail-open.
pub struct LlmAgentEvaluator {
    inner: LlmPromptEvaluator,
}

impl LlmAgentEvaluator {
    /// Attach the session usage registry (see [`LlmPromptEvaluator`]).
    pub fn with_usage_trackers(
        llm_client: crate::llm::client::LlmClient,
        model: impl Into<String>,
        usage_trackers: Option<Arc<tokio::sync::Mutex<HashMap<String, SessionUsageTracker>>>>,
    ) -> Self {
        Self {
            inner: LlmPromptEvaluator::with_usage_trackers(llm_client, model, usage_trackers),
        }
    }
}

#[async_trait]
impl AgentEvaluator for LlmAgentEvaluator {
    async fn evaluate(&self, hook: &HookDefinition, context: &HookContext) -> HookResult {
        let prompt = hook.prompt.as_deref().unwrap_or_default();
        self.inner.evaluate(prompt, context).await
    }
}

/// Record a hook verdict call's usage into the session's tracker, when the
/// registry has one for that session. Missing sessions (hook fired before
/// the tracker was created) are skipped — the call still landed in the
/// durable global aggregate only if the tracker exists; this is best-effort
/// accounting, never a gate.
async fn record_hook_usage(
    trackers: &Option<Arc<tokio::sync::Mutex<HashMap<String, SessionUsageTracker>>>>,
    session_id: &str,
    usage: &crate::core::types::TokenUsage,
) {
    let Some(trackers) = trackers else { return };
    if let Some(tracker) = trackers.lock().await.get(session_id) {
        tracker.record_llm_usage(0, usage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn hook_usage_lands_in_session_tracker() {
        let trackers: Arc<tokio::sync::Mutex<HashMap<String, SessionUsageTracker>>> =
            Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        trackers
            .lock()
            .await
            .insert("sess-1".to_string(), SessionUsageTracker::new("sess-1"));

        record_hook_usage(
            &Some(trackers.clone()),
            "sess-1",
            &crate::core::types::TokenUsage {
                prompt_tokens: 700,
                completion_tokens: 90,
                ..Default::default()
            },
        )
        .await;

        let summary = trackers
            .lock()
            .await
            .get("sess-1")
            .expect("tracker exists")
            .summary();
        assert_eq!(summary.total_prompt_tokens, 700);
        assert_eq!(summary.total_completion_tokens, 90);
        assert_eq!(summary.turn_count, 0, "hook usage must not create a turn");
    }

    #[tokio::test]
    async fn hook_usage_skips_unknown_session() {
        let trackers: Arc<tokio::sync::Mutex<HashMap<String, SessionUsageTracker>>> =
            Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        trackers
            .lock()
            .await
            .insert("sess-1".to_string(), SessionUsageTracker::new("sess-1"));

        // A different session (or one torn down) must not panic or double
        // count — the hook is best-effort accounting.
        record_hook_usage(
            &Some(trackers.clone()),
            "sess-ghost",
            &crate::core::types::TokenUsage {
                prompt_tokens: 1,
                ..Default::default()
            },
        )
        .await;
        let summary = trackers
            .lock()
            .await
            .get("sess-1")
            .expect("tracker exists")
            .summary();
        assert_eq!(summary.total_prompt_tokens, 0);
    }

    #[tokio::test]
    async fn hook_usage_noop_without_registry() {
        record_hook_usage(
            &None,
            "sess-1",
            &crate::core::types::TokenUsage {
                prompt_tokens: 1,
                ..Default::default()
            },
        )
        .await;
    }
}
