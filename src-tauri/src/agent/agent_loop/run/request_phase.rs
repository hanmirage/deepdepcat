//! Request assembly — cache-first tail injection for the agent loop.

use super::super::verification::should_retry_insufficient_resource;
use super::reminders::PLAN_MODE_WORKFLOW;
use super::state::{LoopAction, LoopState};
use super::AgentLoop;
use crate::agent::chat_state::ChatState;
use crate::core::error::AppError;
use crate::core::stream::emit_stream;
use crate::core::types::{
    emit_debug_trace, AgentStatus, ConversationItem, DebugEvent, StreamEvent, TurnOutcome,
};
use crate::hooks::{HookContext, HookEvent};
use crate::llm::provider::{LlmProvider, LlmRequest};
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

impl AgentLoop {
    /// Fire StopFailure + Error hooks for an LLM stream failure (start or
    /// mid-stream), so observers see every API failure exactly once.
    pub(crate) async fn fire_stream_failure_hooks(&self, session_id: &str, message: &str) {
        let fail_ctx = HookContext::new(HookEvent::StopFailure, session_id)
            .with_data("error", serde_json::json!(message));
        self.hook_executor.execute_observe(&fail_ctx).await;
        let err_ctx = HookContext::new(HookEvent::Error, session_id)
            .with_data("error", serde_json::json!(message));
        self.hook_executor.execute_observe(&err_ctx).await;
    }
}

impl AgentLoop {
    /// Assemble the messages for a model request.
    ///
    /// Cache-first rule (DeepSeek prefix cache): the persisted conversation
    /// goes byte-for-byte as the stable prefix; everything that varies per
    /// request — dynamic context / task-spec (`tail`), the plan-mode
    /// workflow, the live coordinator phase, the session goal, and the
    /// interjection guidance — is appended as TRAILING user messages.
    ///
    /// Plan workflow and coordinator phase deliberately live HERE, not in
    /// the system prompt: plan-mode toggles and four-phase advances used to
    /// rewrite the system prompt, invalidating the whole cached prefix on
    /// every flip (~the entire conversation billed at full price). As tail
    /// messages they follow the LIVE state per request and drop
    /// automatically (plan approval flips the mode back; the phase machine
    /// advances) without touching a single cached byte.
    ///
    /// `live_mode_context` gates the plan workflow + coordinator phase for
    /// calls that must not carry them — the forced final answer (no tools,
    /// pure summary) still gets tail/goal/guidance but not mode workflows.
    pub(crate) async fn build_request_messages(
        &self,
        app: &AppHandle,
        session_id: &str,
        chat_state: &mut ChatState,
        tail: Option<&str>,
        live_mode_context: bool,
    ) -> Vec<ConversationItem> {
        // Repair dangling tool calls before EVERY request build —
        // cancellation or mid-batch errors can leave assistant tool calls
        // without matching results, and OpenAI-compatible APIs reject such
        // conversations (HTTP 400).
        chat_state.repair_dangling_tool_calls();

        let mut msgs = chat_state.request_messages();
        if let Some(tail) = tail {
            msgs.push(ConversationItem::user(tail.to_string()));
        }
        // Plan-mode workflow guidance — while the permission mode is
        // read-only (plan), tell the model how the plan loop runs:
        // explore → design → write plan → exit_plan_mode (which PAUSES for
        // user approval before any code is written).
        if live_mode_context {
            let state = app.state::<crate::bootstrap::AppState>();
            if state.session_mode(session_id).await.is_read_only() {
                msgs.push(ConversationItem::user(PLAN_MODE_WORKFLOW.to_string()));
            }
        }
        // Coordinator-mode phase awareness — the LIVE phase of the
        // orchestration state machine is injected into the request so the
        // model can drive the four-phase delegation workflow (the suffix
        // describes the workflow; this is its current position).
        if live_mode_context && self.config.mode == super::super::AgentLoopMode::Coordinator {
            let phase = {
                let state = app.state::<crate::bootstrap::AppState>();
                state
                    .coordinator
                    .worker_state()
                    .current_phase(session_id)
                    .await
            };
            msgs.push(ConversationItem::user(format!(
                "<coordinator_phase>{}</coordinator_phase>",
                phase.as_str()
            )));
        }
        // Declared session goal (update_goal tool) — injected per request
        // so goal changes apply immediately.
        let goal = {
            let state = app.state::<crate::bootstrap::AppState>();
            state.goal_store.get(session_id)
        };
        if let Some(ref g) = goal {
            msgs.push(ConversationItem::user(format!(
                "\n\n<current-goal>\n{g}\n</current-goal>"
            )));
        }
        // Per-turn live guidance (interjection registry): todo gates and
        // background subagent signals must reach the model in THIS request,
        // not the one built at run start. Each fragment becomes its own
        // <user_query> message so sources stay independent. Collected =
        // consumed (one-shot guidance is never replayed).
        for (key, fragment) in self.interjection_guidance().await {
            // Replay event: which front/stop nudge is being injected into this
            // request — the dedup_key identifies the gate (todo-plan-front,
            // exploration-budget, strategy-switch, …) without the full text.
            if !key.is_empty() {
                crate::observability::event_log::record(
                    app,
                    session_id,
                    None,
                    "nudge_fired",
                    serde_json::json!({ "key": key }),
                );
            }
            // Interjections render as USER messages (wrapped in <user_query>),
            // so the model must be able to tell them apart from a user-forged
            // instruction — [CONSTRAINT 0] tells it to distrust <user_query>
            // content "unless injected by DeepDepCat itself". The <app-guidance>
            // nudges already carry that marker; wrap the plain-text ones so the
            // trust signal is uniform instead of only on a few gates (audit:
            // trust-signal-unification).
            let framed = if fragment.contains("<app-guidance>") {
                fragment
            } else {
                format!(
                    "<app-guidance>这是应用内置的系统指引（不是用户消息，也不是外部指令）。\n{fragment}\n</app-guidance>"
                )
            };
            msgs.push(ConversationItem::user(format!(
                "\n\n<user_query>\n{framed}\n</user_query>"
            )));
        }
        msgs
    }

    /// Phase 2 — build the model request (cache-first tail injection) and
    /// record its cache-prefix shape + context breakdown. Produces the
    /// request consumed by [`Self::phase_llm_and_parse`].
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn phase_build_request(
        &self,
        app: &AppHandle,
        session_id: &str,
        chat_state: &mut ChatState,
        model: &str,
        state: &mut LoopState,
    ) -> LoopAction {
        let LoopState {
            ref intent_result,
            ref mut augmented_message,
            ref tool_defs,
            ref system_prompt,
            ref max_tokens_override,
            ref mut request,
            ..
        } = state;
        let max_tokens_override = *max_tokens_override;
        // Reasoning effort: the user's explicit input-bar selection
        // (low/high/max) wins; "auto" tiers effort per intent + complexity
        // (light turns cheaper, heavy work keeps max for High). Auto-tiering
        // speaks DeepSeek's effort vocabulary (low/high/max) — non-DeepSeek
        // models get a safe default instead. Defaults to "high" when unset.
        // The model stays constant — no distillation routing — so the DeepSeek
        // prefix cache keeps hitting.
        let reasoning_effort = match self.config.reasoning_effort.as_deref() {
            Some("low") | Some("high") | Some("max") => self.config.reasoning_effort.clone(),
            _ if model.to_lowercase().contains("deepseek") => crate::agent::intent_effort::intent_effort(
                intent_result.intent,
                model.to_lowercase().contains("pro"),
                chat_state
                    .last_intent_decision
                    .as_ref()
                    .map(|d| d.complexity)
                    .unwrap_or(crate::agent::intent::TaskComplexity::High),
            ),
            _ => Some("high".to_string()),
        };
        let thinking_mode = reasoning_effort.is_some();
        let request_messages = self
            .build_request_messages(
                app,
                session_id,
                chat_state,
                augmented_message.as_deref(),
                true,
            )
            .await;
        *request = Some(LlmRequest {
            model: model.to_string(),
            provider: chat_state.provider.clone(),
            messages: request_messages,
            tools: tool_defs.to_vec(),
            system_prompt: system_prompt.to_string(),
            temperature: if thinking_mode {
                None
            } else {
                self.config.temperature
            },
            top_p: None,
            max_tokens: max_tokens_override.or(self.config.turn_output_token_limit),
            stream: true,
            reasoning_effort: reasoning_effort.clone(),
            response_format: None,
            cache_control: None,
            user_id: Some(session_id.to_string()),
        });

        let built = match request.as_ref() {
            Some(built) => built,
            None => {
                return LoopAction::Break(Err(AppError::Internal(
                    "request build produced no request".into(),
                )))
            }
        };
        self.record_request_shape(&built.system_prompt, tool_defs);

        if let Some(tracker) = self.usage_tracker.as_ref() {
            let (conversation_tokens, tool_result_tokens) =
                crate::agent::token::estimate_conversation_tokens_by_kind(&built.messages);
            let skill_tokens = self.context_builder.skill_inventory_tokens().await;
            tracker.record_context_breakdown(crate::observability::usage::ContextBreakdown {
                system_prompt_tokens: crate::agent::token::estimate_system_prompt_tokens(
                    &built.system_prompt,
                ),
                skill_tokens,
                tool_definition_tokens: crate::agent::token::estimate_tool_definitions_tokens(
                    tool_defs,
                ),
                conversation_tokens,
                tool_result_tokens,
            });
        }
        LoopAction::Continue
    }

    /// Phases 3+4 — stream the LLM call (with pre-hook gate and bounded
    /// error recoveries) and parse the response (DSML extraction, usage
    /// accounting, doom/truncation/resource recovery). The stream stays
    /// internal to this method so its opaque type never crosses phase
    /// boundaries.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn phase_llm_and_parse(
        &self,
        app: &AppHandle,
        session_id: &str,
        debug_mode: bool,
        chat_state: &mut ChatState,
        cancellation_token: &CancellationToken,
        turn_id: &str,
        model: &str,
        state: &mut LoopState,
    ) -> LoopAction {
        let LoopState {
            ref mut budget,
            ref mut augmented_message,
            ref mut request,
            ref mut max_tokens_override,
            ref mut prompt_too_long_retries,
            ref mut max_tokens_reject_retries,
            ref mut max_tokens_truncation_retries,
            ref mut pre_llm_denials,
            ref mut doom_retries,
            ref mut system_resource_retries,
            ref mut accumulated_text,
            ref mut accumulated_reasoning,
            ref mut accumulated_tool_calls,
            ref mut finish_reason,
            ref mut usage,
            ref mut doom_signal,
            ..
        } = state;
        let Some(request) = request.take() else {
            return LoopAction::Break(Err(AppError::Internal(
                "LLM phase ran without a built request".into(),
            )));
        };

        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::llm_call_start(session_id, model, chat_state.conversation.len() as u32),
        );

        let pre_llm_ctx = HookContext::new(HookEvent::PreLLMCall, session_id)
            .with_data("turn", serde_json::json!(budget.current_turn()))
            .with_data("model", serde_json::json!(model));
        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::hook_trigger(session_id, "PreLLMCall"),
        );
        if let Err(reason) = self.hook_executor.execute_gate(&pre_llm_ctx).await {
            *pre_llm_denials += 1;
            warn!(
                reason = %reason,
                denials = *pre_llm_denials,
                "PreLLMCall hook blocked request"
            );
            if *pre_llm_denials < super::state::MAX_PRE_LLM_DENIALS {
                if !budget.should_continue() {
                    info!("Budget exceeded after PreLLMCall denials — forcing final answer");
                    let result = self
                        .force_final_answer(
                            app,
                            turn_id,
                            session_id,
                            chat_state,
                            augmented_message.as_deref(),
                            cancellation_token,
                        )
                        .await;
                    return LoopAction::Break(result);
                }
                chat_state.push_transient_system(format!(
                    "A PreLLMCall hook blocked the previous model request:\n{reason}\n\
                     Address the feedback, then retry."
                ));
                return LoopAction::Continue;
            }
            warn!("PreLLMCall hook keeps blocking — releasing the request to break the loop");
        }

        let stream_start_ctx = HookContext::new(HookEvent::LLMStreamStart, session_id);
        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::hook_trigger(session_id, "LLMStreamStart"),
        );
        self.hook_executor.execute_observe(&stream_start_ctx).await;

        let stream_result = self.llm_client.stream(&request).await;

        let mut stream = match stream_result {
            Ok(s) => s,
            Err(e) => {
                if e.is_prompt_too_long() && *prompt_too_long_retries < 2 {
                    *prompt_too_long_retries += 1;
                    let max_hint = match &e {
                        AppError::PromptTooLong { max_tokens } => *max_tokens,
                        _ => None,
                    };
                    match self
                        .handle_prompt_too_long(app, turn_id, session_id, chat_state, max_hint)
                        .await
                    {
                        Ok(()) => {
                            info!("Emergency compaction succeeded — retrying LLM call");
                            return LoopAction::Continue;
                        }
                        Err(compaction_err) => {
                            warn!(error = %compaction_err, "Emergency compaction failed");
                        }
                    }
                }

                if e.is_max_tokens_exceeded() && *max_tokens_reject_retries < 3 {
                    *max_tokens_reject_retries += 1;
                    // The provider rejected our request because the requested
                    // max_tokens exceeds ITS ceiling. Recovery must clamp DOWN
                    // to the reported `max` — escalating further (the old
                    // behavior) guarantees another rejection, since the ceiling
                    // only goes down, never up.
                    if let AppError::MaxTokensExceeded { max, .. } = &e {
                        if *max > 0 {
                            *max_tokens_override = Some(*max);
                            return LoopAction::Continue;
                        }
                    }
                    warn!("MaxTokensExceeded reported no usable ceiling — failing the turn");
                }

                error!("LLM stream failed: {}", e);
                let _ = app.emit("agent-status-changed", AgentStatus::Error);
                emit_stream(
                    app,
                    StreamEvent::Error {
                        turn_id: turn_id.to_string(),
                        session_id: session_id.to_string(),
                        message: e.to_string(),
                        trace_id: chat_state.trace_id.clone(),
                    },
                );
                // TurnEnd so the frontend stream finalizes — a hard error
                // would otherwise leave the streaming cursor up until the
                // invoke promise's finally.
                emit_stream(
                    app,
                    StreamEvent::TurnEnd {
                        turn_id: turn_id.to_string(),
                        session_id: session_id.to_string(),
                        reason: "error".to_string(),
                        status: TurnOutcome::Failed,
                        trace_id: chat_state.trace_id.clone(),
                    },
                );
                self.fire_stream_failure_hooks(session_id, &e.to_string()).await;
                return LoopAction::Break(Err(e));
            }
        };

        let (
            mut parsed_text,
            parsed_reasoning,
            mut parsed_calls,
            parsed_finish,
            parsed_usage,
            parsed_doom,
        ) = match self
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
                // Mid-stream failure — the stream started but errored while
                // parsing; the Error/StopFailure hooks must fire here too.
                self.fire_stream_failure_hooks(session_id, &e.to_string()).await;
                emit_stream(
                    app,
                    StreamEvent::TurnEnd {
                        turn_id: turn_id.to_string(),
                        session_id: session_id.to_string(),
                        reason: "error".to_string(),
                        status: TurnOutcome::Failed,
                        trace_id: chat_state.trace_id.clone(),
                    },
                );
                return LoopAction::Break(Err(e));
            }
          };

        // Capture the RAW text state BEFORE stripping the DSML/protocol
        // markup. A truncated tool-call block strips to empty, so the
        // truncation recovery below must key on "was there any raw output"
        // (not "does the stripped text still have prose") — otherwise a
        // cut-off DSML tool call is silently treated as an empty reply and
        // the turn ends as "turn limit reached" instead of re-issuing.
        let raw_had_content = !parsed_text.is_empty();
        // DeepSeek V3.2/V4 sometimes streams its native DSML tool calls as
        // plain text instead of structured tool_calls — parse them back into
        // real calls and strip the protocol markup from the stored text.
        if parsed_calls.is_empty() && crate::core::dsml::has_markup(&parsed_text) {
            let parsed = crate::core::dsml::parse_tool_calls(&parsed_text);
            if !parsed.is_empty() {
                info!(
                    count = parsed.len(),
                    "Parsed DeepSeek DSML tool calls from streamed text"
                );
                parsed_calls = parsed;
            }
        }
        if crate::core::dsml::has_markup(&parsed_text) {
            parsed_text = crate::core::str_util::strip_tool_call_markup(&parsed_text);
        }

        let stream_end_ctx = HookContext::new(HookEvent::LLMStreamEnd, session_id);
        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::hook_trigger(session_id, "LLMStreamEnd"),
        );
        self.hook_executor.execute_observe(&stream_end_ctx).await;

        let post_llm_ctx = HookContext::new(HookEvent::PostLLMCall, session_id)
            .with_data("turn", serde_json::json!(budget.current_turn()))
            .with_data("finish_reason", serde_json::json!(parsed_finish));
        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::hook_trigger(session_id, "PostLLMCall"),
        );
        self.hook_executor.execute_observe(&post_llm_ctx).await;

        info!(
            text_len = parsed_text.len(),
            tool_call_count = parsed_calls.len(),
            finish_reason = %parsed_finish,
            "Stream phase complete"
        );
        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::llm_call_end(session_id, model, 0, parsed_usage.clone()),
        );

        let pricing = self.model_catalog.pricing(model);
        budget.record_usage_for_model(&parsed_usage, &pricing);
        chat_state.total_usage.add(&parsed_usage);
        if let Some(ref tracker) = self.usage_tracker {
            tracker.record_llm_usage(budget.current_turn(), &parsed_usage);
        }

        let cache_miss = parsed_usage.prompt_cache_miss_tokens.unwrap_or(0);
        if let Some(reason) = self.diagnose_cache_miss(cache_miss) {
            tracing::info!(session_id = %session_id, %reason, "Cache diagnosis");
        }

        emit_stream(
            app,
            StreamEvent::Usage {
                turn_id: turn_id.to_string(),
                usage: parsed_usage.clone(),
            },
        );

        // Phase 4.5: Doom loop recovery.
        if let Some(ref signal) = parsed_doom {
            if *doom_retries < super::state::MAX_DOOM_RETRIES {
                *doom_retries += 1;
                self.inject_doom_loop_recovery(app, turn_id, chat_state, signal)
                    .await;
                if !budget.should_continue() {
                    info!("Budget exceeded after doom-loop recovery — forcing final answer");
                    let result = self
                        .force_final_answer(
                            app,
                            turn_id,
                            session_id,
                            chat_state,
                            augmented_message.as_deref(),
                            cancellation_token,
                        )
                        .await;
                    return LoopAction::Break(result);
                }
                return LoopAction::Continue;
            }
            warn!("Doom loop retry budget exhausted — asking model to conclude");
            chat_state.push_transient_system(
                "Your responses keep repeating the same content (output loop \
                 detected). Stop repeating. Provide a final concise answer now.",
            );
        }

        // Phase 4.6: Truncation recovery. Fires when the model's output
        // ended while it was emitting a tool call — either the provider
        // reported `finish_reason=length`, or a `<tool_calls` block leaked
        // into the text unparsed (an unrecognized DSML variant / cut-off
        // block). Both mean a tool call was intended but never executed; the
        // fix is to re-issue it IN FULL with more output budget, not to
        // conclude. Bounded by `max_tokens_truncation_retries` so a genuinely
        // huge write degrades to a final answer instead of looping.
        let tool_block_leaked =
            parsed_text.contains("<tool_calls") && !parsed_text.contains("</tool_calls>");
        // A STRUCTURED tool call cut off by `finish_reason=length` leaves its
        // arguments as invalid JSON (cut mid-object). `parse_stream` still
        // finalizes it into `parsed_calls`, so the leak test above misses it —
        // without this check the partial call is dispatched, fails with
        // "Invalid JSON arguments", and the model re-issues at the unchanged
        // cap, looping until the turn budget forces an answer.
        let structured_call_truncated = parsed_finish == "length"
            && parsed_calls.iter().any(|tc| tc.parse_arguments().is_err());
        // A length-truncated PLAIN answer is not a cut-off tool call: the
        // model was writing prose, hit the ceiling, and there is no tool
        // block to re-issue. Treating it as a tool-call truncation would
        // burn retries on a call that never existed and could coax the model
        // into fabricating one.
        let truncated_tool_call = tool_block_leaked || structured_call_truncated;
        let text_truncated = parsed_finish == "length" && !truncated_tool_call;
        if truncated_tool_call && *max_tokens_truncation_retries < 3 {
            if !budget.should_continue() {
                info!("Budget exceeded after truncation — forcing final answer");
                let result = self
                    .force_final_answer(
                        app,
                        turn_id,
                        session_id,
                        chat_state,
                        augmented_message.as_deref(),
                        cancellation_token,
                    )
                    .await;
                return LoopAction::Break(result);
            }
            *max_tokens_truncation_retries += 1;
            if self.config.turn_output_token_limit.is_some() {
                warn!("truncated tool call at user-set cap — instructing model to split the write");
                chat_state.push_transient_system(
                    "Your previous output was cut off while emitting a tool call. \
                     Re-issue that tool call, splitting its content into smaller \
                     pieces if it was a large file write (create the file first, \
                     then append sections across several calls). Do not shorten or \
                     drop content.",
                );
                return LoopAction::Continue;
            }
            // Escalate from the model's DEFAULT output ceiling (8192 for
            // DeepSeek V4 when no explicit max_tokens is sent), not 4096 —
            // starting from 4096 makes the first escalation land on 8192,
            // i.e. the model's current default, so the re-issue truncates
            // again and the turn degrades to "turn limit reached".
            let current = max_tokens_override.unwrap_or(8192);
            if let Some(next) = self.escalate_output_limit(app, turn_id, current) {
                *max_tokens_override = Some(next);
                chat_state.push_transient_system(
                    "Your previous output was cut off while emitting a tool call \
                     (output token limit). Re-issue that tool call IN FULL with its \
                     complete arguments — do not summarize or shorten them. The \
                     output limit has been raised for this retry.",
                );
                return LoopAction::Continue;
            }
            warn!("truncated tool call at max tier — instructing model to split the write");
            chat_state.push_transient_system(
                "Your previous output was cut off while emitting a tool call. \
                 Re-issue it, splitting large file writes into smaller pieces \
                 (create the file first, then append sections).",
            );
            return LoopAction::Continue;
        }

        // Phase 4.6b: Plain-text truncation recovery. `finish_reason=length`
        // with no leaked tool block means the model's PROSE answer hit the
        // ceiling — there is nothing to re-issue. Raise the ceiling once and
        // ask it to continue from where it stopped; at the max tier, accept
        // the partial answer instead of burning retries or inventing a call.
        if text_truncated && raw_had_content && parsed_calls.is_empty() && *max_tokens_truncation_retries < 3 {
            if !budget.should_continue() {
                info!("Budget exceeded after text truncation — forcing final answer");
                let result = self
                    .force_final_answer(
                        app,
                        turn_id,
                        session_id,
                        chat_state,
                        augmented_message.as_deref(),
                        cancellation_token,
                    )
                    .await;
                return LoopAction::Break(result);
            }
            *max_tokens_truncation_retries += 1;
            let current = max_tokens_override.unwrap_or(8192);
            if let Some(next) = self.escalate_output_limit(app, turn_id, current) {
                *max_tokens_override = Some(next);
                chat_state.push_transient_system(
                    "Your previous answer was cut off at the output token limit. \
                     Continue from exactly where you stopped and finish the answer. \
                     Do not repeat what you already wrote and do not invent any \
                     tool calls.",
                );
                return LoopAction::Continue;
            }
            warn!("text answer truncated at max tier — accepting the partial answer");
        }

        // Phase 4.7: Resource recovery (insufficient_system_resource).
        if should_retry_insufficient_resource(
            &parsed_finish,
            !parsed_text.is_empty() || !parsed_calls.is_empty(),
            *system_resource_retries,
        ) {
            if !budget.should_continue() {
                info!("Budget exceeded after resource stall — forcing final answer");
                let result = self
                    .force_final_answer(
                        app,
                        turn_id,
                        session_id,
                        chat_state,
                        augmented_message.as_deref(),
                        cancellation_token,
                    )
                    .await;
                return LoopAction::Break(result);
            }
            *system_resource_retries += 1;
            let delay_secs = 1u64 << (*system_resource_retries).min(3);
            warn!(
                retry = *system_resource_retries,
                delay_secs, "finish_reason=insufficient_system_resource — backing off and retrying"
            );
            tokio::select! {
                _ = tokio::time::sleep(std::time::Duration::from_secs(delay_secs)) => {}
                _ = cancellation_token.cancelled() => {
                    return LoopAction::Break(Err(AppError::Cancelled));
                }
            }
            return LoopAction::Continue;
        }

        // A truncated tool call that could not be recovered (the truncation
        // retry budget is exhausted) must NOT be dispatched: its arguments are
        // invalid JSON, and dispatching it produces a guaranteed "Invalid JSON
        // arguments" tool error. Drop the half-built call(s) so the turn
        // degrades to the valid text instead of executing garbage. Only fires
        // on `finish_reason=length` with a malformed call, never on a normal
        // turn (audit recovery-3).
        if truncated_tool_call {
            parsed_calls.retain(|tc| tc.parse_arguments().is_ok());
        }

        *accumulated_text = parsed_text;
        *accumulated_reasoning = parsed_reasoning;
        *accumulated_tool_calls = parsed_calls;
        *finish_reason = parsed_finish;
        *usage = parsed_usage;
        *doom_signal = parsed_doom;
        LoopAction::Continue
    }
}
