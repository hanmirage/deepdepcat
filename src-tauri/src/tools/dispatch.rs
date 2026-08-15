//! Tool dispatcher — handles tool execution with permission checks, hooks,
//! and streaming progress.

use crate::toolkit::{PermissionDecision, ToolContext, ToolStreamItem};
use crate::core::error::{AppError, AppResult};
use crate::core::types::{PermissionRequest, ToolCall, ToolDefinition};
use crate::hooks::types::{HookContext, HookEvent};
use crate::permissions::checker::PermissionResult;
use crate::permissions::PermissionChecker;
use crate::tools::registry::ToolRegistry;
use crate::workspace::checkpoint::FileStateTracker;
use futures_util::FutureExt;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{debug, warn};

/// Fallback wall-clock timeout for ANY tool execution (seconds).
///
/// Tools with their own (shorter) internal timeouts are unaffected — this
/// is the safety net that guarantees a tool can never hang the loop.
/// Shared with `use_tool` (the meta-tool forwards to a target tool without
/// going through this dispatcher, so it needs the same guard).
pub const TOOL_EXECUTION_TIMEOUT_SECS: u64 = 600;

/// The result of a tool execution — content plus the authoritative
/// success/failure flag (mirrors `ToolResult.is_error`, which the stream's
/// `Terminal(Ok)` variant carries but the previous `String` return dropped).
/// An optional image travels separately from the content text (read_file
/// multimodal) — without this field the image would be dropped here and never
/// reach the conversation. `app` carries an MCP Apps UI payload (raw JSON
/// from `ToolResult.metadata["mcp_app"]`) for the frontend to render.
#[derive(Debug, Clone)]
pub struct ToolExecutionOutcome {
    pub content: String,
    pub is_error: bool,
    pub image: Option<crate::toolkit::ToolImage>,
    /// MCP Apps interactive UI payload (MCP server tool results).
    pub app: Option<serde_json::Value>,
}

/// Run a single tool execution to completion (stream + reminders).
///
/// Extracted so the dispatcher can re-invoke it once for read-only
/// retryable tools without duplicating the stream-consumption logic.
#[allow(clippy::too_many_arguments)]
async fn execute_tool_inner(
    tool: &dyn crate::toolkit::Tool,
    args: &serde_json::Value,
    context: &ToolContext,
    _app: &AppHandle,
    session_id: &str,
    _turn_id: &str,
    tool_call: &ToolCall,
    max_output_chars: usize,
    reminders: &[Arc<dyn crate::tools::reminders::Reminder>],
) -> AppResult<ToolExecutionOutcome> {
    let mut stream = tool.execute_stream(args.clone(), context).await?;

    if let Some(ToolStreamItem::Terminal(result)) = stream.next().await {
        let result = result.map_err(|e| AppError::ToolExecution {
            tool_name: tool_call.name.clone(),
            message: e,
        })?;
        let content = crate::core::str_util::truncate_content(
            &result.content,
            max_output_chars,
            &tool_call.name,
        );

        // Failure path: append a "switch strategy" hint so the model
        // corrects itself instead of retrying blindly. Success-path
        // reminders are skipped on failure (they are meaningless for
        // an already-failed call and would add noise).
        if result.is_error {
            if let Some(guidance) = crate::tools::failure_guidance::FailureGuidance::evaluate(
                &tool_call.name,
                args,
                &content,
                true,
            ) {
                let with_hint =
                    crate::tools::reminders::format_with_reminders(content, vec![guidance]);
                return Ok(ToolExecutionOutcome {
                    content: with_hint,
                    is_error: true,
                    image: None,
                    app: result.mcp_app(),
                });
            }
            return Ok(ToolExecutionOutcome {
                content,
                is_error: true,
                image: None,
                app: result.mcp_app(),
            });
        }

        // Success path — collect cross-cutting reminders and append
        // them to the tool output so the model sees the hints in
        // context.
        let mut hints = Vec::new();
        for reminder in reminders {
            if let Some(hint) = reminder
                .evaluate(
                    &tool_call.name,
                    args,
                    &content,
                    session_id,
                    context.workspace.as_deref(),
                )
                .await
            {
                hints.push(hint);
            }
        }
        return Ok(ToolExecutionOutcome {
            content: crate::tools::reminders::format_with_reminders(content, hints),
            is_error: false,
            app: result.mcp_app(),
            image: result.image,
        });
    }

    Err(AppError::ToolExecution {
        tool_name: tool_call.name.clone(),
        message: "Tool stream ended without terminal result".to_string(),
    })
}

/// The tool dispatcher — coordinates tool execution.
pub struct ToolDispatcher {
    registry: Arc<ToolRegistry>,
    permissions: Arc<PermissionChecker>,
    max_output_chars: usize,
    workspace: Option<PathBuf>,
    /// Pending permission requests (shared with AppState).
    pending_permissions: Arc<
        tokio::sync::Mutex<HashMap<String, crate::permissions::grant_store::PendingPermission>>,
    >,
    /// Durable "always allow" grants — matching calls skip the prompt.
    grant_store: Arc<crate::permissions::grant_store::PermissionGrantStore>,
    /// File state tracker for checkpoint/rewind functionality.
    file_state_tracker: Option<FileStateTracker>,
    /// Behavior version injected into every tool call context.
    behavior_version: crate::toolkit::ToolBehaviorVersion,
    /// Cross-cutting reminders appended to tool outputs.
    reminders: Vec<Arc<dyn crate::tools::reminders::Reminder>>,
    /// Concurrency cap for parallel tool execution (max_concurrent_tools).
    /// `None` = unbounded (legacy behavior).
    concurrency: Option<Arc<tokio::sync::Semaphore>>,
    /// Product work mode — execution-time boundary enforcement. The registry
    /// is already filtered at build time (`for_mode`), but meta-tools and
    /// any future path that reaches the registry directly must not be able
    /// to invoke a tool declared for the other mode.
    work_mode: crate::toolkit::WorkMode,
    /// Parent session id when this dispatcher serves a SUBAGENT (spawned by
    /// a parent session). Permission prompts emitted by a subagent are
    /// labelled with this so the user sees "subagent X requests…" instead of
    /// an unknown session id.
    parent_session: Option<String>,
    /// Subagent nesting depth of the agent this dispatcher serves (0 = main
    /// loop). Threaded into every ToolContext so the `agent` tool spawns
    /// children at depth+1 — the recursion guard compares the REQUESTED
    /// depth, so a hardcoded 1 would bypass `max_depth` entirely.
    subagent_depth: u32,
    /// Whether this dispatcher serves an EVALUATOR subagent. Evaluators are
    /// "review only, never change code" — their bash is hard-forced through
    /// the read-only validator so `echo x > file` / `rm` cannot silently
    /// mutate the codebase while bypassing the edit-evidence gates
    /// (#88 audit H11: bash writes never enter agent_edited_paths, so a
    /// modifying evaluator was invisible to verification).
    is_evaluator: bool,
    /// Session provider hint (`deepseek` / `provider-<ts>` / …) threaded into
    /// every ToolContext. Meta-tools that make their own LLM calls (`agent`
    /// decompose) must route them to the SAME provider as the session, or a
    /// custom-provider model falls back to the first enabled provider and
    /// fails with HTTP 400 (the #102 model-routing bug class).
    provider: Option<String>,
    /// Session usage tracker threaded into every ToolContext — tools that
    /// make their own LLM calls (`visual_describe`, read_file transcription)
    /// record their billed tokens into the session stats here.
    usage_tracker: Option<crate::observability::usage::SessionUsageTracker>,
    /// This agent's own permission rules (custom agent definition
    /// `permissions:` frontmatter). `None` = no agent-level restrictions.
    agent_rules: Option<Arc<crate::permissions::rules::AgentPermissionRules>>,
    /// Deny rules inherited from the parent agent chain (raw `Tool(pattern)`
    /// strings). Passed into every ToolContext so nested subagents keep the
    /// full deny chain — a parent's hard deny can never be dropped by a
    /// child that only knows its own definition.
    agent_deny_rules: Vec<String>,
}

impl ToolDispatcher {
    pub fn new(
        registry: Arc<ToolRegistry>,
        permissions: Arc<PermissionChecker>,
        max_output_chars: usize,
        workspace: Option<PathBuf>,
        pending_permissions: Arc<
            tokio::sync::Mutex<HashMap<String, crate::permissions::grant_store::PendingPermission>>,
        >,
        file_state_tracker: Option<FileStateTracker>,
    ) -> Self {
        Self {
            registry,
            permissions,
            max_output_chars,
            workspace,
            pending_permissions,
            grant_store: Arc::new(crate::permissions::grant_store::PermissionGrantStore::default()),
            file_state_tracker,
            behavior_version: crate::toolkit::ToolBehaviorVersion::Current,
            reminders: vec![],
            concurrency: None,
            work_mode: crate::toolkit::WorkMode::Code,
            parent_session: None,
            subagent_depth: 0,
            is_evaluator: false,
            provider: None,
            usage_tracker: None,
            agent_rules: None,
            agent_deny_rules: Vec::new(),
        }
    }

    /// Set the session provider hint threaded into tool call contexts.
    pub fn with_provider(mut self, provider: Option<String>) -> Self {
        self.provider = provider;
        self
    }

    /// Set the session usage tracker threaded into tool call contexts.
    pub fn with_usage_tracker(
        mut self,
        tracker: Option<crate::observability::usage::SessionUsageTracker>,
    ) -> Self {
        self.usage_tracker = tracker;
        self
    }

    /// Attach this agent's own permission rules (custom agent definition).
    /// `None` = no agent-level restrictions beyond the normal layers.
    pub fn with_agent_rules(
        mut self,
        rules: Option<Arc<crate::permissions::rules::AgentPermissionRules>>,
    ) -> Self {
        self.agent_rules = rules;
        self
    }

    /// Attach the full deny chain inherited from the parent agent (raw
    /// `Tool(pattern)` strings). These ride every ToolContext so nested
    /// subagents keep the parent's hard denies.
    pub fn with_agent_deny_rules(mut self, denies: Vec<String>) -> Self {
        self.agent_deny_rules = denies;
        self
    }

    /// Set the subagent nesting depth of the serving agent (0 = main loop).
    pub fn with_subagent_depth(mut self, depth: u32) -> Self {
        self.subagent_depth = depth;
        self
    }

    /// Mark this dispatcher as serving an EVALUATOR subagent — its bash is
    /// restricted to read-only commands (verification runs), enforced in
    /// the permission pipeline.
    pub fn with_evaluator(mut self, evaluator: bool) -> Self {
        self.is_evaluator = evaluator;
        self
    }

    /// Set the product work mode (execution-time boundary enforcement).
    pub fn with_work_mode(mut self, mode: crate::toolkit::WorkMode) -> Self {
        self.work_mode = mode;
        self
    }

    /// Mark this dispatcher as serving a subagent of the given parent session
    /// (permission prompts get a "subagent" label).
    pub fn with_parent_session(mut self, parent: Option<String>) -> Self {
        self.parent_session = parent;
        self
    }

    /// Whether this dispatcher serves a SUBAGENT (spawned by a parent
    /// session). Harness-level user questions (e.g. doom-loop continue/stop)
    /// only make sense for the main loop — workers report to the parent.
    pub fn is_subagent(&self) -> bool {
        self.parent_session.is_some()
    }

    /// Attach the durable grant store ("always allow" memories).
    pub fn with_grant_store(
        mut self,
        store: Arc<crate::permissions::grant_store::PermissionGrantStore>,
    ) -> Self {
        self.grant_store = store;
        self
    }

    /// Set the behavior version injected into tool call contexts.
    pub fn with_behavior_version(
        mut self,
        version: crate::toolkit::ToolBehaviorVersion,
    ) -> Self {
        self.behavior_version = version;
        self
    }

    /// Cap concurrent tool executions with a shared semaphore.
    pub fn with_concurrency(mut self, semaphore: Arc<tokio::sync::Semaphore>) -> Self {
        self.concurrency = Some(semaphore);
        self
    }

    /// Register a cross-cutting reminder evaluator.
    pub fn with_reminder(mut self, reminder: Arc<dyn crate::tools::reminders::Reminder>) -> Self {
        self.reminders.push(reminder);
        self
    }

    /// Get all tool definitions for the model API.
    pub fn tool_definitions(&self) -> Vec<ToolDefinition> {
        // Bare whole-tool deny rules remove the tool from the model's
        // context entirely (Claude Code semantics) — scoped denies still
        // surface the tool and deny at call time.
        self.registry
            .definitions()
            .into_iter()
            .filter(|def| !self.permissions.is_tool_removed(&def.function.name))
            .collect()
    }

    /// Whether the named tool is safe for concurrent execution.
    ///
    /// Used by `execute_tool_batch` to partition tool calls into a parallel
    /// group (read-only tools like `read_file`, `grep`) and a serial group
    /// (side-effecting tools like `write_file`, `bash`).
    pub fn is_concurrency_safe(&self, tool_name: &str) -> bool {
        self.registry
            .get(tool_name)
            .map(|t| t.is_concurrency_safe())
            .unwrap_or(false)
    }

    /// Execute a single tool call with streaming progress support.
    ///
    /// Calls `tool.execute_stream()`, forwards each `ToolProgress` item to the
    /// frontend as a `StreamEvent::ToolCallProgress` event, and returns the
    /// terminal result content. `conversation` is a snapshot of the session
    /// conversation injected into the tool context (fork-mode subagents);
    /// `attached_images` carries the current turn's `(name, path)` image notes
    /// from a text-only main model path (injected into non-fork subagents).
    #[allow(clippy::too_many_arguments)]
    pub async fn execute(
        &self,
        tool_call: &ToolCall,
        app: &AppHandle,
        session_id: &str,
        turn_id: &str,
        conversation: &[crate::core::types::ConversationItem],
        model: String,
        provider: Option<String>,
        attached_images: Vec<(String, String)>,
    ) -> AppResult<ToolExecutionOutcome> {
        let tool = self
            .registry
            .get(&tool_call.name)
            .ok_or_else(|| AppError::ToolNotFound(tool_call.name.clone()))?;

        // Execution-time mode boundary: even if a tool reached this
        // dispatcher through a path that skipped build-time filtering
        // (meta-tools, future hooks), it must never run in the wrong mode.
        if !self.work_mode.allows(tool.scope()) {
            return Err(AppError::ToolNotFound(format!(
                "Tool '{}' is not available in {} mode",
                tool_call.name,
                self.work_mode.as_str()
            )));
        }

        // Acquire a concurrency permit (if capped) — parallel-safe tools
        // run concurrently but never beyond max_concurrent_tools.
        let _permit = match &self.concurrency {
            Some(sem) => Some(
                sem.clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| AppError::Internal(format!("Semaphore closed: {e}")))?,
            ),
            None => None,
        };

        let args = parse_args(tool_call)?;

        // Schema validation before anything executes — a tool called with
        // missing/typed-wrong arguments fails fast with a clear message the
        // model can correct, instead of erroring mid-execution.
        if let Err(reason) = tool.validate_args(&args) {
            debug!(tool = %tool_call.name, reason = %reason, "Tool argument validation failed");
            return Err(AppError::ToolExecution {
                tool_name: tool_call.name.clone(),
                message: format!("Invalid arguments: {reason}"),
            });
        }

        let context = ToolContext {
            session_id: session_id.to_string(),
            turn_id: turn_id.to_string(),
            call_id: tool_call.id.clone(),
            work_mode: self.work_mode,
            model,
            provider,
            usage_tracker: self.usage_tracker.clone(),
            workspace: self.workspace.clone(),
            app: app.clone(),
            file_state_tracker: self.file_state_tracker.clone(),
            behavior_version: self.behavior_version,
            conversation: conversation.to_vec(),
            parent_session_id: self.parent_session.clone(),
            agent_depth: self.subagent_depth,
            agent_deny_rules: self.agent_deny_rules.clone(),
            attached_images,
        };

        self.check_permission(tool.as_ref(), &args, &context, tool_call, app, session_id)
            .await?;

        debug!(tool = %tool_call.name, "Executing tool");

        // Uniform fallback timeout: tools with their own internal timeouts
        // are unaffected (theirs fire first); tools without one can never
        // hang the loop indefinitely. Read-only retryable tools get ONE
        // automatic retry after a short backoff for transient errors.
        let mut attempt: u32 = 0;
        loop {
            let outcome = AssertUnwindSafe(tokio::time::timeout(
                std::time::Duration::from_secs(TOOL_EXECUTION_TIMEOUT_SECS),
                execute_tool_inner(
                    tool.as_ref(),
                    &args,
                    &context,
                    app,
                    session_id,
                    turn_id,
                    tool_call,
                    self.max_output_chars,
                    &self.reminders,
                ),
            ))
            .catch_unwind()
            .await;

            match outcome {
                Ok(Ok(result)) => match result {
                    Ok(r) => return Ok(r),
                    Err(e) => {
                        let retryable =
                            attempt == 0 && tool.is_read_only() && tool.is_retryable(&e);
                        if retryable {
                            attempt += 1;
                            warn!(
                                tool = %tool_call.name,
                                error = %e,
                                "Transient tool error — retrying once"
                            );
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                            continue;
                        }
                        return Err(e);
                    }
                },
                Ok(Err(_)) => {
                    warn!(
                        tool = %tool_call.name,
                        timeout_secs = TOOL_EXECUTION_TIMEOUT_SECS,
                        "Tool execution timed out"
                    );
                    return Err(AppError::ToolExecution {
                        tool_name: tool_call.name.clone(),
                        message: format!(
                            "Tool execution timed out after {}s",
                            TOOL_EXECUTION_TIMEOUT_SECS
                        ),
                    });
                }
                Err(_) => {
                    // Normalize a panicking tool to a failed call — the same
                    // "throws become isError" discipline as the pipeline we
                    // model. A tool that unwinds must not take down the whole
                    // batch or abort the turn.
                    warn!(
                        tool = %tool_call.name,
                        "Tool panicked — isolating as a failed tool call"
                    );
                    return Err(AppError::ToolExecution {
                        tool_name: tool_call.name.clone(),
                        message: "Tool panicked during execution".to_string(),
                    });
                }
            }
        }
    }

    /// Check tool permissions — may emit a permission-request event and wait.
    ///
    /// Unified decision pipeline (one entry point, no per-tool shortcuts):
    ///
    /// 1. Plan/Read-only hard gate (mode enforcement).
    /// 2. Tool-level classification — only an explicit `Deny` veto is
    ///    authoritative; `Allow`/`Ask` both fall through to the checker so
    ///    rule layers and security checks can never be skipped by a tool
    ///    declaring itself approved.
    /// 3. `PermissionChecker::check` — the single allow/deny/ask decision:
    ///    project rules → settings rules/mode → filesystem → bash security.
    /// 4. Durable/session grants — consulted ONLY when the checker asks;
    ///    a rule-layer denial (project/settings deny) is final and can
    ///    never be overridden by a grant.
    /// 5. User prompt (30s timeout) for remaining asks.
    async fn check_permission(
        &self,
        tool: &dyn crate::toolkit::Tool,
        args: &serde_json::Value,
        context: &ToolContext,
        tool_call: &ToolCall,
        app: &AppHandle,
        session_id: &str,
    ) -> AppResult<()> {
        // ── Evaluator bash hard gate ──────────────────────────────────
        // An evaluator reviews and runs verification — it must NEVER mutate
        // the codebase. The toolset already excludes every write tool, but
        // bash is a write vector (`echo x > file`, `rm`) that would bypass
        // the edit-evidence gates (bash writes never enter
        // agent_edited_paths — a modifying evaluator was invisible to
        // verification). Force ALL evaluator bash through the read-only
        // validator, regardless of permission mode (#88 audit H11).
        if self.is_evaluator && tool.name() == "bash" {
            let command = args.get("command").and_then(|c| c.as_str()).unwrap_or("");
            if !self.permissions.is_read_only_bash(command) {
                return Err(AppError::PermissionDenied {
                    tool_name: tool_call.name.clone(),
                    reason: "Evaluator may only run read-only commands (tests/builds/checks) — \
                             file writes and destructive operations are disabled for reviewers"
                        .to_string(),
                });
            }
        }

        // ── Plan/Read-only hard gate ──────────────────────────────────
        // In plan/read-only modes non-read-only tools are rejected outright —
        // this is an enforcement gate, not a prompt (bash read-only commands
        // are handled separately by the checker's validate_read_only).
        // enter_plan_mode/exit_plan_mode are the plan loop's own tools —
        // exempt so the agent can enter/exit plan mode freely.
        //
        // The mode is SESSION-SCOPED: a session's plan phase must only lock
        // its own write tools. The global mode remains the default; a
        // subagent inherits its parent's override (parent_session) so a
        // subagent spawned inside a parent plan phase stays read-only too.
        let state = app.state::<crate::bootstrap::AppState>();
        let mode = state
            .effective_session_mode(session_id, self.parent_session.as_deref())
            .await;
        if mode.is_read_only() {
            // The use_tool meta-tool delegates to a target — classify by the
            // TARGET's read-only identity, exactly like the unified pipeline
            // below (484-504). Gating the meta-tool itself (never read-only)
            // would reject `use_tool → read_file` in plan mode while the
            // direct `read_file` call sails through — same intent, two
            // outcomes.
            let effective_read_only = if tool.name() == "use_tool" {
                args.get("tool_name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|target| {
                        self.registry.get(target).is_some_and(|t| t.is_read_only())
                    })
            } else {
                tool.is_read_only()
            };
            if tool.name() == "bash" {
                // The checker's plan-mode bash validation only sees the
                // GLOBAL mode; a session-scoped read-only override needs the
                // same read-only enforcement here (the global-Plan path keeps
                // its exact legacy behavior inside the checker).
                if !self.permissions.mode().is_read_only() {
                    let command = args.get("command").and_then(|c| c.as_str()).unwrap_or("");
                    if !self.permissions.is_read_only_bash(command) {
                        return Err(AppError::PermissionDenied {
                            tool_name: tool_call.name.clone(),
                            reason: format!(
                                "{} mode is read-only — write tools are disabled. \
                                 Switch the input-bar mode to accept edits or full \
                                 access to unlock writes",
                                mode.as_str()
                            ),
                        });
                    }
                }
            } else if !effective_read_only
                && tool.name() != "ask_user"
                && tool.name() != "enter_plan_mode"
                && tool.name() != "exit_plan_mode"
            {
                return Err(AppError::PermissionDenied {
                    tool_name: tool_call.name.clone(),
                    reason: format!(
                        "{} mode is read-only — write tools are disabled. \
                         Switch the input-bar mode to accept edits or full \
                         access to unlock writes",
                        mode.as_str()
                    ),
                });
            }
        }

        // ── Tool-level classification ─────────────────────────────────
        // Keep the Deny veto (explicit tool refusals, e.g. the use_tool
        // work-mode boundary). Allow/Ask are not decisions here — they both
        // continue into the unified checker below.
        match tool.check_permissions(args, context) {
            PermissionDecision::Deny(reason) => {
                return Err(AppError::PermissionDenied {
                    tool_name: tool_call.name.clone(),
                    reason,
                });
            }
            PermissionDecision::Allow | PermissionDecision::Ask => {}
        }

        // ── Unified permission pipeline ───────────────────────────────
        // The use_tool meta-tool delegates to its target: rules, path
        // grants, and read-only classification must see the TARGET tool's
        // name and args, otherwise grants recorded for the target never
        // match and read tools called through the meta-tool lose their
        // auto-approval.
        let (check_name, check_args, read_only) = if tool.name() == "use_tool" {
            match args.get("tool_name").and_then(|v| v.as_str()) {
                Some(target) => {
                    let target_args = args
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({}));
                    match self.registry.get(target) {
                        Some(t) => (target.to_string(), target_args.clone(), t.is_read_only_call(&target_args)),
                        None => ("use_tool".to_string(), args.clone(), false),
                    }
                }
                None => ("use_tool".to_string(), args.clone(), false),
            }
        } else {
            (
                tool.name().to_string(),
                args.clone(),
                tool.is_read_only_call(args),
            )
        };

        // Sensitive-file red line: a write targeting .env / keys /
        // credentials must ALWAYS be confirmed by the user. Neither the
        // tool's self-approval nor a durable/session grant may lift this
        // Ask — grants and Depwork's "new file" policy could otherwise
        // silently modify secret files.
        let sensitive_write = !read_only
            && crate::permissions::sensitive::is_sensitive_edit_call(&check_name, &check_args);

        // Pass the SESSION's effective mode into the rule layer: a
        // session-scoped override (e.g. a worker spawned from a plan-mode
        // parent) must drive the rule decision, not the global mode —
        // otherwise the worker's writes are denied by the global plan.
        match self.permissions.check_with_agent_rules(
            &check_name,
            &check_args,
            read_only,
            session_id,
            mode,
            self.agent_rules.as_deref(),
        ) {
            // Deny is final — durable/session grants can never override a
            // rule-layer denial (project settings deny beats everything).
            PermissionResult::Deny(reason) => Err(AppError::PermissionDenied {
                tool_name: tool_call.name.clone(),
                reason,
            }),
            PermissionResult::Allow => Ok(()),
            PermissionResult::Ask => {
                // The GRANT identity is normalized (workspace-relative →
                // absolute, separators unified, `./`/`..` collapsed): the
                // model alternates path spellings freely, and a raw-string
                // match would re-prompt for the same file ("always allow"
                // did not stick).
                let grant_args = crate::permissions::grant_store::normalize_grant_args(
                    self.workspace.as_deref(),
                    &check_name,
                    &check_args,
                );
                if !sensitive_write {
                    // ── Tool self-approval ────────────────────────────
                    // The unified pipeline said "ask"; the tool may still
                    // prove THIS call is safe without a prompt (Depwork:
                    // new file / own session output). Deny rules already
                    // ran — this can only lift Ask, never override a Deny.
                    match tool.self_approve(args, context) {
                        PermissionDecision::Allow => return Ok(()),
                        PermissionDecision::Deny(reason) => {
                            return Err(AppError::PermissionDenied {
                                tool_name: tool_call.name.clone(),
                                reason,
                            });
                        }
                        PermissionDecision::Ask => {}
                    }
                    // ── Durable grant shortcut ("always allow") ────────
                    // Dangerous bash commands are never grant-covered —
                    // they always prompt even when previously "always
                    // allowed". Sensitive writes are never grant-covered
                    // either (the red line above).
                    if self.grant_store.allows(&check_name, &grant_args) {
                        return Ok(());
                    }
                    // ── Session grant shortcut ("allow for this session")
                    // ── Pure-memory, scoped to the current session, same
                    // dangerous-command exclusion as durable grants.
                    {
                        let app_state = app.state::<crate::bootstrap::AppState>();
                        if app_state
                            .session_grant_allows(session_id, &check_name, &grant_args)
                            .await
                        {
                            return Ok(());
                        }
                    }
                }
                // Unattended runs (scheduled tasks) can never wait on a
                // human: an Ask here becomes a denial with a clear reason so
                // the loop adapts instead of stalling for 30s.
                {
                    let app_state = app.state::<crate::bootstrap::AppState>();
                    if app_state.is_unattended(session_id).await {
                        return Err(AppError::PermissionDenied {
                            tool_name: tool_call.name.clone(),
                            reason: "无人值守（定时任务）：需要人工审批的操作被拒绝，请改用已授权路径或跳过"
                                .to_string(),
                        });
                    }
                }
                // Auto-Review: gray-zone asks route to an independent
                // reviewer instead of stopping for a human. It is a swap,
                // never a grant. Rule denies already returned above, and
                // SENSITIVE WRITES are excluded here on purpose: the
                // sensitive-file preflight returns Ask (not Deny) so a human
                // must always see the change — an auto-reviewer must never
                // silently approve editing .env/keys/credentials.
                {
                    let app_state = app.state::<crate::bootstrap::AppState>();
                    let enabled = app_state
                        .config()
                        .map(|c| c.permissions.auto_review)
                        .unwrap_or(false);
                    if enabled && !sensitive_write {
                        let tripped = {
                            let mut trackers = app_state.auto_review_trackers.lock().await;
                            trackers
                                .entry(session_id.to_string())
                                .or_default()
                                .tripped()
                        };
                        if tripped {
                            crate::permissions::auto_review::emit_denied(
                                app,
                                session_id,
                                &check_name,
                                &check_args,
                                "Auto-Review 熔断：拒绝过多，已停止自动审批",
                            );
                            return Err(AppError::PermissionDenied {
                                tool_name: tool_call.name.clone(),
                                reason: "Auto-Review 熔断：连续拒绝过多，请人工确认或更换路径".to_string(),
                            });
                        }
                        match crate::permissions::auto_review::review_action(
                            app,
                            session_id,
                            &check_name,
                            &check_args,
                        )
                        .await
                        {
                            Ok(verdict) if verdict.allow => {
                                let mut trackers = app_state.auto_review_trackers.lock().await;
                                trackers
                                    .entry(session_id.to_string())
                                    .or_default()
                                    .record(false);
                                return Ok(());
                            }
                            Ok(verdict) => {
                                let mut trackers = app_state.auto_review_trackers.lock().await;
                                trackers
                                    .entry(session_id.to_string())
                                    .or_default()
                                    .record(true);
                                crate::permissions::auto_review::emit_denied(
                                    app,
                                    session_id,
                                    &check_name,
                                    &check_args,
                                    &verdict.reason,
                                );
                                return Err(AppError::PermissionDenied {
                                    tool_name: tool_call.name.clone(),
                                    reason: format!("Auto-Review 拒绝: {}", verdict.reason),
                                });
                            }
                            Err(e) => {
                                // Reviewer infrastructure failure falls back
                                // to the human prompt — fail safe, never
                                // silently allow.
                                warn!(session_id, error = %e, "Auto-Review failed — falling back to user approval");
                            }
                        }
                    }
                }
                self.ask_user_permission(tool_call, args, &check_name, &grant_args, app, session_id)
                    .await
            }
        }
    }

    /// Emit a permission-request event and wait for user response (30s timeout).
    ///
    /// `grant_name`/`grant_args` are the EFFECTIVE permission identity
    /// (`check_name`/`check_args` — the use_tool meta-tool's target when
    /// delegated). An "always allow" decision must record a grant under the
    /// same identity the lookup uses, or grants recorded through the
    /// meta-tool would never match and vice versa.
    async fn ask_user_permission(
        &self,
        tool_call: &ToolCall,
        args: &serde_json::Value,
        grant_name: &str,
        grant_args: &serde_json::Value,
        app: &AppHandle,
        session_id: &str,
    ) -> AppResult<()> {
        let request_id = crate::core::ids::tool_call_id();
        let mut args_summary = summarize_args(&tool_call.name, args);
        // Subagent requests are labelled so the dialog isn't a mystery —
        // the session_id in the payload is the SUBAGENT's id, not the
        // parent conversation's.
        if self.parent_session.is_some() {
            args_summary = format!("[subagent] {args_summary}");
        }

        // The grant identity the dialog must show before the user commits
        // to "always allow": the same tool+pattern identity used to record
        // and look up grants, plus a human-readable scope description.
        let grant_pattern =
            crate::permissions::grant_store::extract_pattern(grant_name, grant_args);
        let grant_scope =
            crate::permissions::grant_store::describe_grant(grant_name, &grant_pattern);

        // Report the parked request as a pending interaction so the frontend
        // can show a "waiting for you" status across panels.
        {
            let app_state = app.state::<crate::bootstrap::AppState>();
            app_state
                .register_pending_interaction(
                    session_id,
                    "permission",
                    &request_id,
                    format!("{} {}", tool_call.name, args_summary),
                )
                .await;
            crate::permissions::plan::broadcast_pending_interactions(app, session_id).await;
        }

        let request = PermissionRequest {
            request_id: request_id.clone(),
            tool_name: tool_call.name.clone(),
            args_summary,
            session_id: session_id.to_string(),
            parent_session_id: self.parent_session.clone(),
            grant_pattern,
            grant_scope,
        };

        let (tx, rx) =
            tokio::sync::oneshot::channel::<crate::permissions::grant_store::PermissionReply>();
        self.pending_permissions.lock().await.insert(
            request_id.clone(),
            crate::permissions::grant_store::PendingPermission {
                sender: tx,
                tool_name: grant_name.to_string(),
                args: grant_args.clone(),
                session_id: session_id.to_string(),
            },
        );

        // Permission lifecycle hooks (observe-only): PermissionAsked marks
        // the wait, Notification marks a product-level prompt.
        {
            let app_state = app.state::<crate::bootstrap::AppState>();
            let ask_ctx = HookContext::new(HookEvent::PermissionAsked, session_id)
                .with_tool(&tool_call.name, args.clone())
                .with_data("request_id", serde_json::json!(request_id))
                .with_data("grant_pattern", serde_json::json!(request.grant_pattern.clone()));
            app_state.hook_executor.execute_observe(&ask_ctx).await;
            let notify_ctx = HookContext::new(HookEvent::Notification, session_id)
                .with_data("kind", serde_json::json!("permission"))
                .with_data("message", serde_json::json!(request.args_summary.clone()));
            app_state.hook_executor.execute_observe(&notify_ctx).await;
        }

        let _ = app.emit("permission_request", &request);

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), rx).await;
        // Unconditional cleanup — EVERY outcome (approve / deny / timeout /
        // channel closed) removes the parked entry. Previously only the
        // timeout/closed branch cleaned up: an approve or deny returned
        // directly, leaking a dead entry (with a spent oneshot sender) into
        // the map for the rest of the session's life, and the frontend
        // "waiting for you" indicator stayed lit.
        {
            let app_state = app.state::<crate::bootstrap::AppState>();
            self.pending_permissions.lock().await.remove(&request_id);
            app_state
                .resolve_pending_interaction(session_id, &request_id)
                .await;
        }
        crate::permissions::plan::broadcast_pending_interactions(app, session_id).await;
        let denial_reason = match outcome {
            Ok(Ok(reply)) if reply.allow => return Ok(()),
            Ok(Ok(reply)) => reply
                .reason
                .unwrap_or_else(|| "User denied the permission request".to_string()),
            // The request was never answered (timeout / channel closed).
            Err(_) => "Permission request timed out after 30 seconds".to_string(),
            Ok(Err(_)) => "Permission channel closed".to_string(),
        };
        {
            let app_state = app.state::<crate::bootstrap::AppState>();
            let denied_ctx = HookContext::new(HookEvent::PermissionDenied, session_id)
                .with_tool(&tool_call.name, args.clone())
                .with_data("reason", serde_json::json!(denial_reason));
            app_state.hook_executor.execute_observe(&denied_ctx).await;
        }
        Err(AppError::PermissionDenied {
            tool_name: tool_call.name.clone(),
            reason: denial_reason,
        })
    }
}

/// Parse tool call arguments, defaulting to empty object if blank.
///
/// The error deliberately does NOT echo the raw arguments — tool arguments
/// may carry secrets (API keys, tokens), and the message is fed back to the
/// LLM and surfaced in the UI. Only the parse error is reported.
fn parse_args(tool_call: &ToolCall) -> AppResult<serde_json::Value> {
    if tool_call.arguments.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    tool_call.parse_arguments().map_err(|e| {
        AppError::Parse(format!(
            "Invalid JSON arguments for tool '{}': {}",
            tool_call.name, e
        ))
    })
}

/// Produce a short human-readable summary of tool arguments for permission prompts.
fn summarize_args(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "bash" | "execute_command" => args
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string(),
        "write_file" | "edit_file" | "read_file" | "list_dir" => args
            .get("path")
            .and_then(|p| p.as_str())
            .unwrap_or("")
            .to_string(),
        _ => {
            let s = args.to_string();
            if s.len() > 200 {
                crate::core::str_util::truncate_at_char_boundary(&s, 200).to_string()
            } else {
                s
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::core::config::PermissionsSection;

    #[test]
    fn evaluator_bash_gate_blocks_writes_allows_verification() {
        // Regression for #88 audit H11: an evaluator's bash is a write
        // vector that bypasses the edit-evidence gates. The read-only
        // validator must accept verification commands and reject mutating
        // ones, regardless of permission mode.
        let checker =
            crate::permissions::checker::PermissionChecker::new(PermissionsSection::default());

        // Verification commands pass.
        for cmd in [
            "cargo test",
            "npm run build",
            "python -m pytest tests/",
            "git status",
        ] {
            assert!(
                checker.is_read_only_bash(cmd),
                "evaluator must be able to run verification: {cmd}"
            );
        }

        // Mutating commands are blocked.
        for cmd in [
            "echo x > src/main.rs",
            "rm -rf target",
            "mv a.rs b.rs",
            "cp x y",
            "touch new_file.txt",
            "chmod +x script.sh",
        ] {
            assert!(
                !checker.is_read_only_bash(cmd),
                "evaluator must NOT run mutating command: {cmd}"
            );
        }
    }
}
