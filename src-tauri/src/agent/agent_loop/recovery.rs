//! Loop recovery mechanisms — doom-loop injection, empty-response nudge,
//! soft termination, prompt-too-long compaction, and max-tokens escalation.
//!
//! These recovery paths are invoked from `run.rs` when the LLM stream
//! completes but the result is not directly usable, or when the API
//! returns a recoverable error:
//!
//! - **Doom loop** — the stream produced repetitive text. Inject a correction
//!   prompt and let the next iteration resample.
//! - **Empty response** — `content` is empty (possibly reasoning-only).
//!   Nudge the model to produce actual output. After three consecutive
//!   empties, force a final answer.
//! - **Soft termination** — the turn budget is exhausted but the model still
//!   requested tools. Skip the tools and force a final summary instead of
//!   leaving the conversation half-finished.
//! - **Prompt too long** — the API rejected the request because the prompt
//!   exceeds the context window. Trigger emergency compaction and retry.
//! - **Max tokens exceeded** — the API rejected the requested output length.
//!   Escalate `max_tokens` up the ladder (8k → 16k → 32k → 64k) and retry.

use super::AgentLoop;
use crate::agent::chat_state::ChatState;
use crate::core::error::{AppError, AppResult};
use crate::core::stream::emit_stream;
use crate::core::types::{StreamEvent, TurnOutcome};
use crate::hooks::{HookContext, HookEvent};
use crate::llm::provider::{LlmProvider, LlmRequest};
use crate::llm::retry::escalate_max_tokens;
use crate::llm::sampler::DoomLoopSignal;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Maximum consecutive empty responses before forcing a final answer.
const MAX_EMPTY_RESPONSES: u32 = 3;

impl AgentLoop {
    /// Inject a doom-loop correction prompt into the conversation.
    ///
    /// The correction tells the model what it was repeating and instructs it
    /// to produce the next actionable step. The next loop iteration will
    /// resample with this additional context.
    pub(super) async fn inject_doom_loop_recovery(
        &self,
        _app: &AppHandle,
        _turn_id: &str,
        chat_state: &mut ChatState,
        signal: &DoomLoopSignal,
    ) {
        let prompt = crate::llm::sampler::recovery_prompt(signal);

        warn!(
            repeated_unit = %signal.repeated_unit,
            count = signal.repetition_count,
            "Doom loop detected — injecting recovery prompt"
        );

        chat_state.push_transient_system(prompt);
    }

    /// Handle an empty (or reasoning-only) response from the model.
    ///
    /// Returns `Ok(())` when recovery was injected (the loop should continue)
    /// or `Err` when the recovery budget is exhausted (force final answer).
    pub(super) async fn handle_empty_response(
        &self,
        _app: &AppHandle,
        _turn_id: &str,
        chat_state: &mut ChatState,
        accumulated_text: &str,
        accumulated_reasoning: &str,
    ) -> AppResult<()> {
        chat_state.empty_response_count += 1;
        let count = chat_state.empty_response_count;

        debug!(
            empty_count = count,
            has_reasoning = !accumulated_reasoning.is_empty(),
            text_len = accumulated_text.len(),
            "Empty response detected"
        );

        if count >= MAX_EMPTY_RESPONSES {
            warn!(
                count,
                "Empty response budget exhausted — forcing final answer"
            );
            return Err(AppError::Internal(
                "Empty response budget exhausted".to_string(),
            ));
        }

        if !accumulated_reasoning.is_empty() && accumulated_text.trim().is_empty() {
            chat_state.push_transient_system(
                "You produced reasoning but no visible output. \
                 Based on your reasoning, output the complete analysis result or \
                 execute the appropriate tool. Do not return only thinking.",
            );
        } else {
            chat_state.push_transient_system(
                "Your previous response was empty. Provide a concrete analysis \
                 result, or use a tool to take action. An empty response is not acceptable.",
            );
        }

        Ok(())
    }

    /// Force a final answer when the turn budget is exhausted.
    ///
    /// Makes one last non-streaming LLM call with no tools, instructing the
    /// model to summarize its progress. The result is pushed as the final
    /// assistant message and the loop terminates.
    ///
    /// `tail` carries the run's per-request context (dynamic environment
    /// context + task-spec) so the final summary sees the task it is
    /// closing; goal and interjection guidance are included by the shared
    /// request builder. The prompt itself is TRANSIENT — it reaches this
    /// request but is never persisted, so a restarted session does not
    /// replay an internal "maximum iterations" nudge as if the user said it.
    pub(super) async fn force_final_answer(
        &self,
        app: &AppHandle,
        turn_id: &str,
        session_id: &str,
        chat_state: &mut ChatState,
        tail: Option<&str>,
        cancellation_token: &CancellationToken,
    ) -> AppResult<String> {
        // A cancelled session must not spend a full-context summarizer call:
        // the loop can reach the budget gate while a user cancel races it,
        // and the summary is pure waste when the turn is already ending.
        if cancellation_token.is_cancelled() {
            return Err(AppError::Cancelled);
        }

        // Replay event: the loop gave up and forced a read-only summary —
        // the "budget/limit trip" signal (the precise reason — turns vs
        // tokens vs empty-response — is in the caller's log line).
        crate::observability::event_log::record(
            app,
            session_id,
            Some(turn_id),
            "forced_final",
            serde_json::json!({}),
        );

        let prompt = "You have reached the maximum number of iterations. \
            Act as a read-only final summarizer for this turn: do NOT edit, \
            create, move, or delete any file, and do NOT run any command. \
            Write a concise final message (2-4 sentences) covering: (1) what \
            was delivered and where, (2) how to use it (commands/paths), and \
            (3) what remains or is blocked, if anything. Do not restate \
            tool-by-tool steps or repeat earlier summaries.";

        chat_state.push_transient_system(prompt);

        // Drain any pending interjections (todo/narration/exploration nudges)
        // before the final summarizer request — otherwise the model gets
        // "do NOT run any command" alongside a "make the next concrete tool
        // call" nudge registered moments ago by the same tool round. The
        // final summarizer must be read-only and unambiguous.
        let _ = self.interjection_guidance().await;

        // Repair dangling tool calls so the final request is well-formed even
        // if a previous tool batch was interrupted (OpenAI-compatible APIs
        // reject assistant tool calls without matching results).
        chat_state.repair_dangling_tool_calls();

        let system_prompt = self
            .context_builder
            .build_system_prompt(&chat_state.system_prompt)
            .await;
        // Effective effort: explicit tiers pass through; "auto" (None) falls
        // back to max here — a forced final summary has no user intent to
        // tier against, so keep full-strength thinking.
        let reasoning_effort = self
            .config
            .reasoning_effort
            .clone()
            .or_else(|| Some("max".to_string()));
        let thinking_mode = reasoning_effort.is_some();

        let request = LlmRequest {
            model: chat_state.model.clone(),
            provider: chat_state.provider.clone(),
            // Include transient guidance (reminders) and the run's tail /
            // goal / interjection context in the final request too —
            // `live_mode_context` is false so plan-workflow and coordinator
            // phase do not leak into the pure summary.
            messages: self
                .build_request_messages(app, session_id, chat_state, tail, false)
                .await,
            tools: vec![],
            system_prompt,
            temperature: if thinking_mode { None } else { Some(0.3) },
            top_p: None,
            max_tokens: None,
            stream: true,
            reasoning_effort,
            response_format: None,
            cache_control: None,
            user_id: Some(session_id.to_string()),
        };

        let stream_result = self.llm_client.stream(&request).await;
        let mut stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                self.fire_stream_failure_hooks(session_id, &e.to_string()).await;
                return Err(e);
            }
        };

        let (final_text, final_reasoning, _, finish_reason, usage, _) = match self
            .parse_stream(
                &mut stream,
                app,
                turn_id,
                session_id,
                cancellation_token,
                chat_state.trace_id.clone(),
            )
            .await
        {
            Ok(parsed) => parsed,
            Err(e) => {
                self.fire_stream_failure_hooks(session_id, &e.to_string()).await;
                return Err(e);
            }
        };

        chat_state.total_usage.add(&usage);

        if let Some(ref tracker) = self.usage_tracker {
            tracker.record_llm_usage(0, &usage);
        }

        // Replay-exact audit: the forced final summary is a model call too.
        crate::observability::event_log::record(
            app,
            session_id,
            Some(turn_id),
            "model_call",
            serde_json::json!({
                "model": &chat_state.model,
                "provider": &chat_state.provider,
                "finish_reason": finish_reason,
                "forced": true,
                "usage": {
                    "prompt": usage.prompt_tokens,
                    "completion": usage.completion_tokens,
                    "cache_hit": usage.prompt_cache_hit_tokens,
                    "cache_miss": usage.prompt_cache_miss_tokens,
                    "reasoning": usage.reasoning_tokens,
                },
            }),
        );

        emit_stream(
            app,
            StreamEvent::Usage {
                turn_id: turn_id.to_string(),
                usage: usage.clone(),
            },
        );

        // Strip XML tool-call protocol markup from the forced final answer
        // too — the provider may have emitted `<tool_calls>` text that must
        // not reach the persisted history or the frontend.
        let clean_text = crate::core::str_util::strip_tool_call_markup(&final_text);
        chat_state.push_assistant_message(clean_text, vec![], Some(usage), None);

        if !final_reasoning.is_empty() {
            debug!(
                reasoning_len = final_reasoning.len(),
                "Final answer reasoning"
            );
        }

        info!(finish_reason = %finish_reason, "Force final answer complete");

        // Emit TurnEnd so the frontend stream finalizes (this path bypasses
        // the normal loop exit — without it the UI relies on the invoke
        // promise's finally, which leaves the streaming cursor up longer).
        emit_stream(
            app,
            StreamEvent::TurnEnd {
                turn_id: turn_id.to_string(),
                session_id: session_id.to_string(),
                reason: if finish_reason == "length" {
                    "length".to_string()
                } else {
                    "stop".to_string()
                },
                status: TurnOutcome::Limit,
                trace_id: chat_state.trace_id.clone(),
            },
        );

        Ok(turn_id.to_string())
    }

    /// Handle a prompt-too-long error by triggering emergency compaction.
    ///
    /// When the API rejects the request because the prompt exceeds the context
    /// window, this method forces an aggressive compaction pass that reduces
    /// the conversation to the bare minimum needed for the current turn.
    /// The caller should retry the LLM call after this returns successfully.
    pub(super) async fn handle_prompt_too_long(
        &self,
        app: &AppHandle,
        _turn_id: &str,
        session_id: &str,
        chat_state: &mut ChatState,
        max_tokens_hint: Option<u64>,
    ) -> AppResult<()> {
        warn!(
            max_tokens_hint = ?max_tokens_hint,
            conversation_len = chat_state.conversation.len(),
            "Prompt too long — triggering emergency compaction"
        );

        // PreCompaction hook — emergency path is still a compaction pass.
        self.hook_executor
            .execute_observe(
                &HookContext::new(HookEvent::PreCompaction, session_id)
                    .with_data("force", serde_json::json!(true))
                    .with_data("reason", serde_json::json!("prompt_too_long")),
            )
            .await;

        // Force aggressive compaction — target a small token budget. Pass
        // the memory store + workspace so the emergency path externalizes
        // the dropped prefix into learnings like the normal path does.
        let state = app.state::<crate::bootstrap::AppState>();
        let memory = Some(state.memory.clone());
        let workspace = state.workspace.read().ok().and_then(|w| w.clone());
        let outcome = self
            .compactor
            .compact_with_budget(chat_state, 4096, 2, memory, workspace.as_deref())
            .await;
        let compacted = match outcome {
            Ok(Some(compacted)) => {
                info!(
                    compacted_tokens = compacted,
                    new_len = chat_state.conversation.len(),
                    "Emergency compaction succeeded after prompt-too-long"
                );
                compacted
            }
            Ok(None) => {
                warn!("Emergency compaction found nothing to compact");
                return Err(AppError::Internal(
                    "Cannot compact: conversation too short for compaction".to_string(),
                ));
            }
            Err(e) => {
                warn!(error = %e, "Emergency compaction failed");
                return Err(e);
            }
        };
        // PostCompaction hook — fire with the freed token count even though
        // the caller is about to retry the LLM call.
        self.hook_executor
            .execute_observe(
                &HookContext::new(HookEvent::PostCompaction, session_id)
                    .with_data("compacted_tokens", serde_json::json!(compacted))
                    .with_data("reason", serde_json::json!("prompt_too_long")),
            )
            .await;
        Ok(())
    }

    /// Escalate the output token limit for OUTPUT-TRUNCATION recovery — the
    /// model hit `finish_reason=length` mid-generation and needs more room to
    /// re-issue the truncated call/answer. NOT for the API-side
    /// `MaxTokensExceeded` REJECTION, which clamps DOWN to the provider's
    /// reported ceiling (see `request_phase.rs`) — escalating a rejected
    /// request only guarantees another rejection.
    ///
    /// Returns `Some(new_max_tokens)` if escalation succeeded, or `None` if
    /// already at the highest tier. The caller should retry the LLM call with
    /// the new `max_tokens` value.
    pub(super) fn escalate_output_limit(
        &self,
        _app: &AppHandle,
        _turn_id: &str,
        current_max: u64,
    ) -> Option<u64> {
        match escalate_max_tokens(current_max) {
            Some(next) => {
                info!(
                    current = current_max,
                    next, "Escalating max_tokens after MaxTokensExceeded error"
                );
                Some(next)
            }
            None => {
                warn!(
                    current = current_max,
                    "Already at highest max_tokens tier — cannot escalate further"
                );
                None
            }
        }
    }

    /// Ask the user whether to keep going after repeated tool failures
    /// (doom-loop decision). Returns `true` to continue, `false` to stop.
    ///
    /// Reuses the ask-user channel: the same frontend question dock renders
    /// it. Timeout / closed channel defaults to CONTINUE — the strategy-
    /// switch nudge still fires afterwards, so a silent user never leaves
    /// the model looping blindly.
    pub(super) async fn ask_doom_loop_continue(
        &self,
        app: &AppHandle,
        session_id: &str,
        overheated_tools: &[String],
    ) -> bool {
        let request_id = crate::core::ids::generate_id();
        let question = format!(
            "Agent 已连续失败：工具 {} 连续失败达到上限，继续硬试没有意义。\n\
             选择「继续」会让我换一种方法重试；选择「停止」则结束当前回合。",
            overheated_tools.join(", ")
        );
        let payload = serde_json::json!({
            "request_id": request_id,
            "session_id": session_id,
            "question": question,
            "options": ["继续", "停止"],
        });

        let (tx, rx) = tokio::sync::oneshot::channel();
        let state = app.state::<crate::bootstrap::AppState>();
        state.register_user_input_request(&request_id, tx).await;
        state
            .register_pending_interaction(session_id, "question", &request_id, question.clone())
            .await;
        // UserInputRequested hook — the doom-loop decision is a
        // human-in-the-loop point (audit / auto-answer via hook).
        state
            .hook_executor
            .execute_observe(
                &HookContext::new(HookEvent::UserInputRequested, session_id)
                    .with_data("request_id", json!(request_id))
                    .with_data("question", json!(question)),
            )
            .await;
        crate::permissions::plan::broadcast_pending_interactions(app, session_id).await;

        let _ = app.emit("ask-user", payload);

        // A shorter timeout than the 5-minute ask_user tool: the loop is
        // already stuck, and the strategy-switch nudge is the fallback.
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(120), rx).await;
        let response = match outcome {
            Ok(Ok(response)) => Some(response),
            Ok(Err(_)) | Err(_) => {
                let state = app.state::<crate::bootstrap::AppState>();
                state.remove_user_input_request(&request_id).await;
                None
            }
        };
        state
            .resolve_pending_interaction(session_id, &request_id)
            .await;
        crate::permissions::plan::broadcast_pending_interactions(app, session_id).await;

        match response {
            Some(text) => !looks_like_stop(&text),
            // Timeout / channel closed → keep going (with the nudge).
            None => true,
        }
    }
}

/// Whether a user's doom-loop answer is a "stop" decision. Anything
/// ambiguous defaults to continue — the strategy-switch nudge still
/// corrects the model, and a wrong "stop" on a garbled answer would
/// silently kill useful work.
fn looks_like_stop(answer: &str) -> bool {
    let lower = answer.trim().to_lowercase();
    [
        "停止", "停", "结束", "终止", "取消", "stop", "exit", "abort", "cancel", "quit",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_answers_are_recognized() {
        for answer in [
            "停止",
            "停吧",
            "结束吧",
            "终止",
            "取消",
            "stop",
            "Stop!",
            "exit",
            "abort",
            "cancel",
            "quit",
        ] {
            assert!(looks_like_stop(answer), "must be stop: {answer}");
        }
    }

    #[test]
    fn continue_answers_default_to_continue() {
        for answer in [
            "继续",
            "换种方法继续",
            "continue",
            "再试一次",
            "好的",
            "123",
        ] {
            assert!(!looks_like_stop(answer), "must continue: {answer}");
        }
    }

    #[test]
    fn empty_answer_is_continue() {
        assert!(!looks_like_stop(""));
        assert!(!looks_like_stop("   "));
    }
}
