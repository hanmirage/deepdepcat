//! Plan-mode tools — `enter_plan_mode` / `exit_plan_mode`.
//!
//! The "pause and plan with the user" loop:
//!
//! 1. `enter_plan_mode` — the agent proactively switches the permission mode
//!    to `ReadOnly` (read-only hard gate). The previous mode is remembered per
//!    session so it can be restored once the plan is approved.
//! 2. `exit_plan_mode` — the agent presents its plan. The tool BLOCKS: it
//!    registers a pending plan approval, emits a `plan-approval-request`
//!    event, and waits for the user's decision (10-minute cap).
//! 3. `Approved` → restore the previous mode, return success — the model
//!    continues into implementation on the next loop iteration.
//!    `Rejected(feedback)` → stay in plan mode, return the sanitized
//!    feedback as a tool error so the model revises and re-submits.

use crate::agent::sanitize::sanitize_injection_slot;
use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use crate::bootstrap::AppState;
use crate::permissions::mode::PermissionMode;
use crate::permissions::plan::{collect_changed_files, now_secs, PlanDecision};
use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::{Emitter, Manager};

/// Cap on how long the agent parks waiting for plan approval.
const PLAN_APPROVAL_TIMEOUT_SECS: u64 = 600;

// ── enter_plan_mode ─────────────────────────────────────────────────────────

pub struct EnterPlanModeTool;

impl EnterPlanModeTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &str {
        "enter_plan_mode"
    }

    fn description(&self) -> &str {
        "Enter a read-only plan mode: switch the permission mode to Plan so you can \
         explore the codebase and write an implementation plan without changing files. \
         Use when a task has ambiguity about the right approach or the user asks for \
         a plan. Call exit_plan_mode when the plan is ready."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, _args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let state = context.app.state::<AppState>();
        // Session-scoped mode: entering plan mode here must only lock THIS
        // session's write tools, never every other session's (the global
        // mode stays the shared default for sessions without an override).
        let current = state.session_mode(&context.session_id).await;
        if current == PermissionMode::ReadOnly {
            return Ok(ToolResult::success(
                "Already in plan mode (read-only). Explore the codebase, write your \
                 plan, then call exit_plan_mode to present it.",
            ));
        }
        if !current.is_read_only() {
            // Remember what mode we came from so approval can restore it.
            state
                .set_plan_previous_mode(&context.session_id, current.as_str().to_string())
                .await;
        }
        state
            .set_session_mode(&context.session_id, PermissionMode::ReadOnly)
            .await;
        state
            .broadcast_plan_mode(&context.app, &context.session_id)
            .await;
        let mut msg = String::from(
            "Plan mode enabled — read-only. Investigate BEFORE you write: (1) read \
             the files the task touches and search for related symbols, (2) confirm \
             the existing patterns and conventions, (3) then write your plan. Your \
             plan's KEY FILES must be files you actually read. Call exit_plan_mode \
             to present the plan for approval.",
        );
        // Calibrate against the workspace's plan history — recent outcomes
        // (what got rejected and whether steps overran) are cheap, concrete
        // signals the model should not plan against blindly.
        if let Some(ws) = state.workspace.read().ok().and_then(|g| (*g).clone()) {
            let reflections = crate::permissions::plan::read_plan_reflections(&ws);
            if !reflections.is_empty() {
                msg.push_str(
                    "\n\nRecent plan outcomes in this workspace (calibrate against them):",
                );
                for r in reflections.iter().rev().take(3) {
                    let outcome = r.steps_done.map_or_else(
                        || "no structured steps".to_string(),
                        |done| format!("{done}/{} steps completed", r.steps_total.unwrap_or(0)),
                    );
                    let mut line = format!("\n- {}: {outcome}", r.plan_hint);
                    if let Some(fb) = &r.feedback {
                        let clipped: String = fb.chars().take(120).collect();
                        line.push_str(&format!(" — was rejected first: \"{clipped}\""));
                    }
                    msg.push_str(&line);
                }
            }
        }
        Ok(ToolResult::success(msg))
    }
}

// ── exit_plan_mode ──────────────────────────────────────────────────────────

pub struct ExitPlanModeTool;

impl ExitPlanModeTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str {
        "exit_plan_mode"
    }

    fn description(&self) -> &str {
        "Present your implementation plan and PAUSE for user approval. The plan is \
         shown in an approval panel; the user can approve it (you leave plan mode \
         and start coding), reject it with feedback (you stay in plan mode, revise \
         the plan, and call exit_plan_mode again), or simply wait. Write a plan the \
         user can defend: BACKGROUND, APPROACH (chosen vs rejected + why), KEY FILES \
         (only ones you actually read), numbered STEPS, OUT OF SCOPE, ASSUMPTIONS, \
         and VERIFY. The panel renders exactly what you write. If the request was \
         ambiguous, clarify it with ask_user BEFORE presenting a plan."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "plan": {
                    "type": "string",
                    "description": "The complete implementation plan for the user to review. \
                        Structure it as a defensible, reviewable plan: BACKGROUND (what/why), \
                        APPROACH (the design you chose AND the alternative you rejected, and why), \
                        KEY FILES (only files you have actually read — read any you have not), \
                        numbered STEPS in execution order, OUT OF SCOPE (what you deliberately \
                        will not do), ASSUMPTIONS (facts you rely on that the user should confirm), \
                        and how you will VERIFY the result (tests/lint for code, read-back for documents)."
                }
            },
            "required": ["plan"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let plan_content = args
            .get("plan")
            .and_then(|p| p.as_str())
            .unwrap_or_default()
            .to_string();
        if plan_content.trim().is_empty() {
            return Ok(ToolResult::error(
                "exit_plan_mode requires a non-empty `plan` argument. Write your \
                 plan first, then present it.",
            ));
        }

        let state = context.app.state::<AppState>();
        let request_id = crate::core::ids::generate_id();
        let changed_files = collect_changed_files(context.workspace.as_deref());

        let (tx, rx) = tokio::sync::oneshot::channel();
        state
            .register_plan_approval(&request_id, tx, &context.session_id)
            .await;

        let payload = json!({
            "request_id": request_id,
            "session_id": context.session_id,
            "plan": plan_content,
            "changed_files": changed_files,
            "created_at": now_secs(),
        });
        let _ = context.app.emit("plan-approval-request", &payload);
        crate::permissions::plan::broadcast_pending_interactions(&context.app, &context.session_id)
            .await;

        match tokio::time::timeout(
            std::time::Duration::from_secs(PLAN_APPROVAL_TIMEOUT_SECS),
            rx,
        )
        .await
        {
            Ok(Ok(PlanDecision::Approved)) => {
                let restored = state
                    .take_plan_previous_mode(&context.session_id)
                    .await
                    .map(|m| PermissionMode::from_str(&m))
                    .filter(|m| !m.is_read_only())
                    // Plan approval restores to ACCEPT-EDITS (the product's
                    // default execution posture) instead of the removed
                    // "confirm before changes" mode.
                    .unwrap_or(PermissionMode::AcceptEdits);
                // Restore into the SESSION-scoped override: approving this
                // session's plan must never flip the global mode that every
                // other session (and the user's own setting) sees.
                state.set_session_mode(&context.session_id, restored).await;
                state
                    .broadcast_plan_mode(&context.app, &context.session_id)
                    .await;
                // Structured planner: parse the approved plan into steps and
                // store them per session. The loop's checklist gate reminds
                // the model to walk through the steps one by one.
                let steps = crate::permissions::plan::parse_plan_steps(&plan_content);
                state
                    .set_active_plan_steps(&context.session_id, steps.clone())
                    .await;
                // Keep the RAW plan text so the run-end hook can archive it
                // to `.deepdepcat/plans/<session>.md` after execution.
                state
                    .set_approved_plan_text(&context.session_id, plan_content.clone())
                    .await;
                // Bridge the approved plan into the TASK PANEL: when the
                // session has no todo list yet, seed it from the parsed
                // steps so the user sees the plan as actionable items. The
                // model keeps them in sync via todo_write (marking done as
                // it executes) — an existing list is left untouched.
                if !steps.is_empty() && state.todo_store.get(&context.session_id).is_none() {
                    let todos = crate::permissions::plan::plan_steps_to_todos(&steps);
                    state.todo_store.set(&context.session_id, todos.clone());
                    let _ = context.app.emit(
                        "todo-list-updated",
                        crate::tools::builtin::todo_write::TodoListEvent {
                            session_id: context.session_id.clone(),
                            todos,
                        },
                    );
                }
                crate::permissions::plan::broadcast_pending_interactions(
                    &context.app,
                    &context.session_id,
                )
                .await;
                let mut msg = String::from(
                    "Your plan has been approved. You can now start coding — \
                     implement exactly what the plan specifies, step by step.",
                );
                if !steps.is_empty() {
                    msg.push_str("\n\nApproved plan steps:");
                    for (i, s) in steps.iter().enumerate() {
                        msg.push_str(&format!("\n{}. {}", i + 1, s.text));
                    }
                    msg.push_str(
                        "\n\nTrack each step with todo_write and mark them done \
                         as you complete them.",
                    );
                }
                Ok(ToolResult::success(msg))
            }
            Ok(Ok(PlanDecision::Rejected(feedback))) => {
                // Stay in plan mode. The sanitized feedback becomes a tool
                // error so the model revises the plan and re-submits.
                let safe = sanitize_injection_slot(&feedback);
                crate::permissions::plan::broadcast_pending_interactions(
                    &context.app,
                    &context.session_id,
                )
                .await;
                Ok(ToolResult::error(format!(
                    "The user rejected your plan. Feedback:\n{safe}\n\
                     Revise the plan to address the feedback, then call \
                     exit_plan_mode again to re-submit."
                )))
            }
            Ok(Err(_)) | Err(_) => {
                state.abandon_plan_approval(&request_id).await;
                crate::permissions::plan::broadcast_pending_interactions(
                    &context.app,
                    &context.session_id,
                )
                .await;
                Ok(ToolResult::error(
                    "No plan decision was received (timeout or channel closed). \
                     Re-raise exit_plan_mode to present the plan again.",
                ))
            }
        }
    }
}
