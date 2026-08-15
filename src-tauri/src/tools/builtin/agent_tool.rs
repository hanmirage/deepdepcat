//! Agent tool — spawns subagents for delegated tasks.
//!
//! Supports three modes:
//! 1. **Single subagent** — spawn one subagent with a specific type (explore/plan/general)
//! 2. **Task decomposition** — LLM decomposes the task, then spawns parallel workers
//! 3. **Background execution** — spawn and return immediately, result injected next turn

use crate::agent::chat_state::ChatState;
use crate::agent::multi_agent::{SubagentConfig, SubagentType};
use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use crate::bootstrap::AppState;
use crate::hooks::{HookContext, HookEvent};
use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::{Emitter, Manager};
use tracing::warn;

pub struct AgentTool;

impl AgentTool {
    pub fn new() -> Self {
        Self
    }

    /// Resolve the max turns based on agent type.
    fn resolve_max_turns(agent_type: &SubagentType, user_override: Option<u32>) -> u32 {
        if let Some(turns) = user_override {
            return turns;
        }
        match agent_type {
            SubagentType::Explore => 10,
            SubagentType::Plan => 8,
            SubagentType::General => 20,
            // Evaluator is harness-internal (EvaluatorQa loop only); the
            // value here is a safety default if it ever leaks through.
            SubagentType::Evaluator => 20,
            SubagentType::Custom(_) => 15,
        }
    }

    /// Resolve the default wall-clock timeout for a subagent type.
    ///
    /// A subagent WITHOUT a user-specified timeout must still be bounded —
    /// an unbounded worker that stalls (LLM stream hang, tool wedge) makes
    /// the parent wait forever and reads to the user as "回不来了" (#84
    /// audit). Values are generous enough for honest work but hard-stop a
    /// stuck worker. A user-provided timeout always wins.
    fn resolve_timeout_secs(agent_type: &SubagentType, user_override: Option<u64>) -> Option<u64> {
        if let Some(secs) = user_override {
            return Some(secs);
        }
        Some(match agent_type {
            // Read-only search: bounded by design — a 90s cap is plenty to
            // survey a module.
            SubagentType::Explore => 90,
            // Analysis/planning: a bit more room for reading multiple files.
            SubagentType::Plan => 120,
            // Full-access work (edits + verification) legitimately takes
            // minutes; the cap prevents indefinite hangs without punishing
            // real work.
            SubagentType::General => 300,
            SubagentType::Evaluator => 120,
            // Custom agents (market-manager etc.) do long research pipelines,
            // so give them a generous but FINITE ceiling — an unbounded
            // worker that wedges holds its concurrency permit and cancellation
            // token forever (#84 audit: "回不来了").
            SubagentType::Custom(_) => 600,
        })
    }
}

/// Whether summoning this agent type needs a user confirmation: only
/// Custom specialists in a Depwork MAIN session (the "群主" flow). Built-in
/// worker types are internal mechanics; subagents never ask the user.
fn needs_summon_confirmation(
    work_mode: crate::toolkit::WorkMode,
    is_subagent: bool,
    agent_type: &SubagentType,
) -> bool {
    !is_subagent
        && work_mode == crate::toolkit::WorkMode::Depwork
        && matches!(agent_type, SubagentType::Custom(_))
}

/// Ask the user for permission before summoning a specialist agent in a
/// Depwork main session (group-chat style: the main agent proposes, the
/// user approves who enters). Returns `true` when the user confirms; a
/// timeout defaults to `true` — the main agent already judged the task
/// needs the specialist, and the turn should not hang on a silent user.
async fn confirm_specialist_summon(
    app: &tauri::AppHandle,
    session_id: &str,
    agent_name: &str,
) -> bool {
    let request_id = crate::core::ids::generate_id();
    let question = format!(
        "这个任务更适合「{agent_name}」来处理，要叫 ta 进来吗？\n\
         选择「叫他处理」会派专家执行；选择「不用」则由我继续完成。"
    );
    let payload = json!({
        "request_id": request_id,
        "session_id": session_id,
        "question": question,
        "options": ["叫他处理", "不用，你自己来"],
    });

    let (tx, rx) = tokio::sync::oneshot::channel();
    let state = app.state::<AppState>();
    state.register_user_input_request(&request_id, tx).await;
    state
        .register_pending_interaction(session_id, "question", &request_id, question.clone())
        .await;
    // UserInputRequested hook — the specialist-summon confirmation is a
    // human-in-the-loop point; observers can audit or auto-answer it.
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

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(120), rx).await;
    let response = match outcome {
        Ok(Ok(response)) => Some(response),
        Ok(Err(_)) | Err(_) => {
            let state = app.state::<AppState>();
            state.remove_user_input_request(&request_id).await;
            None
        }
    };
    state
        .resolve_pending_interaction(session_id, &request_id)
        .await;
    crate::permissions::plan::broadcast_pending_interactions(app, session_id).await;

    match response {
        Some(text) => !looks_like_decline(&text),
        // Timeout / channel closed → proceed (the specialist was already
        // the main agent's judgment).
        None => true,
    }
}

/// Whether a user's summon answer declines the specialist. Ambiguous
/// answers default to confirming the summon.
fn looks_like_decline(answer: &str) -> bool {
    let lower = answer.trim().to_lowercase();
    [
        "不用",
        "不要",
        "不需要",
        "不必",
        "算了",
        "我自己来",
        "你继续",
        "你自己来",
        "cancel",
        "no",
        "skip",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str {
        "agent"
    }

    fn description(&self) -> &str {
        "Spawn a subagent to handle a delegated task. Use for complex, multi-step tasks that benefit from focused attention. \
         Supports different agent types: 'explore' (read-only code search), 'plan' (analysis and planning), \
         'evaluator' (independent review: read-only tools + bash to run tests, never edits files), \
         'general' (full access). Can also decompose a complex task into parallel workers.\n\n\
         WHEN TO SPAWN: only when the subtask is genuinely self-contained, takes many steps, or \
         benefits from parallel exploration of SEPARATE areas. Good examples: \"survey the auth \
         module for how tokens are validated\", \"review these 3 independent files in parallel\".\n\
         WHEN NOT TO SPAWN: small or focused tasks (a single file, one fix, a short answer) are \
         FASTER done directly — spawning overhead (context copy + startup + result round-trip) \
         exceeds the work. Never spawn to \"double-check\" what you can verify in one tool call. \
         If you have already explored an area, do the follow-up yourself instead of delegating it.\n\
         SPAWN SCALE — match spawn count to task size: (1) one file, one fix, or a short answer → \
         0 subagents, do it yourself; (2) a few related files or a multi-step but cohesive task → \
         0-2 workers, only when separate areas genuinely parallelize; (3) cross-module work with \
         many independent areas → 2-3 parallel workers. never spawn more than 5 — if a task would \
         need more, re-plan it instead. Delegation costs roughly 10x the tokens of doing the work \
         yourself; spend it only when parallel effort or a clean context split clearly wins.\n\
         PACK THE TASK: the 'task' argument must be a complete, self-contained brief — the worker \
         cannot see this conversation. Every brief needs: (1) Objective — what to deliver, stated \
         so you can verify it; (2) Output — the exact format expected (files to change, paths, \
         code to produce, report structure); (3) Boundaries — what the worker may touch and what \
         it must NOT touch; (4) Background — the minimum context it needs (file paths, symbols, \
         conventions), no more. Vague briefs make workers duplicate work or miss the point.\n\
         VERIFY THE RESULT: a worker's report is a claim, not evidence. Integrate its output and \
         verify it yourself (tests, lint, typecheck, LSP diagnostics) as if you had written it; \
         when a worker reports a blocker, resolve it or re-delegate with a better brief.\n\
         If a spawned subagent returns an error or no useful result, do NOT re-spawn it with the \
         same task — do the work yourself or change approach."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task description for the subagent. Must be a complete, self-contained brief — the worker cannot see this conversation. Include: (1) Objective (what to deliver, stated so it can be verified), (2) Output format (files to change, paths, code to produce, report structure), (3) Boundaries (what it may touch and what it must NOT touch), (4) Background (minimum context: file paths, symbols, conventions). This title is shown to the user in the right panel as the subagent's card title — write it as a clear user-facing sentence, not internal shorthand."
                },
                "agent_type": {
                    "type": "string",
                    "enum": ["explore", "plan", "evaluator", "general"],
                    "description": "Type of subagent. 'explore' = read-only search, 'plan' = analysis, 'evaluator' = INDEPENDENT reviewer (read-only tools + bash to run verification, never edits files — use in the verification phase to accept/reject work), 'general' = full access. Any other string is treated as a custom agent name — if a matching definition exists in .deepdepcat/agents/*.md (name field), its prompt body and tool allowlist are applied."
                },
                "model": {
                    "type": "string",
                    "description": "Optional model override for the subagent. If not specified, uses type-based routing."
                },
                "max_turns": {
                    "type": "integer",
                    "description": "Maximum turns for the subagent. Defaults: explore=10, plan=8, general=20."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Wall-clock timeout in seconds for the whole subagent run. Defaults when omitted: explore=90, plan=120, evaluator=120, general=300 (custom agents are unbounded). Setting 0 is treated as omitted — the type default applies; there is no way to request an unbounded run for a built-in agent type. Prevents stuck workers from hanging the conversation forever."
                },
                "background": {
                    "type": "boolean",
                    "description": "If true, the subagent runs in the background. The tool returns immediately with a task ID, and the result is injected into your next conversation turn automatically."
                },
                "decompose": {
                    "type": "boolean",
                    "description": "If true, the LLM decomposes the task into independent subtasks and runs them in parallel. Useful for complex multi-part tasks."
                },
                "isolation": {
                    "type": "string",
                    "enum": ["none", "worktree"],
                    "description": "Optional isolation mode. 'worktree' runs the subagent in a dedicated git worktree so its edits never touch the parent workspace (requires a git workspace; falls back gracefully). Defaults to 'none'."
                },
                "fork": {
                    "type": "boolean",
                    "description": "DEPRECATED — do not use. Carries a compressed snapshot of the parent conversation, which makes the worker inherit the parent's identity and scope; a worker must run from a clean context with a self-contained brief (see PACK THE TASK). Kept only for backward compatibility with existing definitions."
                },
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "File paths this subagent intends to WRITE. Declaring them lets the system detect write conflicts against other running workers of this session. Keep it accurate: only files you will actually modify. Optional."
                }
            },
            "required": ["task"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let task = args
            .get("task")
            .and_then(|t| t.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'task'".into()))?;

        let agent_type_str = args
            .get("agent_type")
            .and_then(|s| s.as_str())
            .unwrap_or("general");
        let agent_type = match agent_type_str {
            "explore" => SubagentType::Explore,
            "plan" => SubagentType::Plan,
            "evaluator" => SubagentType::Evaluator,
            "general" => SubagentType::General,
            other => SubagentType::Custom(other.to_string()),
        };

        let model_override = args.get("model").and_then(|m| m.as_str());
        let max_turns_override = args
            .get("max_turns")
            .and_then(|m| m.as_u64())
            .map(|v| v as u32);
        let background = args
            .get("background")
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        let decompose = args
            .get("decompose")
            .and_then(|b| b.as_bool())
            .unwrap_or(false);
        let isolation = match args.get("isolation").and_then(|s| s.as_str()) {
            Some("worktree") => crate::agent::multi_agent::IsolationMode::Worktree,
            _ => crate::agent::multi_agent::IsolationMode::None,
        };
        let fork = args.get("fork").and_then(|b| b.as_bool()).unwrap_or(false);
        // Declared write paths (write-conflict preflight against parallel
        // workers of this session). Absent/empty = unknown.
        let planned_paths: Vec<String> = args
            .get("paths")
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        // Fork-mode parent snapshot — captured by the dispatcher when the
        // agent tool executes (the conversation as of this tool call).
        let fork_context: Vec<crate::core::types::ConversationItem> = if fork {
            context.conversation.clone()
        } else {
            Vec::new()
        };
        // Text-only main model path: the parent's attached image notes travel
        // to NON-fork subagents so they can `visual_describe` by path. Fork
        // subagents already inherit the paths via the forked conversation;
        // multimodal parents never populate `attached_images`.
        let image_notes: Vec<(String, String)> = if fork {
            Vec::new()
        } else {
            context.attached_images.clone()
        };
        let timeout_secs = args
            .get("timeout")
            .and_then(|t| t.as_u64())
            .filter(|&t| t > 0)
            .or_else(|| AgentTool::resolve_timeout_secs(&agent_type, None));

        let state = context.app.state::<AppState>();
        let coordinator = &state.coordinator;

        if !coordinator.is_enabled() {
            return Ok(ToolResult::error(
                "Multi-agent mode is disabled in configuration".to_string(),
            ));
        }

        if !coordinator.can_spawn(context.agent_depth + 1) {
            return Ok(ToolResult::error(format!(
                "Cannot spawn subagent — depth limit reached (current depth {}, max depth {})",
                context.agent_depth,
                coordinator.max_depth(),
            )));
        }

        // The EXPLICIT per-call model override only. The full resolution
        // chain (explicit → role model matrix → custom definition frontmatter
        // → parent's current model) runs inside spawn_subagent_with_cancel —
        // forwarding the parent's model here as a "resolved" value would pin
        // role-routed plan/explore/verify workers (and custom frontmatter
        // models) to the parent model, making the role matrix dead config.
        let explicit_model: Option<String> =
            model_override.map(str::to_string).filter(|m| !m.is_empty());
        let resolved_turns = AgentTool::resolve_max_turns(&agent_type, max_turns_override);

        let cancel_token = state
            .cancellation_tokens
            .lock()
            .await
            .get(&context.session_id)
            .cloned()
            .unwrap_or_else(tokio_util::sync::CancellationToken::new);

        let spawn_ctx = SpawnContext::new(coordinator, &cancel_token, context);

        let child_depth = context.agent_depth + 1;

        if background {
            if needs_summon_confirmation(
                context.work_mode,
                context.parent_session_id.is_some(),
                &agent_type,
            ) {
                let name = match &agent_type {
                    SubagentType::Custom(n) => n.clone(),
                    _ => String::new(),
                };
                if !confirm_specialist_summon(&context.app, &context.session_id, &name).await {
                    return Ok(ToolResult::success(
                        "用户选择不用专家：请你自己完成这个任务，不要再次尝试召唤专家。",
                    ));
                }
            }
            let config = SubagentConfig {
                agent_type: agent_type.clone(),
                task: task.to_string(),
                model: explicit_model.clone(),
                max_turns: resolved_turns,
                depth: child_depth,
                background: true,
                surface_completion: true,
                isolation,
                timeout_secs,
                task_id: None,
                call_id: Some(context.call_id.clone()),
                fork,
                fork_context: fork_context.clone(),
                work_mode: Some(context.work_mode.as_str().to_string()),
                session_id: Some(context.session_id.clone()),
                paths: if planned_paths.is_empty() {
                    None
                } else {
                    Some(planned_paths.clone())
                },
                image_notes: image_notes.clone(),
                inherited_denies: context.agent_deny_rules.clone(),
            };
            match spawn_ctx.coordinator.spawn_background_subagent(
                config,
                context.model.to_string(),
                context.provider.clone(),
                context.session_id.clone(),
                spawn_ctx.cancel_token.clone(),
                spawn_ctx.context.app.clone(),
            ) {
                Ok(task_id) => Ok(ToolResult::success(format!(
                    "Subagent dispatched to background.\n\n**Task ID:** {}\n**Task:** {}\n\nThe result will be injected into your next conversation turn automatically when the subagent completes.",
                    task_id, task
                ))),
                Err(e) => Ok(ToolResult::error(format!(
                    "Failed to spawn background subagent: {}", e
                ))),
            }
        } else if decompose {
            self.run_decomposed(
                task,
                explicit_model.as_deref(),
                resolved_turns,
                spawn_ctx,
                isolation,
                timeout_secs,
                fork,
                fork_context,
                image_notes,
                &context.call_id,
            )
            .await
        } else {
            self.run_single(
                task,
                &agent_type,
                explicit_model.as_deref(),
                resolved_turns,
                spawn_ctx,
                isolation,
                timeout_secs,
                fork,
                fork_context,
                image_notes,
                &context.call_id,
                planned_paths,
            )
            .await
        }
    }
}

/// Context shared across subagent spawning operations.
struct SpawnContext<'a> {
    coordinator: &'a crate::agent::multi_agent::MultiAgentCoordinator,
    cancel_token: &'a tokio_util::sync::CancellationToken,
    context: &'a ToolContext,
}

impl<'a> SpawnContext<'a> {
    fn new(
        coordinator: &'a crate::agent::multi_agent::MultiAgentCoordinator,
        cancel_token: &'a tokio_util::sync::CancellationToken,
        context: &'a ToolContext,
    ) -> Self {
        Self {
            coordinator,
            cancel_token,
            context,
        }
    }
}

impl AgentTool {
    /// Run a single subagent.
    #[allow(clippy::too_many_arguments)]
    /// Run a single subagent and wait for it to finish.
    #[allow(clippy::too_many_arguments)]
    async fn run_single(
        &self,
        task: &str,
        agent_type: &SubagentType,
        explicit_model: Option<&str>,
        max_turns: u32,
        spawn_ctx: SpawnContext<'_>,
        isolation: crate::agent::multi_agent::IsolationMode,
        timeout_secs: Option<u64>,
        fork: bool,
        fork_context: Vec<crate::core::types::ConversationItem>,
        image_notes: Vec<(String, String)>,
        call_id: &str,
        planned_paths: Vec<String>,
    ) -> AppResult<ToolResult> {
        if needs_summon_confirmation(
            spawn_ctx.context.work_mode,
            spawn_ctx.context.parent_session_id.is_some(),
            agent_type,
        ) {
            let name = match agent_type {
                SubagentType::Custom(n) => n.clone(),
                _ => String::new(),
            };
            if !confirm_specialist_summon(
                &spawn_ctx.context.app,
                &spawn_ctx.context.session_id,
                &name,
            )
            .await
            {
                return Ok(ToolResult::success(
                    "用户选择不用专家：请你自己完成这个任务，不要再次尝试召唤专家。",
                ));
            }
        }
        let config = SubagentConfig {
            agent_type: agent_type.clone(),
            task: task.to_string(),
            model: explicit_model.map(str::to_string),
            max_turns,
            depth: spawn_ctx.context.agent_depth + 1,
            background: false,
            surface_completion: true,
            isolation,
            timeout_secs,
            task_id: None,
            call_id: Some(call_id.to_string()),
            fork,
            fork_context,
            work_mode: Some(spawn_ctx.context.work_mode.as_str().to_string()),
            session_id: Some(spawn_ctx.context.session_id.clone()),
            paths: if planned_paths.is_empty() {
                None
            } else {
                Some(planned_paths)
            },
            image_notes,
            inherited_denies: spawn_ctx.context.agent_deny_rules.clone(),
        };

        // The fallback model/provider for spawn-side resolution: the parent's
        // CURRENT model and provider (a mid-session switch must reach
        // subagents immediately, and the provider hint prevents a
        // custom-provider model from falling back to the first enabled
        // provider — the #102 model-routing bug class).
        let parent_state = ChatState::with_provider(
            spawn_ctx.context.model.to_string(),
            spawn_ctx.coordinator.default_context_window(),
            spawn_ctx.context.provider.clone(),
        );

        let result = spawn_ctx
            .coordinator
            .spawn_subagent_with_cancel(
                &config,
                &parent_state,
                &spawn_ctx.context.app,
                spawn_ctx.cancel_token,
            )
            .await
            .map_err(|e| AppError::Internal(format!("Subagent execution failed: {}", e)))?;

        Ok(self.format_single_result(&result))
    }

    /// Decompose the task and run parallel workers.
    #[allow(clippy::too_many_arguments)]
    async fn run_decomposed(
        &self,
        task: &str,
        explicit_model: Option<&str>,
        max_turns: u32,
        spawn_ctx: SpawnContext<'_>,
        isolation: crate::agent::multi_agent::IsolationMode,
        timeout_secs: Option<u64>,
        fork: bool,
        fork_context: Vec<crate::core::types::ConversationItem>,
        image_notes: Vec<(String, String)>,
        call_id: &str,
    ) -> AppResult<ToolResult> {
        let worker_defs = spawn_ctx
            .coordinator
            .decompose_task(
                task,
                Some(spawn_ctx.context.model.as_str()),
                spawn_ctx.context.provider.as_deref(),
                &spawn_ctx.context.app,
                spawn_ctx.context.usage_tracker.as_ref(),
            )
            .await?;

        // ── Write-conflict preflight (ronx fleet style) ─────────────
        // Two parallel workers writing the same file would race each other.
        // Reject the whole batch so the planner can re-decompose instead of
        // letting workers clobber each other's edits.
        if let Some(conflict) = crate::agent::multi_agent::find_write_conflict(&worker_defs) {
            warn!(%conflict, "Decomposed workers have write-path conflicts");
            return Ok(ToolResult::error(conflict));
        }

        let mut output = format!(
            "Task decomposed into {} parallel workers.\n\n",
            worker_defs.len()
        );

        // Batch cancellation: a shared token whose child is each worker's own
        // token. When any worker fails, the batch token is cancelled so
        // sibling workers stop early instead of burning tokens to completion.
        // The parent's cancel token chains in — a user interrupt cancels the
        // whole batch.
        let total_workers = worker_defs.len();
        let batch_token = spawn_ctx.cancel_token.child_token();
        let mut futures: Vec<_> = Vec::with_capacity(total_workers);
        for (worker_idx, worker_def) in worker_defs.into_iter().enumerate() {
            let config = SubagentConfig {
                agent_type: worker_def.agent_type,
                task: worker_def.task,
                // LLM-declared override wins; the explicit tool override is
                // the fallback — otherwise (the common case) the worker goes
                // through the spawn-side role model resolution chain.
                model: worker_def
                    .model
                    .filter(|m| !m.is_empty())
                    .or_else(|| explicit_model.map(str::to_string)),
                max_turns: worker_def.max_turns.min(max_turns),
                depth: spawn_ctx.context.agent_depth + 1,
                background: false,
                surface_completion: true,
                isolation,
                timeout_secs,
                task_id: None,
                call_id: Some(call_id.to_string()),
                fork,
                fork_context: fork_context.clone(),
                work_mode: Some(spawn_ctx.context.work_mode.as_str().to_string()),
                session_id: Some(spawn_ctx.context.session_id.clone()),
                paths: worker_def.paths,
                image_notes: image_notes.clone(),
                inherited_denies: spawn_ctx.context.agent_deny_rules.clone(),
            };

            let coord = spawn_ctx.coordinator.clone();
            // Fallback model/provider: the parent's current model and
            // provider (see run_single) — the worker's own LLM calls must
            // route to the same provider, or a custom-provider model falls
            // back to the first enabled provider (the #102 bug class).
            let parent = ChatState::with_provider(
                spawn_ctx.context.model.to_string(),
                128_000,
                spawn_ctx.context.provider.clone(),
            );
            let app = spawn_ctx.context.app.clone();
            let worker_token = batch_token.child_token();

            let handle = tokio::spawn(async move {
                let result = coord
                    .spawn_subagent_with_cancel(&config, &parent, &app, &worker_token)
                    .await;
                (worker_idx, result)
            });
            futures.push(handle);
        }

        let mut all_success = true;
        let mut total_tokens = 0u64;
        let mut per_worker: Vec<Option<String>> = vec![None; total_workers];
        let mut pending: Vec<_> = futures;
        let mut batch_cancelled = false;

        // Race all workers; the moment one fails, cancel the batch so the
        // remaining workers stop instead of completing uselessly.
        let mut panic_messages: Vec<String> = Vec::new();
        while !pending.is_empty() {
            let (joined, _, rest) = futures_util::future::select_all(pending).await;
            let (worker_idx, worker_result) = match joined {
                Ok(joined) => joined,
                Err(e) => {
                    // A worker task panicked — it carried no index, so record a
                    // generic failure and cancel the rest of the batch.
                    all_success = false;
                    if !batch_cancelled {
                        batch_cancelled = true;
                        batch_token.cancel();
                    }
                    panic_messages.push(format!("A worker panicked: {e}"));
                    pending = rest;
                    continue;
                }
            };

            let (status, body, modified): (String, String, Vec<String>) = match worker_result {
                Ok(result) => {
                    if !result.success {
                        all_success = false;
                    }
                    total_tokens += result.usage.total();
                    (
                        if result.success {
                            "✓ success".to_string()
                        } else {
                            "✗ failed".to_string()
                        },
                        result.response.clone(),
                        result.modified_files.clone(),
                    )
                }
                Err(e) if batch_cancelled => {
                    all_success = false;
                    (
                        "✗ cancelled".to_string(),
                        format!("Cancelled: {e}"),
                        Vec::new(),
                    )
                }
                Err(e) => {
                    all_success = false;
                    ("✗ failed".to_string(), format!("Failed: {e}"), Vec::new())
                }
            };

            // First failure — cancel the sibling workers. Re-evaluated each
            // iteration so later failures don't re-cancel (idempotent).
            if status == "✗ failed" && !batch_cancelled {
                batch_cancelled = true;
                batch_token.cancel();
            }

            per_worker[worker_idx] = Some(format!(
                "### Worker {} ({})\n{}\n\n**Modified files:** {}\n\n",
                worker_idx + 1,
                status,
                body,
                if modified.is_empty() {
                    "none".to_string()
                } else {
                    modified.join(", ")
                }
            ));

            pending = rest;
        }

        for text in per_worker.into_iter().flatten() {
            output.push_str(&text);
        }
        for msg in panic_messages {
            output.push_str(&format!("\n{msg}\n"));
        }

        output.push_str(&format!(
            "**Summary:** {} | **Total tokens:** {}",
            if all_success {
                "all workers succeeded"
            } else if batch_cancelled {
                "a worker failed — remaining workers were cancelled"
            } else {
                "some workers failed"
            },
            total_tokens
        ));

        Ok(ToolResult::success(output))
    }

    /// Format a single subagent result.
    fn format_single_result(
        &self,
        result: &crate::agent::multi_agent::SubagentResult,
    ) -> ToolResult {
        let mut output = String::new();
        if result.success {
            output.push_str(&format!(
                "Subagent completed successfully.\n\n**Worker report (internal context — \
                 synthesize, do not relay verbatim):**\n{}\n\n**Modified files:** {}",
                result.response,
                if result.modified_files.is_empty() {
                    "none".to_string()
                } else {
                    result.modified_files.join(", ")
                }
            ));
        } else {
            // A failed worker can still carry a partial report and modified
            // files (spawn.rs populates them on timeout/cancel precisely so
            // the parent can resume instead of restarting). Dropping them here
            // makes that resume path dead at the tool boundary — the parent
            // would re-investigate from scratch.
            let partial = if result.response.trim().is_empty() {
                "(none)".to_string()
            } else {
                result.response.clone()
            };
            let modified = if result.modified_files.is_empty() {
                "none".to_string()
            } else {
                result.modified_files.join(", ")
            };
            output.push_str(&format!(
                "Subagent failed: {}\n\n**Worker partial report (internal context — \
                 synthesize, do not relay verbatim):**\n{}\n\n**Modified files:** {}",
                result.error.as_deref().unwrap_or("unknown error"),
                partial,
                modified,
            ));
        }

        if result.usage.total() > 0 {
            output.push_str(&format!("\n\n**Token usage:** {}", result.usage.total()));
        }

        ToolResult::success(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_timeout_secs_bounds_every_builtin_type() {
        // Every built-in subagent type gets a finite default timeout — an
        // unbounded worker makes the parent wait forever when it stalls
        // (the "子代理回不来" failure mode).
        for t in [
            SubagentType::Explore,
            SubagentType::Plan,
            SubagentType::General,
            SubagentType::Evaluator,
        ] {
            assert!(
                AgentTool::resolve_timeout_secs(&t, None).is_some(),
                "{t:?} must have a finite default timeout"
            );
        }
    }

    #[test]
    fn resolve_timeout_secs_user_override_wins() {
        assert_eq!(
            AgentTool::resolve_timeout_secs(&SubagentType::Explore, Some(30)),
            Some(30)
        );
        assert_eq!(
            AgentTool::resolve_timeout_secs(&SubagentType::General, Some(0)),
            Some(0)
        );
    }

    #[test]
    fn resolve_timeout_secs_custom_agents_get_a_finite_ceiling() {
        // Custom agents (market-manager etc.) run long research pipelines, so
        // they get a generous but FINITE default — an unbounded worker that
        // wedges holds its concurrency permit forever (#84 audit).
        assert_eq!(
            AgentTool::resolve_timeout_secs(&SubagentType::Custom("market-manager".into()), None),
            Some(600)
        );
        // A user override still wins.
        assert_eq!(
            AgentTool::resolve_timeout_secs(&SubagentType::Custom("market-manager".into()), Some(1200)),
            Some(1200)
        );
    }

    #[test]
    fn resolve_max_turns_defaults_per_type() {
        assert_eq!(
            AgentTool::resolve_max_turns(&SubagentType::Explore, None),
            10
        );
        assert_eq!(AgentTool::resolve_max_turns(&SubagentType::Plan, None), 8);
        assert_eq!(
            AgentTool::resolve_max_turns(&SubagentType::General, None),
            20
        );
        assert_eq!(
            AgentTool::resolve_max_turns(&SubagentType::Explore, Some(5)),
            5
        );
    }

    /// The tool description is the model-facing delegation contract. It must
    /// carry the task-packing brief (self-contained Objective/Output/
    /// Boundaries/Background), the spawn-count cap, and post-delegation
    /// verification — locked here so prompt refactors cannot silently drop
    /// the behavior.
    #[test]
    fn description_carries_delegation_contract() {
        let d = AgentTool.description();
        assert!(d.contains("PACK THE TASK"), "packing brief required: {d}");
        assert!(d.contains("never spawn more than 5"), "spawn cap required");
        assert!(
            d.contains("VERIFY THE RESULT"),
            "verification contract required"
        );
        assert!(d.contains("self-contained brief"), "brief wording required");
        // The effort-scaling rule must mirror the three delegation tiers
        // injected into <task-spec> (direct / parallel_2_3 / parallel_3_5).
        assert!(d.contains("SPAWN SCALE"), "effort-scaling rule required");
        assert!(d.contains("0 subagents"), "direct tier must be stated: {d}");
        let params = AgentTool.parameters();
        let task_desc = params["properties"]["task"]["description"]
            .as_str()
            .unwrap();
        assert!(
            task_desc.contains("cannot see this conversation"),
            "task param must demand self-contained briefs"
        );
        // The fork mode contradicts the self-contained-brief contract (it
        // inherits the parent conversation) — it must be marked deprecated
        // so the model does not pick it up.
        let fork_desc = params["properties"]["fork"]["description"]
            .as_str()
            .unwrap();
        assert!(
            fork_desc.contains("DEPRECATED"),
            "fork must be deprecated: {fork_desc}"
        );
    }

    #[test]
    fn summon_confirmation_only_for_depwork_main_custom_agents() {
        let depwork = crate::toolkit::WorkMode::Depwork;
        let code = crate::toolkit::WorkMode::Code;
        let custom = SubagentType::Custom("市场经理".into());
        let general = SubagentType::General;

        // Depwork main session summoning a custom specialist → confirm.
        assert!(needs_summon_confirmation(depwork, false, &custom));
        // Same specialist as a SUBAGENT of another session → no confirm.
        assert!(!needs_summon_confirmation(depwork, true, &custom));
        // Code main session summoning a custom agent → no confirm (this
        // flow is depwork's "群聊" design; code keeps the old behavior).
        assert!(!needs_summon_confirmation(code, false, &custom));
        // Built-in worker types never confirm.
        assert!(!needs_summon_confirmation(depwork, false, &general));
    }

    #[test]
    fn summon_decline_answers_are_recognized() {
        for answer in [
            "不用",
            "不用，你自己来",
            "不需要",
            "算了",
            "不要叫了",
            "cancel",
            "skip",
        ] {
            assert!(looks_like_decline(answer), "must decline: {answer}");
        }
        for answer in ["叫他处理", "叫他来吧", "好的", "叫", "ok", "yes", ""] {
            assert!(!looks_like_decline(answer), "must confirm: {answer}");
        }
    }
}
