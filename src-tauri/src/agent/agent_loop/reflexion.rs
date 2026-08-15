//! Reflexion self-critique after tool execution rounds.
//!
//! After each tool execution round in Reflexion mode, this module asks the LLM
//! to reflect on the actions taken and suggest improvements. The critique is
//! appended as a system message to the conversation.
//!
//! Enhancements over the naive self-critique:
//! - **Structured reflection prompt** — evaluates goal progress, obstacles,
//!   and concrete next actions instead of free-form musing.
//! - **Deduplication** — the critique references the specific tools used in
//!   this round, so repeated rounds produce distinct, useful reflections.
//! - **Framed as a system reminder** — the critique is wrapped in the
//!   conventional `<system-reminder>` shape the model already associates
//!   with contextual guidance.

use super::AgentLoop;
use crate::core::error::{AppError, AppResult};
use crate::core::types::{ConversationItem, ToolCall};
use crate::llm::provider::{LlmProvider, LlmRequest};
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;
use tracing::debug;

impl AgentLoop {
    /// Run a Reflexion self-critique step.
    ///
    /// `model_override` selects a dedicated verify-role model (P3-6 model
    /// matrix) when configured; `None` uses the conversation's own model.
    pub(super) async fn run_reflexion_critique(
        &self,
        app: &AppHandle,
        session_id: &str,
        chat_state: &mut crate::agent::chat_state::ChatState,
        tool_calls: &[ToolCall],
        cancellation_token: &CancellationToken,
        model_override: Option<&str>,
    ) -> AppResult<()> {
        if cancellation_token.is_cancelled() {
            return Err(AppError::Cancelled);
        }

        // No tools used this round — nothing concrete to reflect on.
        if tool_calls.is_empty() {
            return Ok(());
        }

        let tools_used: Vec<String> = tool_calls.iter().map(|t| t.name.clone()).collect();
        let tools_summary = if tools_used.is_empty() {
            "none".to_string()
        } else {
            tools_used.join(", ")
        };

        // The critique asks about GOAL PROGRESS — the declared session goal
        // must be visible to answer honestly (it lives in the main request
        // tail, but this is an independent request with its own prompt).
        let goal_line = {
            let state = app.state::<crate::bootstrap::AppState>();
            match state.goal_store.get(session_id) {
                Some(g) if !g.trim().is_empty() => {
                    format!(" The user's declared goal is: \"{g}\"")
                }
                _ => String::new(),
            }
        };

        let critique_prompt = format!(
            "Reflect on the tool calls you just made ({tools_summary}).{goal_line} Answer these three questions:\n\
             1. GOAL PROGRESS: Did the actions advance the user's goal? Cite concrete results.\n\
             2. OBSTACLE: What, if anything, is blocking progress right now?\n\
             3. NEXT ACTION: What is the single most effective next step?\n\
             Be specific and concise (2-4 sentences total)."
        );

        let request = LlmRequest {
            model: model_override
                .filter(|m| !m.is_empty())
                .unwrap_or(&chat_state.model)
                .to_string(),
            provider: chat_state.provider.clone(),
            messages: {
                chat_state.repair_dangling_tool_calls();
                let mut msgs = chat_state.conversation_snapshot().to_vec();
                msgs.push(ConversationItem::user(critique_prompt));
                msgs
            },
            tools: vec![],
            system_prompt: "You are a self-reflection assistant embedded in a coding agent.\n\
                Provide a brief, honest, action-oriented critique. Format:\n\
                [Progress] <one sentence>\n\
                [Obstacle] <one sentence>\n\
                [Next] <one sentence>"
                .to_string(),
            temperature: Some(0.3),
            top_p: None,
            max_tokens: Some(300),
            stream: false,
            reasoning_effort: None,
            response_format: None,
            cache_control: None,
            user_id: None,
        };

        let response = self.llm_client.complete(&request).await?;

        // Non-streaming usage accounting (#88 audit H7): reflexion critiques
        // previously vanished from usage stats — the tracker only recorded
        // the streaming path, and the session total (which seeds the next
        // run's budget and persists to the usage pages) missed it too.
        chat_state.total_usage.add(&response.usage);
        if let Some(ref tracker) = self.usage_tracker {
            tracker.record_llm_usage(0, &response.usage);
        }

        let critique = response.content.trim().to_string();
        if !critique.is_empty() {
            // Reflexion critique is injected as a transient system reminder so
            // the LLM sees it as contextual guidance (not a separate turn) —
            // and it never persists to the session database. It is deliberately
            // NOT streamed to the frontend: the user asked about image
            // processing, not the agent's internal self-critique — streaming
            // it as a text delta surfaced noisy English reflection blocks
            // (and occasional encoding artifacts) in the chat.
            chat_state.push_transient_system(format!(
                "<system-reminder>\n[Self-Reflection after tools: {}]\n{}\n</system-reminder>",
                tools_summary, critique
            ));

            debug!(critique_len = critique.len(), "Reflexion critique added");
        }

        Ok(())
    }
}
