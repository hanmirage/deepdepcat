//! Tool-round phases — tool execution with post-round guards, reflexion,
//! and the no-tool path (budget / empty-response / commit / stop gates).

use super::super::gates::{
    code_file_extensions, is_in_workspace, is_non_code_document, narration_similarity,
    suggest_check_command, EXPLORATION_ROUND_LIMIT, MAX_EXPLORATION_NUDGES, REPETITION_SIMILARITY,
    REPETITION_TURNS, TOOL_NAME_FAILURE_LIMIT,
};
use super::super::verification::{
    apply_verification_outcome, verification_outcome, VerificationTier,
};
use super::super::AgentLoopMode;
use super::state::{LoopAction, LoopState, StopGateDecision};
use super::AgentLoop;
use crate::agent::chat_state::ChatState;
use crate::agent::system_reminder::ReminderConfig;
use crate::core::stream::emit_stream;
use crate::core::types::{AgentStatus, ConversationItem, StreamEvent, TurnOutcome};
use crate::skills::activation::SkillActivationEngine;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

impl AgentLoop {
    /// Phase 5 — execute the streamed tool calls, then run the post-round
    /// guard family (verification tracking, goal drift, skeleton hardening,
    /// strategy switch, cross-turn repetition, todo front/sync nudges) and
    /// the shallow reflexion critique. Returns Continue to loop, or Break
    /// on a terminal condition (denial cap / budget exhaustion / error).
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn phase_tool_execution(
        &self,
        app: &AppHandle,
        session_id: &str,
        turn_id: &str,
        debug_mode: bool,
        chat_state: &mut ChatState,
        cancellation_token: &CancellationToken,
        skill_engine: Option<&SkillActivationEngine>,
        run_started_at_ms: u64,
        state: &mut LoopState,
    ) -> LoopAction {
        let LoopState {
            ref mut budget,
            ref mut run_has_tool_activity,
            ref mut consecutive_denials,
            ref mut edited_files,
            ref mut bash_wrote_files,
            ref mut verification_tier,
            ref mut verification_failed,
            ref mut executed_results,
            ref mut executed_results_scanned_len,
            ref mut goal_drift_count,
            ref plan_phase_active,
            ref mut exploration_rounds,
            ref mut exploration_nudges,
            ref mut todo_plan_nudged,
            ref mut todo_sync_nudged,
            ref mut todo_order_nudged,
            ref mut last_narration_text,
            ref mut repeat_narration_streak,
            ref mut reflexion_rounds,
            ref mut code_verify_nudged,
            ref mut reminder_state,
            ref intent_result,
            ref accumulated_text,
            ref accumulated_reasoning,
            ref accumulated_tool_calls,
            ref usage,
            ref mut augmented_message,
            ..
        } = state;
        let plan_phase_active = *plan_phase_active;
        let reasoning_opt = {
            let cleaned = crate::core::str_util::strip_tool_call_markup(accumulated_reasoning);
            if cleaned.is_empty() {
                None
            } else {
                Some(cleaned)
            }
        };

        // Reset empty response counter — we got non-empty content or tools.
        if !accumulated_text.is_empty() || !accumulated_tool_calls.is_empty() {
            chat_state.empty_response_count = 0;
        }

        let _ = app.emit("agent-status-changed", AgentStatus::ToolRunning);
        *run_has_tool_activity = true;

        let clean_text = crate::core::str_util::strip_tool_call_markup(accumulated_text);
        chat_state.push_assistant_message(
            clean_text,
            accumulated_tool_calls.to_vec(),
            Some(usage.clone()),
            reasoning_opt,
        );

        let deny_count = match self
            .execute_tool_batch(
                app,
                session_id,
                turn_id,
                budget.current_turn(),
                chat_state,
                accumulated_tool_calls,
                cancellation_token,
                debug_mode,
                skill_engine,
            )
            .await
        {
            Ok(count) => count,
            Err(e) => {
                // TurnEnd so the frontend stream finalizes on a hard tool
                // failure — the turn dies here, not through the stop path.
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

        if deny_count > 0 {
            let in_plan_phase = {
                let state = app.state::<crate::bootstrap::AppState>();
                state.session_mode(session_id).await.is_read_only()
            };
            if !in_plan_phase {
                *consecutive_denials += deny_count;
                if *consecutive_denials >= self.config.max_consecutive_denials {
                    warn!(
                        consecutive_denials = *consecutive_denials,
                        max = self.config.max_consecutive_denials,
                        "Max consecutive permission denials — terminating loop"
                    );
                    let _ = app.emit("agent-status-changed", AgentStatus::Error);
                    emit_stream(
                        app,
                        StreamEvent::Error {
                            turn_id: turn_id.to_string(),
                            session_id: session_id.to_string(),
                            message: format!(
                                "Agent loop terminated: {} consecutive permission denials",
                                *consecutive_denials
                            ),
                            trace_id: chat_state.trace_id.clone(),
                        },
                    );
                    emit_stream(
                        app,
                        StreamEvent::TurnEnd {
                            turn_id: turn_id.to_string(),
                            session_id: session_id.to_string(),
                            reason: "stop".to_string(),
                            status: TurnOutcome::Denied,
                            trace_id: chat_state.trace_id.clone(),
                        },
                    );
                    return LoopAction::Break(Ok(turn_id.to_string()));
                }
            }
        } else {
            *consecutive_denials = 0;
        }

        // ── Phase 5.4: Verification Tracking ───────────────────────────
        // Mid-run compaction (`replace_conversation`) SHRINKS the conversation
        // array, which leaves the absolute scanned index pointing past the new
        // end. Reset it so the current round's tool results are scanned into
        // `executed_results` — a turn that compacts and then edits files must
        // still reach the verify gate (a stale index keeps the guard from ever
        // firing, so post-compaction edits are invisible to the gate).
        let convo_len = chat_state.conversation.len();
        if *executed_results_scanned_len > convo_len {
            *executed_results_scanned_len = 0;
            executed_results.clear();
        }
        if convo_len > *executed_results_scanned_len {
            for item in &chat_state.conversation[*executed_results_scanned_len..convo_len] {
                if let ConversationItem::ToolResult(tr) = item {
                    executed_results
                        .insert(tr.tool_call_id.clone(), (tr.is_error, tr.content.clone()));
                }
            }
            *executed_results_scanned_len = convo_len;
        }

        for tc in accumulated_tool_calls {
            match tc.name.as_str() {
                "edit_file" | "write_file" | "search_replace" | "apply_patch" => {
                    if let Some((false, _)) = executed_results.get(&tc.id) {
                        if let Ok(args) = tc.parse_arguments() {
                            if let Some(p) = args.get("path").and_then(|v| v.as_str()) {
                                let workspace = self.context_builder.workspace();
                                let resolved =
                                    crate::tools::builtin::resolve_path(workspace.as_deref(), p);
                                // Only WORKSPACE files count as "edited deliverables"
                                // for the verification gate. Scratch files the agent
                                // writes outside the workspace (a temp apply_opt.ps1
                                // helper or frag_*.html in %TEMP%) are throwaway, not
                                // the deliverable — counting them re-arms the code-verify
                                // gate for an otherwise document-only task (an HTML site
                                // whose only "code" was a temp helper script forced a
                                // pointless lsp call). Non-workspace files are still
                                // event-logged below; they just don't drive the gate.
                                let in_workspace =
                                    is_in_workspace(workspace.as_deref(), &resolved);
                                if in_workspace && !edited_files.contains(&resolved) {
                                    edited_files.push(resolved);
                                }
                                crate::observability::event_log::record(
                                    app,
                                    session_id,
                                    Some(turn_id),
                                    "edit",
                                    serde_json::json!({
                                        "tool": &tc.name,
                                        "call_id": &tc.id,
                                        "path": p,
                                        "ok": true,
                                    }),
                                );
                            }
                        }
                    }
                }
                "lsp" | "bash" => {
                    let command = if tc.name == "bash" {
                        tc.parse_arguments().ok().and_then(|args| {
                            args.get("command")
                                .and_then(|v| v.as_str())
                                .map(str::to_string)
                        })
                    } else {
                        None
                    };
                    // The lsp tool's `operation` decides whether it checked the
                    // code (diagnostics) or merely looked something up (hover /
                    // symbols / definition / format / …). Only diagnostics is
                    // verification evidence.
                    let lsp_operation = if tc.name == "lsp" {
                        tc.parse_arguments().ok().and_then(|args| {
                            args.get("operation")
                                .and_then(|v| v.as_str())
                                .map(str::to_string)
                        })
                    } else {
                        None
                    };
                    apply_verification_outcome(
                        verification_outcome(
                            &tc.name,
                            command.as_deref(),
                            lsp_operation.as_deref(),
                            executed_results.get(&tc.id).cloned(),
                        ),
                        verification_tier,
                        verification_failed,
                    );
                    // A successful MUTATING bash command (sed/cat >/tee/…)
                    // is a file write the loop cannot attribute to a path.
                    // It must still defeat the completion brake and arm the
                    // verify/acceptance gates — the same "edit without
                    // verification" signal as a structured write tool, just
                    // without a concrete path (audit: bash-writes-bypass-brake).
                    if tc.name == "bash" {
                        if let Some(cmd) = command.as_deref() {
                            let wrote = executed_results
                                .get(&tc.id)
                                .is_some_and(|(is_err, _)| !is_err);
                            if wrote {
                                let state = app.state::<crate::bootstrap::AppState>();
                                if !state.permissions.is_read_only_bash(cmd) {
                                    *bash_wrote_files = true;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if self.config.mode == AgentLoopMode::Coordinator {
            let worker_edits = {
                let state = app.state::<crate::bootstrap::AppState>();
                state
                    .coordinator
                    .worker_state()
                    .edited_files_since(session_id, run_started_at_ms)
                    .await
            };
            for f in &worker_edits {
                let resolved = crate::tools::builtin::resolve_path(
                    self.context_builder.workspace().as_deref(),
                    f,
                );
                if !edited_files.contains(&resolved) {
                    edited_files.push(resolved);
                }
                chat_state.record_edited_path(f.clone());
            }
            if !worker_edits.is_empty() {
                info!(
                    session_id,
                    worker_edits = worker_edits.len(),
                    "Coordinator: worker edits merged into run verification evidence"
                );
            }
        }

        if !chat_state.auto_diagnostics.is_empty() {
            let external = {
                let state = app.state::<crate::bootstrap::AppState>();
                let mut guard = state
                    .external_changes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                std::mem::take(&mut *guard)
            };
            if !external.is_empty() {
                chat_state.auto_diagnostics.retain(|path, _| {
                    !external.iter().any(|ext| {
                        ext.to_string_lossy() == path.as_str()
                            || path.starts_with(&ext.to_string_lossy().to_string())
                    })
                });
            }
        }

        if !verification_tier.is_at_least(VerificationTier::Syntax) && !edited_files.is_empty() {
            let checked: Vec<bool> = edited_files
                .iter()
                .filter_map(|p| chat_state.auto_diagnostics.get(&p.display().to_string()))
                .copied()
                .collect();
            if checked.len() == edited_files.len() {
                if checked.iter().any(|clean| !clean) {
                    *verification_failed = true;
                } else if !*verification_failed {
                    // Auto-LSP diagnostics are static evidence — Tier 1.
                    *verification_tier = VerificationTier::Syntax;
                }
            }
        }

        // ── Phase 5.4': Early Code-Verify Nudge ─────────────────────
        // The model edited code files this round but none got static
        // verification evidence. Auto-LSP diagnostics only run when an LSP
        // server is ALREADY up — most projects have none, so the model must
        // voluntarily run the typecheck. DeepSeek-class models tend to skip
        // it and claim success. Nudge EARLY (per edit round, once per run)
        // with a concrete command hint; the stop-time verify gate remains the
        // backstop if the nudge is ignored.
        if !*code_verify_nudged && !edited_files.is_empty() {
            let uncovered: Vec<_> = edited_files
                .iter()
                .filter(|p| {
                    !is_non_code_document(p)
                        && !chat_state
                            .auto_diagnostics
                            .contains_key(&p.display().to_string())
                })
                .cloned()
                .collect();
            if !uncovered.is_empty() {
                *code_verify_nudged = true;
                let files: Vec<String> = uncovered
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                let guidance = match suggest_check_command(&code_file_extensions(&uncovered)) {
                    Some(cmd) => format!(
                        "<app-guidance>这是应用内置的验证提醒（不是用户消息）。你刚改了代码文件：{}。\
                         这些文件还没有静态验证证据（未运行类型检查/构建命令）。先跑 `{cmd}` \
                         验证你的改动，把报错修掉再继续——不要在没有验证的情况下声称完成。</app-guidance>",
                        files.join(", ")
                    ),
                    None => format!(
                        "<app-guidance>这是应用内置的验证提醒（不是用户消息）。你刚改了代码文件：{}。\
                         这些文件还没有静态验证证据（未运行类型检查/构建命令）。先跑项目的类型检查/\
                         构建命令（如 cargo check / tsc --noEmit / go build ./...，按项目实际工具选）\
                         验证改动，把报错修掉再继续——不要在没有验证的情况下声称完成。</app-guidance>",
                        files.join(", ")
                    ),
                };
                self.register_interjection(
                    crate::agent::interjection::Interjection::new(
                        "code-verify",
                        crate::agent::interjection::InterjectionPriority::High,
                        guidance,
                    )
                    .with_dedup_key("code-verify-nudge"),
                )
                .await;
            }
        }

        // ── Phase 5.4: Goal-Drift Check ─────────────────────────────
        if intent_result.intent.is_code_work() {
            let has_file_activity = accumulated_tool_calls.iter().any(|tc| {
                matches!(
                    tc.name.as_str(),
                    "write_file" | "edit_file" | "apply_patch" | "search_replace"
                )
            });
            if has_file_activity {
                *goal_drift_count = 0;
            } else {
                *goal_drift_count += 1;
                if *goal_drift_count >= 3 {
                    *goal_drift_count = 0;
                    let goal = {
                        let state = app.state::<crate::bootstrap::AppState>();
                        state.goal_store.get(session_id)
                    };
                    if let Some(ref g) = goal {
                        self.register_interjection(
                            crate::agent::interjection::Interjection::new(
                                "goal",
                                crate::agent::interjection::InterjectionPriority::High,
                                format!(
                                    "Per <task_completion_discipline> TASK RULE 3, stay on target. \
                                     The session goal is: \"{g}\". You have not touched any \
                                     files in several turns — if you are done, conclude; \
                                     otherwise keep working toward the goal with file tools.",
                                ),
                            )
                            .with_dedup_key(format!("goal-drift:{}", session_id)),
                        )
                        .await;
                    }
                }
            }
        }

        // ── Phase 5.4': Skeleton Hardening ─────────────────────────────
        if !accumulated_tool_calls.is_empty() && !plan_phase_active {
            let bash_succeeded = accumulated_tool_calls.iter().any(|tc| {
                tc.name == "bash"
                    && executed_results
                        .get(&tc.id)
                        .is_some_and(|(is_error, _)| !is_error)
            });
            let has_progress = bash_succeeded
                || accumulated_tool_calls.iter().any(|tc| {
                    matches!(
                        tc.name.as_str(),
                        "write_file"
                            | "edit_file"
                            | "search_replace"
                            | "apply_patch"
                            | "todo_write"
                            | "ask_user"
                            | "use_tool"
                            | "agent"
                    )
                });
            if has_progress {
                *exploration_rounds = 0;
            } else if *exploration_nudges < MAX_EXPLORATION_NUDGES {
                *exploration_rounds += 1;
                if *exploration_rounds >= EXPLORATION_ROUND_LIMIT {
                    *exploration_rounds = 0;
                    *exploration_nudges += 1;
                    self.register_interjection(
                        crate::agent::interjection::Interjection::new(
                            "exploration-budget",
                            crate::agent::interjection::InterjectionPriority::High,
                            "You have spent many turns exploring with read-only tools \
                             but have not made concrete progress (no file change, no \
                             todo update, no user question). CONVERGE NOW: if you have \
                             enough information, take the next concrete action (edit a \
                             file / run a command / ask the user); if you are stuck, \
                             summarize what you found, state your hypothesis, and ask \
                             the user or stop. Do not open more directories or files \
                             without a purpose.",
                        )
                        .with_dedup_key("exploration-budget"),
                    )
                    .await;
                }
            }
        }

        // ── Phase 5.4'': Strategy-Switch Guard ─────────────────────────
        if !chat_state.tool_name_failures.is_empty() {
            let mut overheated: Vec<String> = chat_state
                .tool_name_failures
                .iter()
                .filter(|(_, &f)| f >= TOOL_NAME_FAILURE_LIMIT)
                .map(|(name, _)| name.clone())
                .collect();
            if !overheated.is_empty() {
                // ── Doom-loop user decision ───────────────────────────
                // Repeated tool failures are not just a nudge topic: the
                // user gets to decide whether the turn keeps burning
                // tokens. Only the main loop asks — workers report back to
                // the parent instead of interrupting the user.
                let user_confirmed = if !self.tool_dispatcher.is_subagent() {
                    self.ask_doom_loop_continue(app, session_id, &overheated)
                        .await
                } else {
                    true
                };
                if !user_confirmed {
                    warn!(
                        tools = ?overheated,
                        "User stopped the turn after repeated tool failures"
                    );
                    emit_stream(
                        app,
                        StreamEvent::TurnEnd {
                            turn_id: turn_id.to_string(),
                            session_id: session_id.to_string(),
                            reason: "stop".to_string(),
                            status: TurnOutcome::Done,
                            trace_id: chat_state.trace_id.clone(),
                        },
                    );
                    return LoopAction::Break(Ok(turn_id.to_string()));
                }
                for name in &overheated {
                    chat_state.tool_name_failures.remove(name);
                }
                overheated.sort();
                self.register_interjection(
                    crate::agent::interjection::Interjection::new(
                        "strategy-switch",
                        crate::agent::interjection::InterjectionPriority::High,
                        format!(
                            "The tool(s) {} have failed {} consecutive times with \
                             different arguments.{} STOP retrying this approach — \
                             it cannot succeed in this environment. Switch strategy: \
                             use a different tool, re-read the target to confirm it \
                             exists, or tell the user what is missing (e.g. an \
                             unavailable command) and propose an alternative.",
                            overheated.join(", "),
                            TOOL_NAME_FAILURE_LIMIT,
                            if self.tool_dispatcher.is_subagent() {
                                ""
                            } else {
                                " The user was asked and chose to continue. "
                            },
                        ),
                    )
                    .with_dedup_key(format!("strategy-switch:{}", overheated.join(","))),
                )
                .await;
            }
        }

        // ── Phase 5.4''': Cross-Turn Repetition Guard ──────────────────
        if !accumulated_tool_calls.is_empty() {
            *last_narration_text = None;
            *repeat_narration_streak = 0;
        } else if !accumulated_text.trim().is_empty() {
            let text = accumulated_text.trim();
            if let Some(prev) = last_narration_text {
                if narration_similarity(prev, text) >= REPETITION_SIMILARITY {
                    *repeat_narration_streak += 1;
                    if *repeat_narration_streak >= REPETITION_TURNS {
                        *repeat_narration_streak = 0;
                        self.register_interjection(
                            crate::agent::interjection::Interjection::new(
                                "repetition",
                                crate::agent::interjection::InterjectionPriority::High,
                                "Your narration is repeating what you said in a \
                                 previous turn almost verbatim while nothing changed. \
                                 STOP restating the same plan/status. Either take a \
                                 concrete action (tool call) right now, or give the \
                                 user a final summary and stop. Repeated narration \
                                 without action is not progress.",
                            )
                            .with_dedup_key("cross-turn-repetition"),
                        )
                        .await;
                    }
                } else {
                    *repeat_narration_streak = 0;
                }
            }
            *last_narration_text = Some(text.to_string());
        }

        // ── Todo discipline: FOUR lifecycle points, NOT duplicates ────
        // The todo nudges here (front / sync / order) plus the periodic
        // reminder (system_reminder.rs) and the stop-time TodoGate
        // (stop_gates.rs) read as repeated "TASK RULE 3" nagging, but each
        // fires at a DIFFERENT point in a task's life — merging them would
        // lose the distinct triggers:
        //   pre-work   (this front gate)  — "lay out steps before you write"
        //   during-work (periodic + sync) — "you haven't tracked progress"
        //   content    (order)            — "your depends_on ordering is wrong"
        //   stop-time  (TodoGate)         — "don't stop with untracked work"
        // The periodic → stop-gate pair is a DELIBERATE two-stage escalation
        // (gentle mid-run reminder, firm stop-time demand) coordinated via
        // ReminderState::mark_todo_nudge_sent. Do NOT "unify" these into one
        // gate or a shared fire cap — the triggers are complementary, and
        // collapsing them re-introduces the duplicate-summary / unverified-
        // stop regressions the separate gates prevent.
        // ── Phase 5.4'''': Todo-Plan Front Gate ────────────────────────
        if !*todo_plan_nudged
            && !chat_state.conversation.iter().any(|item| {
                matches!(
                    item,
                    ConversationItem::Assistant(a)
                        if a.tool_calls.iter().any(|tc| tc.name == "todo_write")
                )
            })
            && accumulated_tool_calls.iter().any(|tc| {
                matches!(
                    tc.name.as_str(),
                    "write_file" | "edit_file" | "search_replace" | "apply_patch"
                )
            })
        {
            *todo_plan_nudged = true;
            self.register_interjection(
                crate::agent::interjection::Interjection::new(
                    "todo-plan",
                    crate::agent::interjection::InterjectionPriority::High,
                    "You are about to write files. If this is genuinely multi-step \
                     (several distinct files, or work spanning turns), per \
                     <task_completion_discipline> TASK RULE 3 lay out the remaining \
                     steps with `todo_write` FIRST (one in_progress item at a time), \
                     then execute them. If it is a single-file edit you can finish \
                     now, skip the todo list and just do it — a visible todo panel \
                     on an easy request is noise (TASK RULE 3). Track progress in \
                     the todo list as you go so the task panel reflects reality.",
                )
                .with_dedup_key("todo-plan-front"),
            )
            .await;
        }

        // ── Phase 5.4'''': Todo Sync Nudge ─────────────────────────────
        if !*todo_sync_nudged {
            let sync_needed = {
                let state = app.state::<crate::bootstrap::AppState>();
                state
                    .todo_store
                    .get(session_id)
                    .map(|todos| {
                        let names: Vec<&str> = accumulated_tool_calls
                            .iter()
                            .map(|tc| tc.name.as_str())
                            .collect();
                        crate::tools::builtin::todo_write::todo_sync_needed(&todos, &names)
                    })
                    .unwrap_or(false)
            };
            if sync_needed {
                *todo_sync_nudged = true;
                self.register_interjection(
                    crate::agent::interjection::Interjection::new(
                        "todo-sync",
                        crate::agent::interjection::InterjectionPriority::High,
                        "You did work this round but did not update the todo \
                         list. Per <task_completion_discipline> TASK RULE 3, \
                         call `todo_write` now: mark completed items done and \
                         add anything new you discovered. The todo list is the \
                         user's task panel — keep it accurate.",
                    )
                    .with_dedup_key("todo-sync-nudge"),
                )
                .await;
            }
        }

        // ── Phase 5.4''''': Todo Ordering Nudge ────────────────────────
        // A plan that marks a step in_progress/completed while a step it
        // declared via `depends_on` is still unfinished is out of order —
        // the model is about to build on a foundation it never laid. Nudge
        // ONCE per run to fix the todo before continuing. This is what turns
        // a long task (write a game) from randomly-ordered steps into a
        // dependency-respecting build.
        if !*todo_order_nudged {
            let violation = {
                let state = app.state::<crate::bootstrap::AppState>();
                state
                    .todo_store
                    .get(session_id)
                    .and_then(|todos| crate::tools::builtin::todo_write::todo_order_violation(&todos))
            };
            if let Some(msg) = violation {
                *todo_order_nudged = true;
                self.register_interjection(
                    crate::agent::interjection::Interjection::new(
                        "todo-order",
                        crate::agent::interjection::InterjectionPriority::High,
                        format!(
                            "Your todo list is out of order: {msg}. Call `todo_write` to fix \
                             the ordering — mark the dependency completed first, or move the \
                             dependent step back to pending."
                        ),
                    )
                    .with_dedup_key("todo-order-nudge"),
                )
                .await;
            }
        }

        // ── Phase 5.5: Reflexion Self-Critique ─────────────────────────
        const SHALLOW_REFLEXION_INTERVAL: u32 = 3;
        let reflexion_mode = self.config.mode == AgentLoopMode::Reflexion;
        let shallow_reflexion = intent_result.intent.is_code_work()
            && !reflexion_mode
            && *reflexion_rounds > 0
            && reflexion_rounds.is_multiple_of(SHALLOW_REFLEXION_INTERVAL);
        if (reflexion_mode || shallow_reflexion) && budget.should_continue() {
            let verify_model = {
                let state = app.state::<crate::bootstrap::AppState>();
                state.coordinator.verify_model().map(str::to_string)
            };
            if let Err(e) = self
                .run_reflexion_critique(
                    app,
                    session_id,
                    chat_state,
                    accumulated_tool_calls,
                    cancellation_token,
                    verify_model.as_deref(),
                )
                .await
            {
                warn!(error = %e, "Reflexion critique failed");
            }
        }
        *reflexion_rounds += 1;

        if accumulated_tool_calls
            .iter()
            .any(|tc| tc.name == "todo_write")
        {
            reminder_state.on_todo_write();
        }

        if !budget.should_continue() {
            info!("Budget exceeded — forcing final answer");
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
        LoopAction::Continue
    }

    /// The no-tool path — budget gate, empty-response recovery, commit of
    /// the streamed answer, and the stop-path gate chain with TurnEnd.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn phase_no_tool_path(
        &self,
        app: &AppHandle,
        session_id: &str,
        turn_index: usize,
        debug_mode: bool,
        user_message: &str,
        chat_state: &mut ChatState,
        reminder_config: &ReminderConfig,
        cancellation_token: &CancellationToken,
        turn_id: &str,
        state: &mut LoopState,
    ) -> LoopAction {
        let LoopState {
            ref budget,
            ref mut reminder_state,
            ref mut counters,
            ref edited_files,
            ref bash_wrote_files,
            ref verification_tier,
            ref verification_failed,
            ref run_has_tool_activity,
            ref intent_result,
            ref accumulated_text,
            ref accumulated_reasoning,
            ref usage,
            ref mut augmented_message,
            ref finish_reason,
            ..
        } = state;
        let verification_tier = *verification_tier;
        let verification_failed = *verification_failed;
        let run_has_tool_activity = *run_has_tool_activity;
        let bash_wrote_files = *bash_wrote_files;
        if !budget.should_continue() {
            info!("Budget exceeded — forcing final answer");
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

        if accumulated_text.trim().is_empty() {
            match self
                .handle_empty_response(
                    app,
                    turn_id,
                    chat_state,
                    accumulated_text,
                    accumulated_reasoning,
                )
                .await
            {
                Ok(()) => return LoopAction::Continue,
                Err(_) => {
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
            }
        }

        if !accumulated_text.trim().is_empty() {
            let clean_text = crate::core::str_util::strip_tool_call_markup(accumulated_text);
            let reasoning_opt = {
                let cleaned = crate::core::str_util::strip_tool_call_markup(accumulated_reasoning);
                if cleaned.is_empty() {
                    None
                } else {
                    Some(cleaned)
                }
            };
            chat_state.push_assistant_message(
                clean_text,
                vec![],
                Some(usage.clone()),
                reasoning_opt,
            );
        }

        if matches!(
            self.run_stop_gates(
                app,
                session_id,
                turn_id,
                turn_index,
                debug_mode,
                user_message,
                chat_state,
                budget,
                reminder_state,
                reminder_config,
                counters,
                cancellation_token,
                edited_files,
                bash_wrote_files,
                verification_tier,
                verification_failed,
                run_has_tool_activity,
                intent_result,
                accumulated_text,
            )
            .await,
            StopGateDecision::Continue
        ) {
            return LoopAction::Continue;
        }

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
                status: if finish_reason == "length" {
                    TurnOutcome::Limit
                } else {
                    TurnOutcome::Done
                },
                trace_id: chat_state.trace_id.clone(),
            },
        );
        LoopAction::Break(Ok(turn_id.to_string()))
    }
}
