//! Stop-path gate chain — todo / narration / background / verify /
//! plan-checklist nudges, stop hooks, and the independent evaluator gates.

use super::super::gates::{
    edited_only_documents, is_explicit_completion, is_narration_without_action,
};
use super::super::verification::{build_verification_failure_guidance, VerificationTier};
use super::state::{StopGateCounters, StopGateDecision};
use super::AgentLoop;
use crate::agent::budget::BudgetTracker;
use crate::agent::chat_state::ChatState;
use crate::agent::system_reminder::{ReminderConfig, ReminderState};
use crate::core::stream::emit_stream;
use crate::core::types::TurnPhase;
use crate::core::types::{emit_debug_trace, DebugEvent, StreamEvent};
use crate::hooks::{HookContext, HookEvent};
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::super::{evaluator, AgentLoopMode};

/// Emit the "turn is being held for verification/discipline" signal — the
/// already-streamed summary is NOT terminal; the frontend shows a
/// verifying/continuing phase instead of letting the user read it as done.
fn emit_verifying(
    app: &AppHandle,
    turn_id: &str,
    session_id: &str,
    reason: &str,
    trace_id: Option<String>,
) {
    // Replay event: which stop gate held the turn open — the "why did the
    // loop keep going" signal that makes convergence diagnosable.
    crate::observability::event_log::record(
        app,
        session_id,
        Some(turn_id),
        "gate_fired",
        serde_json::json!({ "reason": reason }),
    );
    emit_stream(
        app,
        StreamEvent::TurnStatus {
            turn_id: turn_id.to_string(),
            session_id: session_id.to_string(),
            phase: TurnPhase::Verifying,
            reason: reason.to_string(),
            trace_id,
        },
    );
}

/// Whether the model's final text is waiting on a user decision — the turn
/// ended with a question or an explicit choice request ("告诉我修哪些",
/// "要我继续吗", "你希望怎么处理", "Tell me which ones to fix").
///
/// Stop-time "keep going" nudges (TodoGate, NarrationGate) must skip when
/// this fires: the model has already yielded to the user, and pushing it
/// to continue is exactly what made it self-restart after asking for a
/// decision ("agent 又直接去干活了").
pub(super) fn is_waiting_for_user_decision(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || is_explicit_completion(text) {
        return false;
    }
    // Only the tail decides — an old question in the middle of a long
    // report must not veto the gates.
    let tail: String = {
        let reversed: String = text.chars().rev().take(120).collect();
        reversed.chars().rev().collect()
    };
    let tail = tail.trim();
    let lower = tail.to_lowercase();

    // Ends with a question mark (Chinese or English) → awaiting an answer.
    if tail.ends_with('?') || tail.ends_with('？') {
        return true;
    }

    const ZH: &[&str] = &[
        "告诉我",
        "你希望",
        "你来决定",
        "由你决定",
        "你决定",
        "等你确认",
        "待你确认",
        "请确认",
        "你确认",
        "确认后",
        "请选择",
        "你来选",
        "你选",
        "二选一",
        "怎么处理",
        "如何处理",
        "怎么办",
        "怎么弄",
        "怎么安排",
        "先做哪个",
        "先修哪个",
        "先改哪个",
        "哪个优先",
        "先处理哪个",
        "继续就说",
        "说一声",
        "回我",
        "要不要",
        "是否要",
        "需不需要",
    ];
    const EN: &[&str] = &[
        "let me know",
        "tell me which",
        "tell me what",
        "which one",
        "which approach",
        "your call",
        "want me to",
        "should i",
        "shall i",
        "do you want",
    ];
    ZH.iter().any(|m| tail.contains(m)) || EN.iter().any(|m| lower.contains(m))
}

impl AgentLoop {
    /// The stop-path gate chain (Phase 5c–5e''): todo / narration / background /
    /// verify / plan-checklist nudges, stop hooks, and the independent evaluator
    /// gates (EvaluatorQa / default acceptance / Goal). Returns `Continue` when a
    /// nudge forced another model turn, or `Stop` when the turn may end normally
    /// (the caller emits TurnEnd with the true finish reason).
    ///
    /// Extracted from `run_inner` so the loop body stays lean and the gate chain's
    /// state is explicit: every counter it touches lives in `StopGateCounters`, and
    /// the shared budget / reminder / verification evidence are passed in.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn run_stop_gates(
        &self,
        app: &AppHandle,
        session_id: &str,
        turn_id: &str,
        turn_index: usize,
        debug_mode: bool,
        user_message: &str,
        chat_state: &mut ChatState,
        budget: &BudgetTracker,
        reminder_state: &mut ReminderState,
        reminder_config: &ReminderConfig,
        counters: &mut StopGateCounters,
        cancellation_token: &CancellationToken,
        edited_files: &[std::path::PathBuf],
        bash_wrote_files: bool,
        verification_tier: VerificationTier,
        verification_failed: bool,
        run_has_tool_activity: bool,
        intent_result: &crate::agent::intent::IntentResult,
        accumulated_text: &str,
    ) -> StopGateDecision {
        // Gate-tuning constants stay local to the chain that consumes them.
        const TODO_GATE_MAX_FIRES: u32 = 2;
        const STOP_NUDGE_BUDGET: u32 = 3;
        const MAX_STOP_HOOK_FIRES: u32 = 3;
        const MAX_GOAL_CHECKS: u32 = 3;

        // ── HARD STOP — completion statement is the brake ────────────
        // An explicit completion statement ("已完成，无剩余步骤") ENDS the
        // turn unconditionally: NO gate (todo / background / verify /
        // evaluator / plan / stop hook) may pull the model back for "one
        // more check". The 2026-08-07 "a txt file never ends" loop was
        // exactly this: the model delivered its summary, gates re-dragged
        // it for command verification, and it re-summarized 5+ times. The
        // user's product decision: the summary IS the brake — once output,
        // no further tool call may be initiated this turn.
        //
        // TWO narrow exceptions — a completion statement is NOT a valid
        // brake when it would paper over unverified code work:
        // 1. This run PRODUCED failed verification evidence (a check/command
        //    returned non-zero, or LSP reported errors). Declaring "done"
        //    over known-broken work must not skip the verify gate.
        // 2. This run edited CODE files (not documents) and produced NO
        //    verification evidence of any kind — no successful check command,
        //    no auto-LSP Syntax evidence. DeepSeek-class models love this
        //    path: edit, never verify, declare done. The verify gate pulls
        //    ONE bounded fix round (it fires once; the evaluator once).
        //
        // Goal mode keeps its own completion handling below (it must clear
        // the session goal before stopping). EvaluatorQa keeps its review
        // loop: the user explicitly opted into generator-review quality
        // there, so a bare completion statement is not the acceptance seat.
        // A mutating bash command counts as a code edit even though the loop
        // cannot name its target path — the completion brake, the verify
        // gate, and the default acceptance gate must all see it, or a
        // "改完不验证就宣称完成" summary ships unverified code
        // (audit: bash-writes-bypass-brake).
        let code_edits = has_code_edits(edited_files, bash_wrote_files);
        let edited_code_unverified =
            code_edits && !verification_tier.is_at_least(VerificationTier::Syntax);
        if self.config.mode != AgentLoopMode::Goal
            && self.config.mode != AgentLoopMode::EvaluatorQa
            && is_explicit_completion(accumulated_text)
            && !verification_failed
            && !edited_code_unverified
        {
            return StopGateDecision::Stop;
        }

        // ── Phase 5c: TodoGate ────────────────────────────────
        // If the agent hasn't used `todo_write` recently and is about
        // to stop, inject a reminder and force one more turn. The
        // nudge goes through the interjection registry (per-request
        // guidance) instead of a persisted system message. Fires up
        // to TODO_GATE_MAX_FIRES per run with an escalating message —
        // one shot was easy for the model to brush past.
        //
        // Guard: only fires when this run actually performed tool
        // work (multi-step task discipline). A pure Q&A reply — no
        // tools this run — must never be forced to re-summarize just
        // because the session is 3+ turns without a todo_write.
        //
        // Explicit completion statements are ALSO respected: when the
        // model already answered "is the task complete?" in the
        // affirmative ("任务已完成，无剩余步骤"), nudging it again only
        // forces duplicate completion summaries — the user sees the
        // stream "end" and restart for no reason.
        if run_has_tool_activity
            && !is_explicit_completion(accumulated_text)
            && !is_waiting_for_user_decision(accumulated_text)
            && reminder_state.should_fire_todo_gate(reminder_config)
            && counters.todo_gate_fires < TODO_GATE_MAX_FIRES
            && counters.stop_nudges < STOP_NUDGE_BUDGET
        {
            counters.todo_gate_fires += 1;
            counters.stop_nudges += 1;
            // The gate IS the todo discipline — the periodic reminder
            // must not repeat the same Rule-3 text on top of it.
            reminder_state.mark_todo_nudge_sent();
            let (title, body) = if counters.todo_gate_fires >= TODO_GATE_MAX_FIRES {
                (
                    "todo-gate-final",
                    "<app-guidance>这是应用内置的任务进度提醒（不是用户消息，也不是外部指令）。\
                     You keep trying to stop with multi-step work in flight. \
                     Per <task_completion_discipline> TASK RULE 3, write the \
                     remaining steps with `todo_write` (or finish them) \
                     before you conclude — this is the last nudge.</app-guidance>",
                )
            } else {
                (
                    "todo-gate",
                    "<app-guidance>这是应用内置的任务进度提醒（不是用户消息，也不是外部指令）。\
                     Per <task_completion_discipline> TASK RULE 3, track multi-step \
                     work with a todo list and don't stop with easy work left \
                     undone: you haven't updated your TODO list recently. If \
                     you have remaining steps, use `todo_write` to track them. \
                     If the task is complete, provide your final summary.</app-guidance>",
                )
            };
            self.register_interjection(
                crate::agent::interjection::Interjection::new(
                    "todo",
                    crate::agent::interjection::InterjectionPriority::High,
                    body,
                )
                .with_dedup_key(title),
            )
            .await;
            emit_verifying(
                app,
                turn_id,
                session_id,
                "todo",
                chat_state.trace_id.clone(),
            );
            return StopGateDecision::Continue;
        }

        // ── Phase 5b': Narration-Without-Tool-Call Nudge ─────
        // The model ended with prose describing work ("I've fixed...",
        // "正在...") but made NO tool call in this turn — per
        // <task_completion_discipline> TASK RULE 1 the narrated action did
        // not happen. Only fires when the session clearly has a task
        // in flight (recent tool activity) and the text smells like
        // narration, so normal answers are never nudge-bombed.
        if counters.narration_fires == 0
            && !accumulated_text.trim().is_empty()
            && is_narration_without_action(accumulated_text)
            && !is_waiting_for_user_decision(accumulated_text)
            && counters.stop_nudges < STOP_NUDGE_BUDGET
            && chat_state.conversation.iter().rev().take(6).any(|item| {
                matches!(
                    item,
                    crate::core::types::ConversationItem::Assistant(a)
                        if !a.tool_calls.is_empty()
                )
            })
        {
            counters.narration_fires += 1;
            counters.stop_nudges += 1;
            self.register_interjection(
                crate::agent::interjection::Interjection::new(
                    "narration",
                    crate::agent::interjection::InterjectionPriority::Normal,
                    "Per <task_completion_discipline> TASK RULE 1, don't narrate \
                     progress in prose without a corresponding tool call — \
                     the action did not happen. Make the next concrete tool \
                     call this turn.",
                )
                .with_dedup_key("narration-no-tool"),
            )
            .await;
            emit_verifying(
                app,
                turn_id,
                session_id,
                "narration",
                chat_state.trace_id.clone(),
            );
            return StopGateDecision::Continue;
        }

        // ── Phase 5c': False-Completion Nudge ────────────────
        // The agent wants to stop while background tasks are STILL
        // running — a classic "declared done too early" signal.
        // Nudge it to either wait for / inspect the tasks or
        // explicitly tell the user they're still in flight.
        if counters.bg_nudge_fires == 0 && counters.stop_nudges < STOP_NUDGE_BUDGET {
            let bg_running = {
                let state = app.state::<crate::bootstrap::AppState>();
                state
                    .background_tasks
                    .list(session_id)
                    .into_iter()
                    .filter(|t| t.is_running())
                    .count()
            };
            if bg_running > 0 {
                counters.bg_nudge_fires += 1;
                counters.stop_nudges += 1;
                self.register_interjection(
                    crate::agent::interjection::Interjection::new(
                        "background",
                        crate::agent::interjection::InterjectionPriority::High,
                        format!(
                            "Per <task_completion_discipline> TASK RULE 4, don't \
                             stop with work left undone: {bg_running} background \
                             task(s) are still running. Do NOT declare the work \
                             complete. Use `wait_tasks` to collect their output, \
                             or explicitly tell the user they are still in flight."
                        ),
                    )
                    .with_dedup_key("bg-running"),
                )
                .await;
                emit_verifying(
                    app,
                    turn_id,
                    session_id,
                    "background",
                    chat_state.trace_id.clone(),
                );
                return StopGateDecision::Continue;
            }
        }

        // ── Phase 5c'': Verification Gate (Tier 1) ────────────
        // The model edited files this run but never produced even Syntax
        // evidence (no successful lsp diagnostics, no successful
        // test/lint/typecheck/build command). Per <validation_discipline>,
        // demand a concrete verification step before the loop stops. A
        // FAILED verification command is treated as "not verified": it gets
        // a failure-specific nudge so the model fixes the failure rather
        // than trusting a non-zero exit. Fires at most once per run.
        //
        // EvaluatorQa and Goal SKIP this gate: their acceptance seat
        // is the full independent evaluator review (Phase 5e / 5e''),
        // which runs its own verification — a prompt-level nudge
        // first would be a redundant full LLM call that adds no
        // information.
        if self.config.mode != AgentLoopMode::EvaluatorQa
            && self.config.mode != AgentLoopMode::Goal
            && counters.verify_gate_fires == 0
            && !verification_tier.is_at_least(VerificationTier::Syntax)
            && code_edits
            && counters.stop_nudges < STOP_NUDGE_BUDGET
        {
            counters.verify_gate_fires += 1;
            counters.stop_nudges += 1;
            let mut files: Vec<String> = edited_files
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            // A mutating bash write has no named path — give the nudge a
            // concrete label instead of an empty file list.
            if files.is_empty() && bash_wrote_files {
                files.push("files written via bash".to_string());
            }
            let (title, body) = if verification_failed {
                (
                    "verify-failed",
                    build_verification_failure_guidance(&files, chat_state),
                )
            } else {
                (
                    "verify-gate",
                    format!(
                        "Per <validation_discipline>, edited files need at \
                         least one static check before you conclude. You \
                         changed: {} — but produced no successful Syntax \
                         evidence (LSP diagnostics via the `lsp` tool, or a \
                         typecheck/lint command like `tsc --noEmit` / \
                         `cargo check` that passed). THIS ROUND: run the \
                         concrete verification step now and report its \
                         output. Do NOT output a new final summary or \
                         explanation — once the verification evidence is \
                         clean the turn ends on its own.",
                        files.join(", ")
                    ),
                )
            };
            self.register_interjection(
                crate::agent::interjection::Interjection::new(
                    "verify",
                    crate::agent::interjection::InterjectionPriority::High,
                    body,
                )
                .with_dedup_key(title),
            )
            .await;
            emit_verifying(
                app,
                turn_id,
                session_id,
                "verify",
                chat_state.trace_id.clone(),
            );
            return StopGateDecision::Continue;
        }

        // ── Phase 5c''': Approved-Plan Checklist Gate ─────────
        // If the session has an approved plan with parsed steps, the
        // model must walk through them before concluding. The steps
        // are consumed once — after the reminder fires, the checklist
        // is cleared so it never nags a finished task.
        if counters.plan_gate_fires == 0
            && counters.stop_nudges < STOP_NUDGE_BUDGET
            && !is_explicit_completion(accumulated_text)
        {
            let steps = {
                let state = app.state::<crate::bootstrap::AppState>();
                state.take_active_plan_steps(session_id).await
            };
            if let Some(steps) = steps {
                if !steps.is_empty() {
                    counters.plan_gate_fires += 1;
                    counters.stop_nudges += 1;
                    let mut body = format!(
                        "Your approved plan has {} steps. Walk through \
                         them one by one before concluding:\n",
                        steps.len()
                    );
                    for (i, s) in steps.iter().enumerate() {
                        body.push_str(&format!("\n{}. {}", i + 1, s.text));
                    }
                    body.push_str(
                        "\n\nFor each step: implement it (file tools), then \
                         mark it done in your todo list. If a step is no \
                         longer needed, say so explicitly in your summary.",
                    );
                    self.register_interjection(
                        crate::agent::interjection::Interjection::new(
                            "plan",
                            crate::agent::interjection::InterjectionPriority::High,
                            body,
                        )
                        .with_dedup_key("plan-checklist"),
                    )
                    .await;
                    emit_verifying(
                        app,
                        turn_id,
                        session_id,
                        "plan",
                        chat_state.trace_id.clone(),
                    );
                    return StopGateDecision::Continue;
                }
            }
        }

        // ── Phase 5d: Stop Hooks ─────────────────────────────
        // Blocking hooks fire when the agent is about to stop. A denied
        // Stop hook injects its reason as a correction prompt and forces
        // one more loop iteration. Guarded so hooks cannot loop forever.
        let stop_ctx = HookContext::new(HookEvent::Stop, session_id)
            .with_data("turn", serde_json::json!(turn_index))
            .with_data("model", serde_json::json!(self.config.mode.as_str()));
        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::hook_trigger(session_id, "Stop"),
        );
        if let Err(reason) = self.hook_executor.execute_stop_hooks(&stop_ctx).await {
            if counters.stop_hook_fires < MAX_STOP_HOOK_FIRES {
                counters.stop_hook_fires += 1;
                warn!(reason = %reason, "Stop hook denied — continuing loop");
                chat_state.push_transient_system(format!(
                    "A Stop hook requested changes before this turn could end:\n{reason}\n\
                     Address the feedback above, then provide your final response."
                ));
                emit_verifying(
                    app,
                    turn_id,
                    session_id,
                    "stop_hook",
                    chat_state.trace_id.clone(),
                );
                return StopGateDecision::Continue;
            }
            warn!(reason = %reason, "Stop hook denied — max retries reached, ending turn");
        }

        // ── Phase 5e: Evaluator-QA gate ─────────────────────────
        // In EvaluatorQa mode, the generator is not allowed to stop
        // until an INDEPENDENT evaluator subagent passes the work
        // (or the fix-round cap is hit). The evaluator runs in an
        // isolated context with verification-only tools; a FAIL
        // verdict injects the findings and forces another generator
        // round. Only fires when this turn actually touched files —
        // a pure Q&A reply has nothing to review.
        if self.config.mode == AgentLoopMode::EvaluatorQa
            && counters.evaluator_rounds < evaluator::MAX_EVALUATOR_ROUNDS
            && run_has_tool_activity
        {
            let edited_paths: Vec<String> = chat_state.agent_edited_paths.iter().cloned().collect();
            let task = chat_state
                .prompt_texts
                .last()
                .cloned()
                .unwrap_or_else(|| user_message.to_string());
            let work_mode = self.context_builder.work_mode();
            // Box::pin breaks the compiler-visible recursive future:
            // run_inner → review → spawn_subagent_with_cancel → run →
            // run_inner. At RUNTIME the evaluator subagent runs in
            // Standard mode, so Phase 5e never re-triggers inside it.
            match Box::pin(evaluator::run_evaluator_review(
                app,
                session_id,
                &task,
                &edited_paths,
                work_mode,
                self.config.agent_deny_rules.clone(),
                cancellation_token,
                intent_result.acceptance_hint.as_deref(),
            ))
            .await
            {
                Ok(evaluator::EvaluatorVerdict::Pass) => {
                    info!(session_id, "Evaluator gate passed — ending turn");
                }
                Ok(evaluator::EvaluatorVerdict::Fail { findings }) => {
                    counters.evaluator_rounds += 1;
                    info!(
                        session_id,
                        counters.evaluator_rounds,
                        "Evaluator gate failed — forcing another generator round"
                    );
                    // Findings enter the generator's context as a
                    // transient system message (never persisted) so
                    // the model must respond to them this turn.
                    chat_state.push_transient_system(format!(
                        "<evaluator-review>\nAn independent evaluator reviewed your \
                         work and it did NOT pass. Fix EVERY finding below with \
                         concrete code changes (verify with tests/build). Do not \
                         argue or repeat yourself without evidence.\n\n{findings}\n\
                         </evaluator-review>"
                    ));
                    emit_verifying(
                        app,
                        turn_id,
                        session_id,
                        "evaluator",
                        chat_state.trace_id.clone(),
                    );
                    return StopGateDecision::Continue;
                }
                Err(e) => {
                    // Review failure (spawn/LLM error) must not hang
                    // the turn — log and end normally.
                    warn!(session_id, error = %e, "Evaluator review failed — ending turn");
                }
            }
        }

        // ── Phase 5e': Default Acceptance Gate (Tier 3) ────────
        // In every other mode (Standard / PlanExecute / Reflexion /
        // Coordinator) a turn that edited files but produced NO Tests-level
        // evidence (passing test/build/verify pipeline) gets ONE
        // independent evaluator review before it may end. Tier 1 Syntax
        // evidence satisfies the VerifyGate but NOT this gate: "it
        // type-checks" is not "the work is correct". The evaluator subagent
        // (isolated context, verification-only tools) is the same
        // acceptance seat as EvaluatorQa — capped at TWO reviews per turn
        // to bound the cost: the initial review, plus ONE re-review of the
        // fix round it forces, so a "fix but don't verify" follow-up cannot
        // slip through unverified (audit: eval-fix-round-unverified). Clean
        // Tests evidence (verification_tier ≥ Tests, no failure) skips it.
        // Goal mode has its own gate below (Phase 5e'') and must not
        // double-review here.
        if self.config.mode != AgentLoopMode::EvaluatorQa
            && self.config.mode != AgentLoopMode::Goal
            && counters.evaluator_rounds < 2
            && code_edits
            && (!verification_tier.is_at_least(VerificationTier::Tests) || verification_failed)
        {
            // Review THIS run's edits only — the session-level
            // agent_edited_paths accumulates across turns, so a turn
            // that changed one file would otherwise be reviewed
            // against every change the session ever made.
            let edited_paths: Vec<String> = edited_files
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            let task = chat_state
                .prompt_texts
                .last()
                .cloned()
                .unwrap_or_else(|| user_message.to_string());
            let work_mode = self.context_builder.work_mode();
            match Box::pin(evaluator::run_evaluator_review(
                app,
                session_id,
                &task,
                &edited_paths,
                work_mode,
                self.config.agent_deny_rules.clone(),
                cancellation_token,
                intent_result.acceptance_hint.as_deref(),
            ))
            .await
            {
                Ok(evaluator::EvaluatorVerdict::Pass) => {
                    info!(session_id, "Default acceptance gate passed — ending turn");
                }
                Ok(evaluator::EvaluatorVerdict::Fail { findings }) => {
                    counters.evaluator_rounds += 1;
                    info!(
                        session_id,
                        "Default acceptance gate failed — forcing one fix round"
                    );
                    chat_state.push_transient_system(format!(
                        "<evaluator-review>\nAn independent evaluator reviewed your \
                         work because you ended the turn without verified changes, \
                         and it did NOT pass. Fix EVERY finding below with concrete \
                         code changes, then VERIFY them (tests/lint/typecheck/LSP \
                         diagnostics). Do not argue or repeat yourself without \
                         evidence.\n\n{findings}\n</evaluator-review>"
                    ));
                    emit_verifying(
                        app,
                        turn_id,
                        session_id,
                        "evaluator",
                        chat_state.trace_id.clone(),
                    );
                    return StopGateDecision::Continue;
                }
                Err(e) => {
                    warn!(
                        session_id,
                        error = %e,
                        "Default acceptance review failed — ending turn"
                    );
                }
            }
        }

        // ── Phase 5e'': Goal-Achievement Gate ─────────────────
        // In Goal mode, the generator may only stop when an
        // INDEPENDENT evaluator confirms the session goal is achieved
        // (or the check cap / budget is hit). The evaluator is the
        // same acceptance seat as EvaluatorQa — isolated context,
        // verification-only tools — but the review criterion is the
        // session goal (<current-goal>), the user's definition-of-
        // done, rather than the last prompt. Unlike EvaluatorQa this
        // runs even for pure Q&A turns: the user explicitly asked for
        // goal-verified completion.
        if self.config.mode == AgentLoopMode::Goal
            && counters.goal_checks < MAX_GOAL_CHECKS
            && budget.should_continue()
        {
            let goal = {
                let state = app.state::<crate::bootstrap::AppState>();
                state
                    .goal_store
                    .get(session_id)
                    .filter(|g| !g.trim().is_empty())
            };
            // Explicit completion declaration ends the Goal turn: the
            // generator has already stated the goal is answered/done.
            // Re-reviewing only forces duplicate work — and a STALE goal
            // (e.g. an old question already answered, like this session's
            // "为什么出错啊") would otherwise push endless rounds. The goal
            // is cleared so later messages are not haunted by it.
            //
            // Same two brake exceptions as the Standard-mode hard stop: a
            // completion statement must NOT paper over unverified code work
            // (a failed verification, or edited code with no Syntax-tier
            // evidence). Those fall through to the goal evaluator below,
            // which reviews the actual state instead of trusting the claim.
            if is_explicit_completion(accumulated_text)
                && !verification_failed
                && !edited_code_unverified
            {
                if goal.is_some() {
                    let state = app.state::<crate::bootstrap::AppState>();
                    state.goal_store.set(session_id, String::new());
                    let _ = app.emit(
                        "goal-updated",
                        crate::tools::builtin::update_goal::GoalEvent {
                            session_id: session_id.to_string(),
                            goal: None,
                        },
                    );
                    info!(session_id, "Goal mode: explicit completion — clearing goal");
                }
                StopGateDecision::Stop
            } else {
                let task = goal.clone().unwrap_or_else(|| {
                    chat_state
                        .prompt_texts
                        .last()
                        .cloned()
                        .unwrap_or_else(|| user_message.to_string())
                });
                let edited_paths: Vec<String> = edited_files
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                let work_mode = self.context_builder.work_mode();
                match Box::pin(evaluator::run_evaluator_review(
                    app,
                    session_id,
                    &task,
                    &edited_paths,
                    work_mode,
                    self.config.agent_deny_rules.clone(),
                    cancellation_token,
                    None,
                ))
                .await
                {
                    Ok(evaluator::EvaluatorVerdict::Pass) => {
                        info!(session_id, "Goal gate passed — goal achieved");
                        StopGateDecision::Stop
                    }
                    Ok(evaluator::EvaluatorVerdict::Fail { findings }) => {
                        counters.goal_checks += 1;
                        info!(
                            session_id,
                            counters.goal_checks,
                            "Goal gate failed — forcing another generator round"
                        );
                        if !budget.should_continue() {
                            info!("Budget exhausted after goal FAIL — ending turn");
                            StopGateDecision::Stop
                        } else {
                            let goal_hint = goal
                                .map(|g| format!("\n\nSession goal: {g}"))
                                .unwrap_or_default();
                            chat_state.push_transient_system(format!(
                                "<goal-review>\nAn independent evaluator checked whether \
                                 your session goal is achieved and it is NOT.{goal_hint}\n\n\
                                 Fix EVERY finding below with concrete work, then verify \
                                 (tests/lint/typecheck/LSP diagnostics). Do not argue or \
                                 repeat yourself without evidence.\n\n{findings}\n\
                                 </goal-review>"
                            ));
                            emit_verifying(
                                app,
                                turn_id,
                                session_id,
                                "goal",
                                chat_state.trace_id.clone(),
                            );
                            StopGateDecision::Continue
                        }
                    }
                    Err(e) => {
                        warn!(session_id, error = %e, "Goal review failed — ending turn");
                        StopGateDecision::Stop
                    }
                }
            }
        } else {
            StopGateDecision::Stop
        }
    }
}

/// Whether CODE files (not documents) were edited this run — or a mutating
/// bash command wrote files the loop cannot attribute to a path. Both are
/// the "edit without a named path" signal that must defeat the completion
/// brake and arm the verify/acceptance gates. Document-only edits and empty
/// edit sets with no bash write return `false` (the brake stays intact).
fn has_code_edits(edited_files: &[std::path::PathBuf], bash_wrote_files: bool) -> bool {
    (!edited_files.is_empty() && !edited_only_documents(edited_files)) || bash_wrote_files
}

#[cfg(test)]
mod tests {
    use super::is_waiting_for_user_decision;

    #[test]
    fn question_ending_is_waiting_for_decision() {
        assert!(is_waiting_for_user_decision(
            "发现了 13 个问题，告诉我修哪些"
        ));
        assert!(is_waiting_for_user_decision("要我继续吗？"));
        assert!(is_waiting_for_user_decision("要不要我把它们一起修了？"));
        assert!(is_waiting_for_user_decision("你希望怎么处理？"));
        assert!(is_waiting_for_user_decision("先修哪个比较好？"));
        assert!(is_waiting_for_user_decision("等你确认后我再继续"));
        assert!(is_waiting_for_user_decision("Tell me which ones to fix."));
        assert!(is_waiting_for_user_decision("Want me to keep going?"));
    }

    #[test]
    fn completed_or_statement_text_is_not_waiting() {
        assert!(!is_waiting_for_user_decision("任务已完成，无剩余步骤"));
        assert!(!is_waiting_for_user_decision("我修好了这个问题。"));
        assert!(!is_waiting_for_user_decision(""));
        // An old question in the middle of a long report does not count.
        let mut s = "甲".repeat(200);
        s.push_str("中间问过一个问题？");
        s.push_str(&"乙".repeat(200));
        s.push_str("这是最终结论。");
        assert!(!is_waiting_for_user_decision(&s));
    }

    #[test]
    fn code_edits_or_bash_write_break_the_brake() {
        use super::has_code_edits;
        let code = vec![std::path::PathBuf::from("src/lib.rs")];
        // Code edited → counts as unverified code work.
        assert!(has_code_edits(&code, false));
        // A mutating bash write with no named path still counts.
        assert!(has_code_edits(&[], true));
        // Document-only edit → brake intact (no command ceremony for docs).
        let doc = vec![std::path::PathBuf::from("README.md")];
        assert!(!has_code_edits(&doc, false));
        // Empty edit set + no bash write → brake intact.
        assert!(!has_code_edits(&[], false));
    }
}
