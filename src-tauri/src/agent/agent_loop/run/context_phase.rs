//! Early loop phases — pause gate, Plan-Execute gate, and context
//! management (background injection, tiered compaction, decompose
//! suggestion, system reminders).

use super::super::gates::LIGHT_TASK_GUIDANCE;
use super::super::AgentLoopMode;
use super::reminders::{build_activity_reminder, TAIL_GUIDANCE_ALLOWANCE_TOKENS};
use super::state::{compose_augmented_message, LoopAction, LoopState};
use super::AgentLoop;
use crate::agent::chat_state::ChatState;
use crate::agent::system_reminder::ReminderConfig;
use crate::core::error::AppError;
use crate::core::stream::emit_stream;
use crate::core::types::{emit_debug_trace, AgentStatus, DebugEvent, StreamEvent};
use crate::hooks::{HookContext, HookEvent};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

impl AgentLoop {
    /// Pause gate — the loop parks here while the session is suspended,
    /// until resume or cancel. Placed after the cancel check so a cancelled
    /// run can never park forever.
    pub(super) async fn phase_pause_gate(
        &self,
        app: &AppHandle,
        session_id: &str,
        turn_id: &str,
        cancellation_token: &CancellationToken,
    ) -> LoopAction {
        {
            let state = app.state::<crate::bootstrap::AppState>();
            if let Some(mut pause_rx) = state.session_paused_receiver(session_id).await {
                if *pause_rx.borrow() {
                    // SessionPause hook — lifecycle observability when the
                    // loop parks for a paused session.
                    let pause_ctx = HookContext::new(HookEvent::SessionPause, session_id);
                    self.hook_executor.execute_observe(&pause_ctx).await;
                    let _ = app.emit("agent-status-changed", AgentStatus::Paused);
                    while *pause_rx.borrow() {
                        tokio::select! {
                            _ = pause_rx.changed() => {}
                            _ = cancellation_token.cancelled() => break,
                        }
                    }
                    // SessionResume hook — fired on the way out regardless of
                    // whether the exit was a resume or a cancellation (the
                    // cancellation path below reports the true outcome).
                    let resume_ctx = HookContext::new(HookEvent::SessionResume, session_id);
                    self.hook_executor.execute_observe(&resume_ctx).await;
                    if cancellation_token.is_cancelled() {
                        emit_stream(
                            app,
                            StreamEvent::TurnEnd {
                                turn_id: turn_id.to_string(),
                                session_id: session_id.to_string(),
                                reason: "cancelled".to_string(),
                                status: crate::core::types::TurnOutcome::Cancelled,
                                trace_id: None,
                            },
                        );
                        emit_stream(
                            app,
                            StreamEvent::Error {
                                turn_id: turn_id.to_string(),
                                session_id: session_id.to_string(),
                                message: "Cancelled by user".to_string(),
                                trace_id: None,
                            },
                        );
                        return LoopAction::Break(Err(AppError::Cancelled));
                    }
                }
            }
        }
        LoopAction::Continue
    }

    /// Plan-Execute gate — structural read-only plan phase for PlanExecute
    /// runs, followed by the per-turn start trace.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn phase_plan_gate(
        &self,
        app: &AppHandle,
        session_id: &str,
        debug_mode: bool,
        chat_state: &mut ChatState,
        state: &mut LoopState,
    ) -> LoopAction {
        let LoopState {
            ref budget,
            ref mut intent_result,
            ref mut plan_phase_active,
            ref mut plan_approved_this_run,
            ..
        } = state;
        if self.config.mode == AgentLoopMode::PlanExecute
            && !*plan_approved_this_run
            && intent_result.intent.is_actionable()
        {
            let mode = {
                let state = app.state::<crate::bootstrap::AppState>();
                state.session_mode(session_id).await
            };
            if mode.is_read_only() {
                if !*plan_phase_active {
                    *plan_phase_active = true;
                    chat_state.push_transient_system(super::PLAN_EXECUTE_PLAN_INSTRUCTION);
                    info!(session_id, "Plan-Execute: forced plan phase (read-only)");
                    return LoopAction::Continue;
                }
                // Plan phase ongoing — the model is exploring/planning
                // or parked in an exit_plan_mode approval wait.
            } else if *plan_phase_active {
                // The mode flipped back from read-only: the plan was
                // approved (or the user released plan mode). Disarm the
                // gate for the rest of this run — execution proceeds.
                *plan_approved_this_run = true;
                let state = app.state::<crate::bootstrap::AppState>();
                state.broadcast_plan_mode(app, session_id).await;
                info!(
                    session_id,
                    "Plan-Execute: plan approved — execution unlocked"
                );
            } else {
                // First iteration with a normal permission mode: force
                // the plan phase before any write can happen.
                let state = app.state::<crate::bootstrap::AppState>();
                state
                    .set_plan_previous_mode(session_id, mode.as_str().to_string())
                    .await;
                state
                    .set_session_mode(session_id, crate::permissions::mode::PermissionMode::ReadOnly)
                    .await;
                state.broadcast_plan_mode(app, session_id).await;
                *plan_phase_active = true;
                chat_state.push_transient_system(super::PLAN_EXECUTE_PLAN_INSTRUCTION);
                info!(
                    session_id,
                    "Plan-Execute: plan phase enabled — writes locked"
                );
                return LoopAction::Continue;
            }
        }

        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::agent_turn_start(
                session_id,
                budget.current_turn(),
                self.config.mode.as_str(),
            ),
        );
        LoopAction::Continue
    }

    /// Context management — background result injection, tiered compaction,
    /// decompose suggestion, and periodic system reminders. Produces the
    /// per-iteration system prompt and tool definitions for later phases.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn phase_context(
        &self,
        app: &AppHandle,
        session_id: &str,
        chat_state: &mut ChatState,
        reminder_config: &ReminderConfig,
        loop_start: &Instant,
        turn_id: &str,
        user_message: &str,
        state: &mut LoopState,
    ) -> LoopAction {
        let LoopState {
            ref budget,
            ref mut reminder_state,
            ref mut augmented_message,
            ref mut dynamic_ctx,
            ref mut tail_suffix,
            ref intent_result,
            ref mut decompose_suggested,
            ref mut tool_defs,
            ref mut system_prompt,
            ..
        } = state;
        *tool_defs = self.tool_dispatcher.tool_definitions();

        // Refresh the volatile dynamic context per iteration (git status,
        // current time) so a long turn reasons against CURRENT state, not a
        // run-start snapshot. The memory injection is cached per user message
        // (ContextBuilder) and structure/cognition/skills are already cached —
        // only git and time are genuinely re-read, and git has its own short
        // TTL. The one-shot task-spec suffix is preserved and re-appended.
        {
            let (fresh_ctx, _) = self
                .context_builder
                .build_dynamic_context(user_message)
                .await;
            *dynamic_ctx = if fresh_ctx.is_empty() {
                None
            } else {
                Some(fresh_ctx)
            };
            *augmented_message = compose_augmented_message(dynamic_ctx, tail_suffix);
        }
        // Keyword activation: skills with `when-to-use` frontmatter activate
        // when the user's message mentions their keywords (a 小红书 template
        // activates as soon as the user asks for 小红书 content, without
        // waiting for a file touch). Idempotent — re-evaluating the same
        // message every iteration inserts the same ids.
        {
            let state = app.state::<crate::bootstrap::AppState>();
            state
                .skill_engine
                .activate_for_message(user_message, self.context_builder.work_mode())
                .await;
        }

        // Phase 1a: drain completed background subagent results.
        self.inject_background_results(app, session_id, chat_state)
            .await;

        // Phase 1a': drain async-hook wake-ups — an `async_rewake` hook
        // exited with code 2 while the loop was mid-turn; inject its
        // message so the model can fix the failure before stopping.
        let wakes = {
            let state = app.state::<crate::bootstrap::AppState>();
            state.drain_async_hook_wakes(session_id).await
        };
        for wake in wakes {
            chat_state.push_transient_system(format!(
                "<system-reminder>\n[async hook wake]\nAn async verification hook \
                 reported a failure:\n{wake}\nFix the reported problem before \
                 concluding — or, if it is an external blocker, say so in your \
                 final message.</system-reminder>"
            ));
        }

        // Phase 1b: tiered compaction — the system prompt is composed HERE
        // (once per iteration) so the estimate reflects the REAL request.
        *system_prompt = format!(
            "{}{}",
            self.context_builder
                .build_system_prompt(&chat_state.system_prompt)
                .await,
            self.config.mode.system_prompt_suffix()
        );
        // Goal tokens contribute to the per-request tail — lifted out so the
        // post-prune re-measure below uses the same allowance.
        let goal_tokens = {
            let state = app.state::<crate::bootstrap::AppState>();
            state
                .goal_store
                .get(session_id)
                .as_deref()
                .map(crate::agent::token::estimate_text_tokens)
                .unwrap_or(0)
        };
        let estimated_tokens = chat_state.estimated_full_request_tokens(
            system_prompt,
            augmented_message.as_deref(),
            goal_tokens + TAIL_GUIDANCE_ALLOWANCE_TOKENS,
            tool_defs,
        );
        let window_tokens = chat_state.context_window.max(1);
        let usage_ratio = estimated_tokens as f64 / window_tokens as f64;

        // Tier 1: 50% — report once, never nag.
        if usage_ratio >= 0.5 && !chat_state.soft_warning_sent {
            chat_state.soft_warning_sent = true;
            chat_state.push_transient_system(format!(
                "[Context usage notice] The conversation is at {:.0}% of the context \
                 window ({estimated_tokens}/{window_tokens} tokens). Keep responses \
                 and tool outputs concise from now on.",
                usage_ratio * 100.0
            ));
        }

        // Tier 2: 60% — cheaply snip stale tool results (no LLM call).
        if usage_ratio >= 0.6 {
            self.compactor.snip_stale_tool_results(chat_state).await;
        }

        // Tier 3/4: prefire + (force-)compaction at threshold. The prefire
        // decision uses the FULL estimate (conversation + transient + tail +
        // goal), not the conversation-only count — tail-heavy sessions would
        // otherwise prefire past the compaction threshold and lose the
        // prefire → consume win.
        self.compactor
            .maybe_prefire(
                chat_state,
                self.config.auto_compact_threshold_percent,
                estimated_tokens,
            )
            .await;

        // The compaction decision re-measures the FULL request AFTER the
        // tier-2 prune (dsh's prune → re-measure → summarize): a snip that
        // already brought the request under threshold skips the LLM summary,
        // and the decision reflects the post-prune conversation instead of
        // the stale pre-prune estimate that triggered the snip. Using the
        // conversation-only count here undercounts the tail and would skip
        // every compact in the 80-90% window.
        let compact_estimate = chat_state.estimated_full_request_tokens(
            system_prompt,
            augmented_message.as_deref(),
            goal_tokens + TAIL_GUIDANCE_ALLOWANCE_TOKENS,
            tool_defs,
        );
        let compact_ratio = compact_estimate as f64 / window_tokens as f64;
        let needs_compaction = compact_ratio
            >= (self.config.auto_compact_threshold_percent as f64 / 100.0)
            || compact_ratio >= 0.9;

        if needs_compaction {
            let _ = app.emit("agent-status-changed", AgentStatus::Thinking);
            let force = compact_ratio >= 0.9;
            // PreCompaction hook — lifecycle observability before the
            // summarization pass (hooks can save state / audit the trigger).
            let pre_ctx = HookContext::new(HookEvent::PreCompaction, session_id)
                .with_data("force", serde_json::json!(force));
            self.hook_executor.execute_observe(&pre_ctx).await;
            let mut compacted_tokens: u64 = 0;
            let state = app.state::<crate::bootstrap::AppState>();
            let workspace = state
                .workspace
                .read()
                .ok()
                .and_then(|w| w.clone());
            // DeepSeek optimization: cache-aware compaction (summary reuses
            // the session prefix, prune-before-summarize) only for DeepSeek
            // sessions with the setting on.
            let cache_optimize = {
                let on = state
                    .config()
                    .map(|c| c.agent.deepseek_auto_reasoning)
                    .unwrap_or(false);
                on && chat_state.model.to_lowercase().contains("deepseek")
            };
            match self
                .compactor
                .compact_if_needed(
                    chat_state,
                    tool_defs,
                    self.config.auto_compact_threshold_percent,
                    force,
                    cache_optimize,
                    Some(compact_estimate),
                    state.memory.clone(),
                    workspace.as_deref(),
                )
                .await
            {
                Ok(Some(tokens)) => {
                    compacted_tokens = tokens;
                    emit_stream(
                        app,
                        StreamEvent::Compaction {
                            session_id: session_id.to_string(),
                            compacted_tokens,
                            summary: "Conversation compacted".to_string(),
                        },
                    );
                    let reminder = {
                        let state = app.state::<crate::bootstrap::AppState>();
                        let running: Vec<_> = state
                            .background_tasks
                            .list(session_id)
                            .into_iter()
                            .filter(|t| t.is_running())
                            .collect();
                        let goal = state.goal_store.get(session_id);
                        build_activity_reminder(&running, goal.as_deref())
                    };
                    if !reminder.is_empty() {
                        chat_state.push_transient_system(reminder);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    warn!("Compaction failed: {}", e);
                }
            }
            // PostCompaction hook — fires after the pass with the number of
            // tokens actually compacted (0 when nothing was compacted).
            let post_ctx = HookContext::new(HookEvent::PostCompaction, session_id)
                .with_data("compacted_tokens", serde_json::json!(compacted_tokens));
            self.hook_executor.execute_observe(&post_ctx).await;
        }

        // Phase 1.5: decomposition suggestion — one-shot per user message.
        if !*decompose_suggested {
            *decompose_suggested = true;
            if let Some(guidance) =
                crate::agent::intent::suggest_decompose(user_message, intent_result.intent)
            {
                self.register_interjection(
                    crate::agent::interjection::Interjection::new(
                        "decompose",
                        crate::agent::interjection::InterjectionPriority::Normal,
                        guidance,
                    )
                    .with_dedup_key(format!("decompose:{}", turn_id)),
                )
                .await;
            } else if crate::agent::intent::light_task_signal(user_message, intent_result.intent) {
                self.register_interjection(
                    crate::agent::interjection::Interjection::new(
                        "scope",
                        crate::agent::interjection::InterjectionPriority::Normal,
                        LIGHT_TASK_GUIDANCE,
                    )
                    .with_dedup_key(format!("scope:{}", turn_id)),
                )
                .await;
            }
        }

        // Phase 1.5: periodic system reminder injection.
        if reminder_state.should_inject(reminder_config, budget.current_turn()) {
            let elapsed = loop_start.elapsed().as_secs();
            let token_usage = chat_state.total_usage.total();
            if let Some(text) = reminder_state.build_reminder(
                reminder_config,
                budget.current_turn(),
                elapsed,
                token_usage,
            ) {
                reminder_state.set_pending(text);
            }
        }
        if let Some(reminder_text) = reminder_state.take_pending() {
            chat_state.push_transient_system(reminder_text);
        }

        LoopAction::Continue
    }
}
