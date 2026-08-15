//! Parallel tool group — concurrency-safe calls executed together.

use super::super::AgentLoop;
use super::support::*;
use crate::core::error::{AppError, AppResult};
use crate::core::stream::emit_stream;
use crate::core::types::{StreamEvent, ToolCall};
use crate::hooks::{HookContext, HookEvent};
use serde_json::json;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

impl AgentLoop {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_parallel_group(
        &self,
        app: &AppHandle,
        session_id: &str,
        turn_id: &str,
        turn: u32,
        chat_state: &mut crate::agent::chat_state::ChatState,
        tool_calls: &[&ToolCall],
        cancellation_token: &CancellationToken,
        debug_mode: bool,
        skill_engine: Option<&crate::skills::activation::SkillActivationEngine>,
        can_see_images: bool,
    ) -> AppResult<u32> {
        let mut permission_denials: u32 = 0;
            // Repeat-failure guard, parallel path: a call whose (tool, args)
            // signature already failed twice is blocked BEFORE dispatch —
            // the model gets a corrective hint instead of retrying a doomed
            // read. Mirror of the serial path's pre-dispatch check.
            let mut blocked_results = Vec::new();
            let mut runnable: Vec<&ToolCall> = Vec::new();
            for tc in tool_calls {
                let guard_key = failure_guard_key(&tc.name, &tc.arguments);
                let failures = chat_state
                    .tool_failure_counts
                    .get(&guard_key)
                    .copied()
                    .unwrap_or(0);
                if failures >= 2 {
                    let blocked = format!(
                        "Blocked: {} was called with identical arguments and has already failed {} \
                         consecutive times this session. Repeating the same call cannot succeed — \
                         verify the target actually exists (e.g. the file/path), then retry with \
                         corrected arguments.",
                        tc.name, failures
                    );
                    warn!(tool = %tc.name, failures, "Repeat-failure guard blocked identical retry (parallel)");
                    emit_stream(
                        app,
                        StreamEvent::ToolCallResult {
                            turn_id: turn_id.to_string(),
                            call_id: tc.id.clone(),
                            name: tc.name.clone(),
                            result: blocked.clone(),
                            is_error: true,
                        },
                    );
                    blocked_results.push(BatchToolResult {
                        call_id: tc.id.clone(),
                        name: tc.name.clone(),
                        content: blocked,
                        is_error: true,
                        permission_denied: false,
                        image: None,
                        app: None,
                        args: json!({}),
                        arguments: tc.arguments.clone(),
                        hook_blocked: false,
                        hook_contexts: Vec::new(),
                    });
                } else {
                    runnable.push(tc);
                }
            }
            for result in blocked_results {
                chat_state.push_tool_result(
                    &result.call_id,
                    crate::core::str_util::spill_tool_output(&result.content),
                    true,
                );
            }

            // Snapshot the conversation once — shared by every concurrent
            // tool (fork-mode subagents read it from the tool context).
            let conversation_snapshot: Vec<_> = chat_state.conversation.clone();
            let image_notes_snapshot: Vec<_> = chat_state.attached_image_notes.clone();
            let provider_snapshot: Option<String> = chat_state.provider.clone();
            let futures: Vec<_> = runnable
                .iter()
                .map(|&tc| {
                    let conversation = conversation_snapshot.clone();
                    let model = chat_state.model.clone();
                    let provider = provider_snapshot.clone();
                    self.execute_single_concurrent(
                        tc,
                        app,
                        session_id,
                        turn_id,
                        cancellation_token,
                        debug_mode,
                        conversation,
                        model,
                        provider,
                        image_notes_snapshot.clone(),
                    )
                })
                .collect();
            let results = futures_util::future::join_all(futures).await;

            for result in results {
                if cancellation_token.is_cancelled() {
                    return Err(AppError::Cancelled);
                }

                if result.hook_blocked {
                    self.emit_blocked_result(app, turn_id, chat_state, &result);
                    for context in &result.hook_contexts {
                        chat_state.push_transient_system(hook_context_wrapper(
                            "PreToolUse",
                            &result.name,
                            context,
                        ));
                    }
                    // Mirror the serial path: hook denials feed the
                    // repeat-failure guard, the per-name strategy-switch
                    // signal AND the consecutive-denial termination guard —
                    // otherwise the model can retry a hook-blocked call
                    // forever at full cost.
                    record_failure_outcome(
                        &mut chat_state.tool_failure_counts,
                        &result.name,
                        &result.arguments,
                        true,
                    );
                    record_tool_name_outcome(
                        &mut chat_state.tool_name_failures,
                        &result.name,
                        true,
                    );
                    permission_denials += 1;
                    continue;
                }

                // PreToolUse `additionalContext` from allowed hooks reaches
                // the model exactly like the serial path.
                for context in &result.hook_contexts {
                    chat_state.push_transient_system(hook_context_wrapper(
                        "PreToolUse",
                        &result.name,
                        context,
                    ));
                }

                // Repeat-failure guard, parallel path: mirror the serial path
                // (the serial loop below) — failures accumulate per
                // (tool, normalized-args) signature, success clears the count.
                // Recorded here in the sequential results loop because the
                // concurrent executor itself never touches `chat_state`.
                // Permission denials are NOT failures (mirrors the serial
                // path): the user/rule said no, so denials must not poison
                // the guard or the per-name strategy-switch streak.
                if !result.permission_denied {
                    record_failure_outcome(
                        &mut chat_state.tool_failure_counts,
                        &result.name,
                        &result.arguments,
                        result.is_error,
                    );
                    // Per-tool-name consecutive failures — drives the
                    // strategy-switch nudge (#84). Tracked here too so the
                    // parallel path behaves identically to the serial one.
                    record_tool_name_outcome(
                        &mut chat_state.tool_name_failures,
                        &result.name,
                        result.is_error,
                    );
                }

                // Record file touch for skill activation.
                if let Some(engine) = skill_engine {
                    self.track_skill_file_touch(engine, &result.name, &result.args)
                        .await;
                }

                // Successful file writes must be recorded so the Evaluator-QA
                // gate and subagent spawns know exactly which files the
                // generator touched (they were previously reading an always-
                // empty list and falling back to workspace-wide review).
                if !result.is_error {
                    record_edited_path(chat_state, &result.name, &result.args);
                    mark_indexes_stale(app);

                    // Auto-LSP verification (parallel path): the serial path
                    // already pulls diagnostics after successful writes — the
                    // concurrent path must too, or edits that land through
                    // the parallel batch never reach the verification gate's
                    // structured evidence. Same "never spawn a server" rule.
                    if is_write_tool(&result.name) {
                        if let Some(path) = result.args.get("path").and_then(|p| p.as_str()) {
                            if let Some(note) = self.pull_auto_diagnostics(app, path).await {
                                let clean = !note.lines().any(|l| l.contains("[error]"));
                                let resolved = crate::tools::builtin::resolve_path(
                                    self.context_builder.workspace().as_deref(),
                                    path,
                                );
                                chat_state
                                    .record_auto_diagnostics(resolved.display().to_string(), clean);
                            }
                        }
                    }
                }

                // PostToolUse hook (observe-only)
                let post_ctx = HookContext::new(HookEvent::PostToolUse, session_id)
                    .with_tool(result.name.as_str(), result.args.clone())
                    .with_result(result.content.as_str());
                for context in self
                    .hook_executor
                    .execute_observe_collect(&post_ctx)
                    .await
                {
                    chat_state.push_transient_system(hook_context_wrapper(
                        "PostToolUse",
                        &result.name,
                        &context,
                    ));
                }
                if result.is_error {
                    let fail_ctx = HookContext::new(HookEvent::PostToolUseFailure, session_id)
                        .with_tool(result.name.as_str(), result.args.clone())
                        .with_result(result.content.as_str());
                    for context in self
                        .hook_executor
                        .execute_observe_collect(&fail_ctx)
                        .await
                    {
                        chat_state.push_transient_system(hook_context_wrapper(
                            "PostToolUseFailure",
                            &result.name,
                            &context,
                        ));
                    }
                    // ToolError hook — the same failure as a tool-level
                    // error signal (redundant with PostToolUseFailure, but
                    // the event catalog exposes both for hook pipelines).
                    let tool_err_ctx = HookContext::new(HookEvent::ToolError, session_id)
                        .with_tool(result.name.as_str(), result.args.clone())
                        .with_result(result.content.as_str());
                    self.hook_executor.execute_observe(&tool_err_ctx).await;
                }

                record_monitor_event(
                    app,
                    session_id,
                    "tool",
                    json!({
                        "tool": result.name,
                        "ok": !result.is_error,
                    }),
                );
                self.emit_and_push_result(app, turn_id, chat_state, &result, can_see_images);
                record_tool_usage(app, session_id, turn, &result.name, &result.content).await;
                if result.permission_denied {
                    permission_denials += 1;
                }
            }
        Ok(permission_denials)
    }
}
