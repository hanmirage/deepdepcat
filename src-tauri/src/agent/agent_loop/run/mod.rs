//! Main agent loop execution — the `run()` entry point.
//!
//! Orchestrates the 7-phase loop: context management → build request →
//! LLM call → parse response → tool execution → loop decision.

mod background;
mod context_phase;
mod housekeeping;
mod reminders;
mod request_phase;
mod state;
mod stop_gates;
mod tool_phase;

use self::state::{compose_augmented_message, LoopAction, LoopState, StopGateCounters};
use super::AgentLoop;
use crate::agent::budget::BudgetTracker;
use crate::agent::chat_state::ChatState;
use crate::agent::system_reminder::{ReminderConfig, ReminderState};
use crate::core::error::{AppError, AppResult};
use crate::core::stream::emit_stream;
use crate::core::types::{
    emit_debug_trace, AgentStatus, DebugEvent, StreamEvent, TokenUsage, TurnOutcome,
};
use crate::hooks::{HookContext, HookEvent};
use crate::workspace::checkpoint::FileStateTracker;
use std::collections::HashMap;
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;
use tracing::info;

/// Plan-Execute gate instruction — injected (transient, one-shot) when the
/// loop force-enables the read-only Plan permission mode at the start of a
/// PlanExecute run. Structural enforcement: write tools are hard-blocked by
/// the permission system until `exit_plan_mode` is approved, so the plan
/// phase cannot be skipped by the model "forgetting" the mode contract.
const PLAN_EXECUTE_PLAN_INSTRUCTION: &str = "\
<plan_execute>
You are in Plan-Execute mode: no file changes happen before the user
approves your plan. The permission system is currently read-only — write
tools are BLOCKED until you present a plan via exit_plan_mode and the
user approves it.

Workflow:
1. Explore the codebase with read-only tools (list_dir / read_file /
   grep / search_symbols / file_dependencies) so the plan is grounded in
   the actual code.
2. Design the implementation: the exact files to change, what changes
   inside each, and how you will verify (tests/lint/typecheck).
3. Call exit_plan_mode with the FULL plan text in the `plan` argument —
   the user reviews it in the approval panel. On approval your write
   tools unlock and you execute the approved plan step by step.
If the plan is rejected, revise it to address the feedback and call
exit_plan_mode again.
</plan_execute>";

impl AgentLoop {
    /// Run the agent loop for a single user message.
    ///
    /// Wrapper that guarantees an `Idle` status is emitted on EVERY exit path
    /// (success or error), so the sidebar status dot never gets stuck.
    #[allow(clippy::too_many_arguments)]
    pub async fn run(
        &self,
        app: &AppHandle,
        session_id: &str,
        chat_state: &mut ChatState,
        user_message: &str,
        cancellation_token: &CancellationToken,
        debug_mode: bool,
        file_state_tracker: Option<FileStateTracker>,
        skill_engine: Option<&crate::skills::activation::SkillActivationEngine>,
    ) -> AppResult<String> {
        let result = self
            .run_inner(
                app,
                session_id,
                chat_state,
                user_message,
                cancellation_token,
                debug_mode,
                file_state_tracker,
                skill_engine,
            )
            .await;

        // Always return to Idle — including on errors and cancellation — so the
        // sidebar agent-status dot doesn't stay stuck in Thinking/ToolRunning.
        let _ = app.emit("agent-status-changed", AgentStatus::Idle);

        // AssistantMessage hook — the turn produced a final reply. The
        // turn_id lets observers pull the authoritative snapshot for the
        // final text; no content is duplicated onto the wire.
        if let Ok(turn_id) = &result {
            let assistant_ctx = HookContext::new(HookEvent::AssistantMessage, session_id)
                .with_data("turn_id", serde_json::json!(turn_id));
            self.hook_executor
                .execute_observe(&assistant_ctx)
                .await;
        }
        // FatalError hook — the loop died on a non-cancelled error (a
        // user cancel is a normal outcome, not a fatal one).
        if let Err(e) = &result {
            if !e.is_cancelled() {
                let fatal_ctx = HookContext::new(HookEvent::FatalError, session_id)
                    .with_data("error", serde_json::json!(e.to_string()));
                self.hook_executor.execute_observe(&fatal_ctx).await;
            }
        }

        result
    }

    /// Internal implementation of the agent loop.
    #[allow(clippy::too_many_arguments)]
    async fn run_inner(
        &self,
        app: &AppHandle,
        session_id: &str,
        chat_state: &mut ChatState,
        user_message: &str,
        cancellation_token: &CancellationToken,
        debug_mode: bool,
        file_state_tracker: Option<FileStateTracker>,
        skill_engine: Option<&crate::skills::activation::SkillActivationEngine>,
    ) -> AppResult<String> {
        chat_state.repair_dangling_tool_calls();

        // Begin turn tracking for checkpoint/rewind.
        let turn_index = chat_state.prompt_index;
        if let Some(ref tracker) = file_state_tracker {
            tracker.begin_turn(turn_index).await;
        }

        // Coordinator mode: a new user message starts a NEW orchestration —
        // reset the phase machine to Research when the previous batch of
        // THIS session's workers fully finished (an in-flight orchestration
        // keeps its phase so follow-up messages continue it; the machine is
        // per-session, so other sessions never block the reset).
        if self.config.mode == super::AgentLoopMode::Coordinator {
            let state = app.state::<crate::bootstrap::AppState>();
            if state
                .coordinator
                .worker_state()
                .reset_if_idle(session_id)
                .await
            {
                info!(
                    session_id,
                    "Coordinator: fresh orchestration — phase reset to research"
                );
            }
        }

        // Async-hook wakes are turn-scoped: a wake that missed its loop
        // (the hook finished after the turn ended) must not surface in a
        // later user message as if it were fresh.
        {
            let state = app.state::<crate::bootstrap::AppState>();
            state.clear_async_hook_wakes(session_id).await;
        }

        // Run-start timestamp — worker edits that finish after this point
        // belong to THIS run's verification evidence (see Phase 5.4).
        let run_started_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Build dynamic context (git, time, chips, memory) and prepend to
        // the user message. The system prompt stays static for KV Cache.
        //
        // The conversation (and therefore persisted history + rewind recall)
        // stores the CLEAN user message. The dynamic context is injected only
        // into the request sent to the model — see `augment_user_message`.
        let (dynamic_ctx, memory_injection) = self
            .context_builder
            .build_dynamic_context(user_message)
            .await;
        // Surface "memory referenced" — emitted before the turn's first LLM
        // call so the UI can show a non-intrusive marker on the reply.
        if let Some(inj) = memory_injection {
            emit_stream(
                app,
                StreamEvent::MemoryInjected {
                    session_id: session_id.to_string(),
                    count: inj.count as u32,
                    snippet: inj.snippet,
                },
            );
        }
        // Tail content injected as a SEPARATE trailing user message in the
        // request (cache-first design): historical messages keep their exact
        // persisted bytes every turn, so the DeepSeek prefix cache keeps
        // hitting the whole conversation — only the tail changes per request.
        // (Previously the dynamic context REPLACED the last user message,
        // whose bytes then reverted to the clean text next turn — breaking
        // the prefix from that point and missing the whole previous reply.)
        //
        // The dynamic context is split from the task-spec + hook suffix so
        // `phase_context` can REFRESH the volatile parts (git status, time)
        // each iteration while the one-shot spec stays fixed.
        let dynamic_ctx_opt: Option<String> = if dynamic_ctx.is_empty() {
            None
        } else {
            Some(dynamic_ctx)
        };
        let mut tail_suffix: Option<String> = None;

        // Turn-scoped skill-activation reset. The engine is a single
        // app-global Arc shared by every session and work mode; without a
        // per-turn reset, a skill activated by one message (keyword) or one
        // workspace (path) stays active for the app's lifetime and leaks
        // into unrelated conversations. activate_for_message re-adds keyword
        // skills for this message; record_file_touch re-adds path skills as
        // tools touch matching files during the turn. Also refresh the
        // workspace root so path patterns (`docs/**/*.md`) match absolute
        // tool paths relativized to THIS session's workspace.
        {
            let state = app.state::<crate::bootstrap::AppState>();
            state.skill_engine.reset_activation().await;
            if let Some(ws) = self.context_builder.workspace().clone() {
                state.skill_engine.set_workspace_root(ws).await;
            }
        }

        // ── Intent Understanding Layer ────────────────────────────────────
        // Classify the user message (heuristic, zero latency) and:
        // 1. Route substantive messages through one bounded LLM call that
        //    decides intent + complexity + planning/subagent needs.
        // 2. Inject a <task-spec> (intent + acceptance + complexity +
        //    planning-required + delegation) onto the current request.
        // 3. Seed the session goal automatically when none is declared yet —
        //    the model can refine it later via update_goal.
        // 4. Track goal drift (code work without file tools) per turn.
        // The heuristic decision is the fallback on any routing failure.
        let mut intent_result = crate::agent::intent::classify(user_message);
        let (mut intent_decision, routing_usage) =
            if crate::agent::intent::is_followup_continuation(user_message) {
                // Short follow-ups ("继续", "再优化一下", "改成 X") inherit the
                // previous substantive intent instead of being re-classified as
                // casual chat — otherwise the effort tier drops to low and the
                // task-spec loses the goal mid-task.
                match chat_state.last_intent_decision.clone() {
                    Some(prev) => {
                        tracing::info!(
                            intent = ?prev.intent,
                            "Follow-up continuation — inheriting previous intent decision"
                        );
                        (prev, TokenUsage::default())
                    }
                    None => (
                        crate::agent::intent::heuristic_decision(
                            user_message,
                            intent_result.intent,
                        ),
                        TokenUsage::default(),
                    ),
                }
            } else if crate::agent::intent::needs_llm_routing(user_message, intent_result.intent) {
                crate::agent::intent::route_with_llm(
                    &self.llm_client,
                    user_message,
                    &chat_state.model,
                    chat_state.provider.as_deref(),
                    intent_result.intent,
                )
                .await
            } else {
                (
                    crate::agent::intent::heuristic_decision(user_message, intent_result.intent),
                    TokenUsage::default(),
                )
            };
        // Multi-intent messages ("1. 做 A 2. 做 B", "顺便…另外…") must
        // always lay out a todo plan — the LLM router may underestimate
        // when it only sees the headline clause.
        if !crate::agent::intent::is_followup_continuation(user_message)
            && crate::agent::intent::split_sub_asks(user_message).len() > 1
        {
            intent_decision.needs_planning = true;
        }
        // Store the intent decision for follow-up inheritance ONLY when the
        // task is actionable. A terse follow-up ("继续") must inherit the
        // LAST REAL TASK's decision, not a casual exchange that happened to
        // precede it — inheriting a Chat/Low decision gave a coding
        // continuation the 25-turn budget and truncated it. Non-actionable
        // turns leave the previous task's decision in place.
        if intent_decision.intent.is_actionable() {
            chat_state.last_intent_decision = Some(intent_decision.clone());
        }
        intent_result.intent = intent_decision.intent;
        // Ambiguity guard: a FIRST message with a bare pronoun ("这个/那个/
        // 它/这段") and no concrete anchor must trigger a clarifying
        // question instead of a guess.
        if crate::agent::intent::needs_clarification(user_message)
            && !chat_state
                .conversation
                .iter()
                .any(|i| matches!(i, crate::core::types::ConversationItem::User(_)))
        {
            chat_state.push_transient_system(
                "此请求包含歧义引用（「这个/那个/它/这段」等），且这是会话的第一条消息，\
                 没有可指代的上下文。不要猜测指代对象：先用 ask_user 问一个澄清问题\
                 （最多一个问题），得到明确指代后再动手。"
                    .to_string(),
            );
        }
        // The per-message intent-routing call is billed like any other LLM
        // call: it previously vanished from session accounting, so usage
        // stats, the seeded budget, and the session token/cost limits all
        // missed it (audit H7 residual). Recording here — BEFORE the budget
        // is constructed below — makes the routing tokens count toward this
        // run's session-level limits from the first turn.
        if routing_usage.total() > 0 {
            chat_state.total_usage.add(&routing_usage);
            if let Some(ref tracker) = self.usage_tracker {
                tracker.record_llm_usage(0, &routing_usage);
            }
        }
        {
            let state = app.state::<crate::bootstrap::AppState>();
            let declared_goal = state.goal_store.get(session_id);
            if intent_result.intent.is_actionable() && declared_goal.is_none() {
                if let Some(ref draft) = intent_result.goal_draft {
                    state.goal_store.set(session_id, draft.clone());
                    let _ = app.emit(
                        "goal-updated",
                        crate::tools::builtin::update_goal::GoalEvent {
                            session_id: session_id.to_string(),
                            goal: Some(draft.clone()),
                        },
                    );
                }
            }
            if let Some(spec) = crate::agent::intent::build_task_spec(
                &intent_result,
                user_message,
                &intent_decision,
            ) {
                tail_suffix = Some(match tail_suffix {
                    Some(prev) => format!("{prev}{spec}"),
                    None => spec,
                });
            }
        }
        // Dynamic turn budget (#84 audit): the old flat 50 turns let a
        // wandering agent loop for 40+ turns before forced conclusion. The
        // intent layer already classifies task scale — reuse it so small
        // tasks finish fast and only genuinely complex work gets the full
        // budget. EvaluatorQa/Coordinator keep generous caps: their loops
        // are quality machinery, not exploration. Computed before LoopState
        // because it seeds the turn budget.
        let dynamic_max_turns: u32 = match self.config.mode {
            super::AgentLoopMode::EvaluatorQa => self.config.max_turns.max(50),
            super::AgentLoopMode::Coordinator => self.config.max_turns.max(50),
            super::AgentLoopMode::Goal => self.config.max_turns.max(50),
            _ => {
                let base = self.config.max_turns.min(50);
                // Reuse the routed decision's complexity — the raw text
                // heuristics miss a High-complexity task phrased tersely
                // ("全面重构数据层"), which would otherwise get a small
                // budget and truncate mid-run. Every NON-light task gets the
                // 40-turn budget: the cap is a fuse against runaway loops,
                // and a normal task finishes on its own well before it (a
                // higher cap costs nothing — the agent stops when done).
                // Only a genuinely light one-off edit stays at 12.
                match intent_decision.complexity {
                    crate::agent::intent::TaskComplexity::High
                    | crate::agent::intent::TaskComplexity::Medium
                    | crate::agent::intent::TaskComplexity::Low => {
                        if crate::agent::intent::light_task_signal(
                            user_message,
                            intent_result.intent,
                        ) {
                            base.min(12)
                        } else {
                            base.min(40)
                        }
                    }
                }
            }
        };
        let model = chat_state.model.clone();
        // No distillation routing: the user picks the model (flash or pro)
        // themselves. Keeping the session model constant lets the DeepSeek
        // prefix cache keep hitting the whole conversation — automatic
        // model switches would miss each other's KV cache every time.
        let effective_model = model.clone();

        // ── LoopState ────────────────────────────────────────────────────
        // All mutable loop state in one bundle — previously ~20 `let mut`
        // locals threaded through phase calls as 8–31 individual `&mut`
        // arguments. Phases take `&mut LoopState` and destructure only the
        // fields they touch; run_inner reads/writes `state.field`. Run-scoped
        // constants (turn_id, user_message, cancellation_token, …) stay
        // locals — they never mutate across iterations.
        // Compose the initial tail BEFORE moving the parts into LoopState.
        let composed_tail = compose_augmented_message(&dynamic_ctx_opt, &tail_suffix);
        let mut state = LoopState {
            dynamic_ctx: dynamic_ctx_opt,
            tail_suffix,
            augmented_message: composed_tail,
            intent_result,
            tool_defs: Vec::new(),
            system_prompt: String::new(),
            request: None,
            goal_drift_count: 0,
            accumulated_text: String::new(),
            accumulated_reasoning: String::new(),
            accumulated_tool_calls: Vec::new(),
            finish_reason: String::new(),
            usage: TokenUsage::default(),
            doom_signal: None,
            max_tokens_override: None,
            prompt_too_long_retries: 0,
            max_tokens_reject_retries: 0,
            max_tokens_truncation_retries: 0,
            pre_llm_denials: 0,
            doom_retries: 0,
            system_resource_retries: 0,
            run_has_tool_activity: false,
            consecutive_denials: 0,
            edited_files: Vec::new(),
            bash_wrote_files: false,
            verification_tier: super::verification::VerificationTier::None,
            verification_failed: false,
            // Populated after `push_user_message` below so the scan includes
            // this turn's user message.
            executed_results: HashMap::new(),
            executed_results_scanned_len: 0,
            decompose_suggested: false,
            exploration_rounds: 0,
            exploration_nudges: 0,
            todo_plan_nudged: false,
            todo_sync_nudged: false,
            todo_order_nudged: false,
            last_narration_text: None,
            repeat_narration_streak: 0,
            reflexion_rounds: 0,
            code_verify_nudged: false,
            plan_phase_active: false,
            plan_approved_this_run: false,
            budget: BudgetTracker::with_config_seeded(
                crate::agent::budget::BudgetConfig {
                    max_turns: dynamic_max_turns,
                    session_token_limit: self.config.session_token_limit,
                    session_cost_limit: self.config.session_cost_limit,
                    max_wall_clock_secs: self.config.run_timeout_secs,
                    // Cost accounting follows the ACTUAL session model — flash-class
                    // sessions priced at pro rates would hit the cost limit ~8x
                    // early (and pro sessions priced at flash rates would overrun).
                    // Read from the model catalog so non-DeepSeek models (GPT-4o,
                    // Claude) are billed at their real per-model rate, not the
                    // DeepSeek fallback.
                    pricing: self.model_catalog.pricing(&model),
                },
                // Seed with usage accrued before this invocation — the session
                // token/cost limits are session-scoped, not per-message (#88
                // audit H6: previously each user message got a fresh zero budget,
                // so the configured cap was silently multiplied per message).
                &chat_state.total_usage,
            ),
            reminder_state: ReminderState::new(),
            counters: StopGateCounters::default(),
        };

        // Persist/rewind the clean text; environment context is per-request.
        chat_state.push_user_message(user_message);

        let turn_id = crate::core::ids::turn_id();

        emit_stream(
            app,
            StreamEvent::TurnStart {
                turn_id: turn_id.clone(),
                session_id: session_id.to_string(),
                model: model.clone(),
                trace_id: chat_state.trace_id.clone(),
            },
        );

        let _ = app.emit("agent-status-changed", AgentStatus::Thinking);

        // Trigger AgentLoopStart hook (observe-only)
        let loop_start_ctx = HookContext::new(HookEvent::AgentLoopStart, session_id);
        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::hook_trigger(session_id, "AgentLoopStart"),
        );
        self.hook_executor.execute_observe(&loop_start_ctx).await;

        let reminder_config = ReminderConfig::default();
        let loop_start = Instant::now();

        // Per-run resets of session-scoped counters: the empty-response
        // nudge budget and the auto-diagnostics evidence are per USER
        // MESSAGE, not per session. A run must not inherit the previous
        // run's nudges (one empty answer then forces a final summary) nor
        // trust diagnostics pulled for an edit from an older message.
        chat_state.empty_response_count = 0;
        chat_state.auto_diagnostics.clear();
        // TodoGate may fire up to twice per run — one nudge was easy for the
        // model to brush past while stopping with steps still in flight.
        // Gate-chain budget: the stop path may force at most this many extra
        // turns via BUILT-IN nudges (todo / narration / background / verify /
        // plan-checklist) per run. Every gate that consumes the budget
        // increments `stop_nudges` — a gate that only CHECKED it (todo
        // excepted) left the other `stop_nudges < BUDGET` conditions
        // effectively unbounded, letting the worst path (todo 2× + narration
        // + bg + verify + plan + …) stack ~10 full-context calls on a single
        // user message. After the budget, the nudges release the turn — the
        // independent evaluator gates are NOT part of this budget (they are
        // the quality backstop and keep their own rounds).

        // ── UserMessage hook (UserPromptSubmit semantics) ──────────────
        // Fires once per user message, before the loop starts. Hooks can
        // inject `additionalContext` (e.g. fresh lint results) that is
        // appended to the per-request tail — visible to the model for the
        // whole turn, never persisted.
        let user_hook_ctx = HookContext::new(HookEvent::UserMessage, session_id)
            .with_data("message", serde_json::json!(user_message));
        let user_hook_contexts = self
            .hook_executor
            .execute_observe_collect(&user_hook_ctx)
            .await;
        if !user_hook_contexts.is_empty() {
            let joined = user_hook_contexts.join("\n\n");
            let wrapped = format!(
                "<system-reminder>\n[hook UserMessage]\n{joined}\n</system-reminder>"
            );
            let prev_suffix = state.tail_suffix.take();
            state.tail_suffix = Some(match prev_suffix {
                Some(prev) => format!("{prev}\n\n{wrapped}"),
                None => wrapped,
            });
            state.augmented_message =
                compose_augmented_message(&state.dynamic_ctx, &state.tail_suffix);
        }

        // ── Verification evidence cache (#88 audit) ───────────────────────
        // Tool results are mapped once per run and then kept incrementally
        // in sync with newly pushed results, instead of rescanning the whole
        // conversation (and cloning every tool-result payload — bash output
        // can be large) on every tool round.
        state.executed_results = chat_state.tool_results_by_call_id();
        state.executed_results_scanned_len = chat_state.conversation.len();

        // ── Main Loop ─────────────────────────────────────────────────────
        // TurnEnd events are emitted at the loop's break points with the
        // true reason: "stop" (normal), "length" (output limit cut the
        // answer), or "cancelled" (cancel path, above).
        //
        // Single-exit housekeeping: EVERY exit path (normal stop,
        // cancellation, stream error, budget final-answer) assigns
        // `loop_result` and breaks out of the loop, so the shared
        // `finish_loop_housekeeping` below runs exactly once — the rewind
        // snapshot and the AgentLoopEnd hook are never skipped.
        // Every loop exit assigns `loop_result` before breaking (cancel,
        // stream error, budget final-answer, denial cap, normal stop) — no
        // initial value is needed.
        let loop_result: AppResult<String>;
        loop {
            if cancellation_token.is_cancelled() {
                emit_stream(
                    app,
                    StreamEvent::TurnEnd {
                        turn_id: turn_id.clone(),
                        session_id: session_id.to_string(),
                        reason: "cancelled".to_string(),
                        status: TurnOutcome::Cancelled,
                        trace_id: chat_state.trace_id.clone(),
                    },
                );
                emit_stream(
                    app,
                    StreamEvent::Error {
                        turn_id: turn_id.clone(),
                        session_id: session_id.to_string(),
                        message: "Cancelled by user".to_string(),
                        trace_id: chat_state.trace_id.clone(),
                    },
                );
                loop_result = Err(AppError::Cancelled);
                break;
            }

            // ── Phase 0: Pause gate ──────────────────────────────────────
            match self
                .phase_pause_gate(app, session_id, &turn_id, cancellation_token)
                .await
            {
                LoopAction::Continue => {}
                LoopAction::Break(r) => {
                    loop_result = r;
                    break;
                }
            }

            state.budget.begin_turn();
            state.reminder_state.on_turn_start();

            // ── AgentLoopTurn hook (per-iteration observability) ───────
            // Fires once per loop iteration, after the turn counter has
            // advanced, so observers can track iteration number + progress.
            let turn_ctx = HookContext::new(HookEvent::AgentLoopTurn, session_id)
                .with_data("iteration", serde_json::json!(state.budget.current_turn()));
            self.hook_executor.execute_observe(&turn_ctx).await;

            // ── Phase 0.5: Plan-Execute Gate ─────────────────────────────
            match self
                .phase_plan_gate(app, session_id, debug_mode, chat_state, &mut state)
                .await
            {
                LoopAction::Continue => {}
                LoopAction::Break(r) => {
                    loop_result = r;
                    break;
                }
            }

            // ── Phase 1: Context Management ──────────────────────────────
            match self
                .phase_context(
                    app,
                    session_id,
                    chat_state,
                    &reminder_config,
                    &loop_start,
                    &turn_id,
                    user_message,
                    &mut state,
                )
                .await
            {
                LoopAction::Continue => {}
                LoopAction::Break(r) => {
                    loop_result = r;
                    break;
                }
            }

            // ── Phase 2: Build Request ────────────────────────────────────
            match self
                .phase_build_request(app, session_id, chat_state, &effective_model, &mut state)
                .await
            {
                LoopAction::Continue => {}
                LoopAction::Break(r) => {
                    loop_result = r;
                    break;
                }
            }

            // ── Phases 3+4: LLM Call + Parse Response ─────────────────────
            match self
                .phase_llm_and_parse(
                    app,
                    session_id,
                    debug_mode,
                    chat_state,
                    cancellation_token,
                    &turn_id,
                    &effective_model,
                    &mut state,
                )
                .await
            {
                LoopAction::Continue => {}
                LoopAction::Break(r) => {
                    loop_result = r;
                    break;
                }
            }
            // Replay-exact audit: one event per completed LLM call of the
            // turn (usage + finish reason, never message contents).
            crate::observability::event_log::record(
                app,
                session_id,
                Some(&turn_id),
                "model_call",
                serde_json::json!({
                    "model": effective_model,
                    "provider": chat_state.provider,
                    "finish_reason": state.finish_reason,
                    "usage": {
                        "prompt": state.usage.prompt_tokens,
                        "completion": state.usage.completion_tokens,
                        "cache_hit": state.usage.prompt_cache_hit_tokens,
                        "cache_miss": state.usage.prompt_cache_miss_tokens,
                        "reasoning": state.usage.reasoning_tokens,
                    },
                }),
            );
            if state.accumulated_tool_calls.is_empty() {
                match self
                    .phase_no_tool_path(
                        app,
                        session_id,
                        turn_index,
                        debug_mode,
                        user_message,
                        chat_state,
                        &reminder_config,
                        cancellation_token,
                        &turn_id,
                        &mut state,
                    )
                    .await
                {
                    LoopAction::Continue => {}
                    LoopAction::Break(r) => {
                        loop_result = r;
                        break;
                    }
                }
            } else {
                match self
                    .phase_tool_execution(
                        app,
                        session_id,
                        &turn_id,
                        debug_mode,
                        chat_state,
                        cancellation_token,
                        skill_engine,
                        run_started_at_ms,
                        &mut state,
                    )
                    .await
                {
                    LoopAction::Continue => {}
                    LoopAction::Break(r) => {
                        loop_result = r;
                        break;
                    }
                }
            }

            // ── AgentLoopTurnEnd hook ───────────────────────────────────
            // Fires when an iteration completed and the loop continues.
            // The final iteration is covered by AgentLoopEnd (housekeeping),
            // so no exit path misses a terminal signal.
            let turn_end_ctx = HookContext::new(HookEvent::AgentLoopTurnEnd, session_id)
                .with_data("iteration", serde_json::json!(state.budget.current_turn()));
            self.hook_executor.execute_observe(&turn_end_ctx).await;
        }

        // Replay event: which budget limit tripped at the end of this turn
        // (turns/tokens/cost/wall-clock) — the "why did the agent stop" signal
        // that complements the model_call/edit trail.
        if let Some(reason) = state.budget.exceeded_reason() {
            crate::observability::event_log::record(
                app,
                session_id,
                Some(&turn_id),
                "budget_trip",
                serde_json::json!({ "reason": reason }),
            );
        }

        // Shared exit housekeeping: AgentLoopEnd hook + file-state snapshot —
        // runs on EVERY exit path (normal stop, cancellation, stream error,
        // budget final-answer) so the rewind snapshot and the AgentLoopEnd
        // hook are never skipped by an early return.
        self.finish_loop_housekeeping(app, session_id, turn_index, debug_mode, &file_state_tracker)
            .await;

        loop_result
    }
}
