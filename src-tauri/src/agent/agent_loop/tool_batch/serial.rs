//! Serial tool group — side-effecting calls executed one at a time.

use super::super::AgentLoop;
use super::support::*;
use crate::core::error::{AppError, AppResult};
use crate::core::stream::emit_stream;
use crate::core::types::{emit_debug_trace, DebugEvent, StreamEvent, ToolCall};
use crate::hooks::{HookContext, HookEvent};
use crate::skills::{extract_file_path, is_file_tool};
use serde_json::json;
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

impl AgentLoop {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_serial_group(
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
        for tool_call in tool_calls {
            if cancellation_token.is_cancelled() {
                return Err(AppError::Cancelled);
            }

            // ── Repeat-failure guard ────────────────────────────────
            // A call whose (tool, args) signature already failed twice this
            // session is blocked before dispatch — the model gets a
            // corrective hint instead of burning tokens retrying the same
            // doomed operation (e.g. an edit whose old_string no longer
            // matches). Successful calls clear their signature's count.
            let guard_key = failure_guard_key(&tool_call.name, &tool_call.arguments);
            let failures = chat_state
                .tool_failure_counts
                .get(&guard_key)
                .copied()
                .unwrap_or(0);
            if failures >= 2 {
                let blocked = format!(
                    "Blocked: {} was called with identical arguments and has already failed {} consecutive \
                     times this session. Repeating the same call cannot succeed — adjust the arguments \
                     (e.g. re-read the file to get its current content before retrying an edit), or choose \
                     a different approach.",
                    tool_call.name, failures
                );
                warn!(tool = %tool_call.name, failures, "Repeat-failure guard blocked identical retry");
                emit_stream(
                    app,
                    StreamEvent::ToolCallResult {
                        turn_id: turn_id.to_string(),
                        call_id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        result: blocked.clone(),
                        is_error: true,
                    },
                );
                chat_state.push_tool_result(
                    &tool_call.id,
                    crate::core::str_util::spill_tool_output(&blocked),
                    true,
                );
                continue;
            }

            let tool_args = self
                .parse_tool_args(tool_call, app, turn_id, chat_state)
                .await;
            let Some(mut tool_args) = tool_args else {
                // Parse failure already emitted + pushed ONE error result
                // for this call — skip dispatch so the same call_id never
                // receives a second result (which would corrupt the
                // conversation for OpenAI-compatible APIs).
                continue;
            };

            // Record file touch for skill activation.
            if let Some(engine) = skill_engine {
                if is_file_tool(&tool_call.name) {
                    if let Some(path_str) = extract_file_path(&tool_call.name, &tool_args) {
                        engine
                            .record_file_touch(std::path::Path::new(&path_str))
                            .await;
                    }
                }
            }

            // PreToolUse hook gate
            let pre_ctx = HookContext::new(HookEvent::PreToolUse, session_id)
                .with_tool(tool_call.name.as_str(), tool_args.clone());
            emit_debug_trace(
                app,
                debug_mode,
                DebugEvent::hook_trigger(session_id, "PreToolUse"),
            );
            let mut rewritten = false;
            match self.hook_executor.execute_gate(&pre_ctx).await {
                Ok(outcome) => {
                    for context in outcome.additional_context {
                        chat_state.push_transient_system(hook_context_wrapper(
                            "PreToolUse",
                            &tool_call.name,
                            &context,
                        ));
                    }
                    if let Some(updated) = outcome.updated_input {
                        tracing::info!(
                            tool = %tool_call.name,
                            "PreToolUse hook rewrote tool input"
                        );
                        tool_args = updated;
                        rewritten = true;
                    }
                }
                Err(reason) => {
                    warn!(tool = %tool_call.name, reason = %reason, "Tool blocked by PreToolUse hook");
                    let blocked = format!("Blocked by hook: {}", reason);
                    emit_stream(
                        app,
                        StreamEvent::ToolCallResult {
                            turn_id: turn_id.to_string(),
                            call_id: tool_call.id.clone(),
                            name: tool_call.name.clone(),
                            result: blocked.clone(),
                            is_error: true,
                        },
                    );
                    chat_state.push_tool_result(
                        &tool_call.id,
                        crate::core::str_util::spill_tool_output(&blocked),
                        true,
                    );
                    // Hook denials are failures like any other: they feed the
                    // repeat-failure guard, the per-name strategy-switch
                    // signal AND the consecutive-denial termination guard —
                    // otherwise the model can retry a hook-blocked call
                    // forever at full cost.
                    record_failure_outcome(
                        &mut chat_state.tool_failure_counts,
                        &tool_call.name,
                        &tool_call.arguments,
                        true,
                    );
                    record_tool_name_outcome(
                        &mut chat_state.tool_name_failures,
                        &tool_call.name,
                        true,
                    );
                    permission_denials += 1;
                    continue;
                }
            }

            // Tool dispatch
            emit_debug_trace(
                app,
                debug_mode,
                DebugEvent::tool_dispatch(session_id, &tool_call.name, &tool_call.arguments),
            );
            // A hook that rewrote the input dispatches the REWRITTEN call —
            // the model's original arguments stay in `tool_call` for the
            // repeat-failure guard and audit, while the permission system
            // sees the rewritten (safer) variant.
        let rewritten_call = if rewritten {
            let mut c = (*tool_call).clone();
            c.arguments = tool_args.to_string();
            Some(c)
            } else {
                None
            };
            let dispatch_call: &ToolCall = rewritten_call.as_ref().map_or(tool_call, |c| c);
            // Cancellation DURING a running tool: select against the token so
            // a user stop drops the tool future mid-flight — for bash the
            // dropped `Child` (kill_on_drop) kills the process tree instead of
            // waiting out the command's own timeout. The dispatcher awaits the
            // tool directly (no spawn), so dropping its future propagates.
            let ran = tokio::select! {
                r = self.tool_dispatcher.execute(
                    dispatch_call,
                    app,
                    session_id,
                    turn_id,
                    &chat_state.conversation,
                    chat_state.model.clone(),
                    chat_state.provider.clone(),
                    chat_state.attached_image_notes.clone(),
                ) => Some(r),
                _ = cancellation_token.cancelled() => None,
            };
            let Some(result) = ran else {
                warn!(tool = %tool_call.name, "Tool cancelled mid-execution — dropping future");
                return Err(AppError::Cancelled);
            };
            let (content, is_error, image, mcp_app) = match result {
                Ok(outcome) => {
                    if outcome.is_error {
                        // Content-level failure (ToolResult::error — e.g. bash
                        // non-zero exit, edit text-not-found) now feeds the
                        // repeat-failure guard like any other failure.
                        record_failure_outcome(
                            &mut chat_state.tool_failure_counts,
                            &tool_call.name,
                            &tool_call.arguments,
                            true,
                        );
                        record_tool_name_outcome(
                            &mut chat_state.tool_name_failures,
                            &tool_call.name,
                            true,
                        );
                        record_tool_diagnostic_kind(app, &tool_call.name, "tool_error");
                        warn!(tool = %tool_call.name, error = %outcome.content, "Tool failed");
                    } else {
                        // Success clears the repeat-failure guard for this signature.
                        record_failure_outcome(
                            &mut chat_state.tool_failure_counts,
                            &tool_call.name,
                            &tool_call.arguments,
                            false,
                        );
                        // A success also clears the per-name failure streak —
                        // the approach is working again.
                        record_tool_name_outcome(
                            &mut chat_state.tool_name_failures,
                            &tool_call.name,
                            false,
                        );
                        // Record the edited file so Evaluator-QA / spawns can
                        // target exactly what the generator touched.
                        record_edited_path(chat_state, &tool_call.name, &tool_args);
                        // The cached symbol index is now pre-edit — the next
                        // search_symbols call must rebuild, not serve stale
                        // answers about the old content.
                        mark_indexes_stale(app);
                    }
                    (outcome.content, outcome.is_error, outcome.image, outcome.app)
                }
                Err(e) => {
                    let denied = e.is_permission_denied();
                    if denied {
                        permission_denials += 1;
                    }
                    // Same-signature failures accumulate for the guard. A
                    // permission denial is NOT a tool failure: the call did
                    // not fail, the user/rule said no — counting it would
                    // make the repeat-failure guard "Block" a call the model
                    // may legitimately retry after addressing the concern,
                    // with a message that claims the call itself failed.
                    if !denied {
                        record_failure_outcome(
                            &mut chat_state.tool_failure_counts,
                            &tool_call.name,
                            &tool_call.arguments,
                            true,
                        );
                        record_tool_name_outcome(
                            &mut chat_state.tool_name_failures,
                            &tool_call.name,
                            true,
                        );
                    }
                    record_tool_diagnostic(app, &tool_call.name, &e);
                    warn!(tool = %tool_call.name, error = %e, "Tool failed");
                    // Dispatch-level failures (Io / timeout / use_tool
                    // forwarding) get a recovery hint too — the content-level
                    // hint was already added inside dispatch.
                    let content = e.to_string();
                    let content = if e.is_permission_denied() {
                        content
                    } else if let Some(guidance) =
                        crate::tools::failure_guidance::FailureGuidance::evaluate(
                            &tool_call.name,
                            &tool_args,
                            &content,
                            true,
                        )
                    {
                        crate::tools::reminders::format_with_reminders(content, vec![guidance])
                    } else {
                        content
                    };
                    (content, true, None, None)
                }
            };

            // Auto LSP verification: after a successful file write, pull
            // diagnostics from an ALREADY-RUNNING language server (never
            // spawn one here — cold-starting rust-analyzer could stall the
            // loop for many seconds). The note is appended to the tool
            // result so the model sees type-check feedback immediately,
            // and recorded on chat_state so the verification gate has
            // structured evidence instead of relying on the model's
            // self-report. No server running / pull failure → silently
            // skipped (the gate then behaves as before).
            let mut auto_diag_note: Option<String> = None;
            if !is_error && is_write_tool(&tool_call.name) {
                if let Some(path) = tool_args.get("path").and_then(|p| p.as_str()) {
                    auto_diag_note = self.pull_auto_diagnostics(app, path).await;
                    if let Some(ref note) = auto_diag_note {
                        let clean = !note.lines().any(|l| l.contains("[error]"));
                        let resolved = crate::tools::builtin::resolve_path(
                            self.context_builder.workspace().as_deref(),
                            path,
                        );
                        chat_state.record_auto_diagnostics(resolved.display().to_string(), clean);
                    }
                }
            }
            let content = match &auto_diag_note {
                Some(note) => format!("{content}\n{note}"),
                None => content,
            };

            emit_debug_trace(
                app,
                debug_mode,
                DebugEvent::tool_result(session_id, &tool_call.name, 0, is_error),
            );

            // PostToolUse hook (observe-only)
            let post_ctx = HookContext::new(HookEvent::PostToolUse, session_id)
                .with_tool(tool_call.name.as_str(), tool_args.clone())
                .with_result(content.as_str());
            for context in self
                .hook_executor
                .execute_observe_collect(&post_ctx)
                .await
            {
                chat_state.push_transient_system(hook_context_wrapper(
                    "PostToolUse",
                    &tool_call.name,
                    &context,
                ));
            }
            if is_error {
                let fail_ctx = HookContext::new(HookEvent::PostToolUseFailure, session_id)
                    .with_tool(tool_call.name.as_str(), tool_args.clone())
                    .with_result(content.as_str());
                for context in self
                    .hook_executor
                    .execute_observe_collect(&fail_ctx)
                    .await
                {
                    chat_state.push_transient_system(hook_context_wrapper(
                        "PostToolUseFailure",
                        &tool_call.name,
                        &context,
                    ));
                }
                let tool_err_ctx = HookContext::new(HookEvent::ToolError, session_id)
                    .with_tool(tool_call.name.as_str(), tool_args.clone())
                    .with_result(content.as_str());
                self.hook_executor.execute_observe(&tool_err_ctx).await;
            }

            record_monitor_event(
                app,
                session_id,
                "tool",
                json!({
                    "tool": tool_call.name,
                    "ok": !is_error,
                }),
            );

            // Emit result + push to conversation. The conversation injection
            // is the centralized output guard: tools truncate internally, but
            // external sources (MCP results, resource reads) can return
            // unbounded content — cap it here so the token budget is not
            // silently swallowed. The chat-stream event carries the SAME
            // capped text (the frontend renders what the model saw; the
            // full raw output never crosses IPC unbounded).
            let capped = crate::core::str_util::spill_tool_output(&content);
            emit_stream(
                app,
                StreamEvent::ToolCallResult {
                    turn_id: turn_id.to_string(),
                    call_id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    result: capped.clone(),
                    is_error,
                },
            );
            chat_state.push_tool_result(&tool_call.id, &capped, is_error);
            // Only embed the image when the main model natively accepts
            // images — a text-only model (DeepSeek) would reject an
            // image_url/input_image block with HTTP 400. Such models rely on
            // the automatic transcription pipeline instead.
            if let Some(img) = image {
                if can_see_images {
                    chat_state.push_transient_image(img.media_type, img.data);
                }
            }

            // MCP Apps: surface the interactive UI payload (emitted after the
            // text result — the frontend keys both on call_id). The serial
            // path runs `use_tool` (which forwards to MCP servers), so it must
            // carry the payload exactly as the concurrent path does — dropping
            // `outcome.app` here would silently discard the UI.
            if let Some(app_json) = mcp_app {
                if let (Some(server), Some(resource_uri), Some(html), Some(app_is_error)) = (
                    app_json.get("server").and_then(|v| v.as_str()),
                    app_json.get("resource_uri").and_then(|v| v.as_str()),
                    app_json.get("html").and_then(|v| v.as_str()),
                    app_json.get("is_error").and_then(|v| v.as_bool()),
                ) {
                    emit_stream(
                        app,
                        StreamEvent::McpApp {
                            turn_id: turn_id.to_string(),
                            call_id: tool_call.id.clone(),
                            name: tool_call.name.clone(),
                            server: server.to_string(),
                            resource_uri: resource_uri.to_string(),
                            html: html.to_string(),
                            is_error: app_is_error,
                            csp: app_json.get("csp").cloned(),
                        },
                    );
                }
            }

            record_tool_usage(app, session_id, turn, &tool_call.name, &content).await;

            // CRITICAL: Add a transient reminder to prevent duplicate summaries
            // This addresses DeepSeek's tendency to re-summarize after tool calls.
            // Transient = reaches the model this turn but never persists.
            chat_state.push_transient_system(
                "[SYSTEM REMINDER] You have just received tool results. \
                DO NOT summarize the project/file again if you already did so earlier in this response. \
                This applies ACROSS turns too: if a previous turn already reported the final result of this task, \
                do NOT produce another full report, table, or checklist of it — only state what changed. \
                If you already provided an overview or summary, continue directly to answering the user's question or taking the next action. \
                DO NOT start your response with phrases like 'Based on the file content' or 'Now I understand'. \
                Simply answer concisely.".to_string()
            );
        }
        Ok(permission_denials)
    }
}
