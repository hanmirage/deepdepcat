//! Concurrent tool execution — one concurrency-safe call at a time.

use super::super::AgentLoop;
use crate::core::stream::emit_stream;
use crate::core::types::{emit_debug_trace, DebugEvent, StreamEvent, ToolCall};
use crate::hooks::{HookContext, HookEvent};
use crate::skills::{extract_file_path, is_file_tool};
use serde_json::json;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::support::*;
impl AgentLoop {
    /// Execute a single concurrency-safe tool call without touching
    /// `chat_state` (results are collected and pushed sequentially later).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_single_concurrent(
        &self,
        tool_call: &ToolCall,
        app: &AppHandle,
        session_id: &str,
        turn_id: &str,
        cancellation_token: &CancellationToken,
        debug_mode: bool,
        conversation: Vec<crate::core::types::ConversationItem>,
        model: String,
        provider: Option<String>,
        attached_images: Vec<(String, String)>,
    ) -> BatchToolResult {
        if cancellation_token.is_cancelled() {
            return BatchToolResult {
                call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                content: "Cancelled".to_string(),
                is_error: true,
                permission_denied: false,
                image: None,
                app: None,
                args: json!({}),
                arguments: tool_call.arguments.clone(),
                hook_blocked: false,
                hook_contexts: Vec::new(),
            };
        }

        // Parse arguments without touching chat_state.
        let mut tool_args = if tool_call.arguments.trim().is_empty() {
            json!({})
        } else {
            match tool_call.parse_arguments() {
                Ok(v) => v,
                Err(e) => {
                    warn!(tool = %tool_call.name, error = %e, "Invalid JSON arguments");
                    return BatchToolResult {
                        call_id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        content: format!("Invalid JSON arguments: {}", e),
                        is_error: true,
                        permission_denied: false,
                        image: None,
                        app: None,
                        args: json!({}),
                        arguments: tool_call.arguments.clone(),
                        hook_blocked: false,
                        hook_contexts: Vec::new(),
                    };
                }
            }
        };

        // PreToolUse hook gate
        let pre_ctx = HookContext::new(HookEvent::PreToolUse, session_id)
            .with_tool(tool_call.name.as_str(), tool_args.clone());
        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::hook_trigger(session_id, "PreToolUse"),
        );
        let mut hook_contexts = Vec::new();
        let mut rewritten = false;
        match self.hook_executor.execute_gate(&pre_ctx).await {
            Ok(outcome) => {
                hook_contexts = outcome.additional_context;
                if let Some(updated) = outcome.updated_input {
                    tracing::info!(
                        tool = %tool_call.name,
                        "PreToolUse hook rewrote tool input (concurrent)"
                    );
                    tool_args = updated;
                    rewritten = true;
                }
            }
            Err(reason) => {
                warn!(tool = %tool_call.name, reason = %reason, "Tool blocked by PreToolUse hook");
                crate::observability::event_log::record(
                    app,
                    session_id,
                    Some(turn_id),
                    "tool_run",
                    serde_json::json!({
                        "tool": &tool_call.name,
                        "call_id": &tool_call.id,
                        "is_error": true,
                        "hook_blocked": true,
                        "args_len": tool_call.arguments.len(),
                        "result_len": 0,
                    }),
                );
                return BatchToolResult {
                    call_id: tool_call.id.clone(),
                    name: tool_call.name.clone(),
                    content: format!("Blocked by hook: {}", reason),
                    is_error: true,
                    permission_denied: false,
                    image: None,
                    app: None,
                    args: tool_args,
                    arguments: tool_call.arguments.clone(),
                    hook_blocked: true,
                    hook_contexts,
                };
            }
        }

        // Tool dispatch
        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::tool_dispatch(session_id, &tool_call.name, &tool_call.arguments),
        );
        // Dispatch the REWRITTEN call when a hook changed the input — the
        // permission system and tool see the safe variant; the original
        // arguments remain in `tool_call` for audit and guard purposes.
        let rewritten_call = if rewritten {
            let mut c = tool_call.clone();
            c.arguments = tool_args.to_string();
            Some(c)
        } else {
            None
        };
        let dispatch_call: &ToolCall = rewritten_call.as_ref().map_or(tool_call, |c| c);
        // Cancellation DURING a running tool: select against the token so a
        // user stop drops the tool future mid-flight — for bash the dropped
        // `Child` (kill_on_drop) kills the process tree instead of waiting
        // out the command's own timeout (up to minutes). The dispatcher awaits
        // the tool directly (no spawn), so dropping its future propagates.
        let ran = tokio::select! {
            r = self.tool_dispatcher.execute(
                dispatch_call,
                app,
                session_id,
                turn_id,
                &conversation,
                model,
                provider,
                attached_images,
            ) => Some(r),
            _ = cancellation_token.cancelled() => None,
        };
        let Some(result) = ran else {
            warn!(tool = %tool_call.name, "Tool cancelled mid-execution — dropping future");
            return BatchToolResult {
                call_id: tool_call.id.clone(),
                name: tool_call.name.clone(),
                content: "Cancelled by user mid-execution".to_string(),
                is_error: true,
                permission_denied: false,
                image: None,
                app: None,
                args: json!({}),
                arguments: tool_call.arguments.clone(),
                hook_blocked: false,
                hook_contexts,
            };
        };
        let (content, is_error, permission_denied, image, mcp_app) = match result {
            Ok(outcome) => (
                outcome.content,
                outcome.is_error,
                false,
                outcome.image,
                outcome.app,
            ),
            Err(e) => {
                let denied = e.is_permission_denied();
                record_tool_diagnostic(app, &tool_call.name, &e);
                warn!(tool = %tool_call.name, error = %e, "Tool failed");
                // Dispatch-level failures get a recovery hint (content-level
                // failures were already hinted inside dispatch).
                let content = e.to_string();
                let content = if denied {
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
                (content, true, denied, None, None)
            }
        };
        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::tool_result(session_id, &tool_call.name, 0, is_error),
        );
        // Replay-exact audit: one event per executed tool, summary-shaped
        // (lengths + status — never full args/output or secrets).
        crate::observability::event_log::record(
            app,
            session_id,
            Some(turn_id),
            "tool_run",
            serde_json::json!({
                "tool": &tool_call.name,
                "call_id": &tool_call.id,
                "is_error": is_error,
                "permission_denied": permission_denied,
                "args_len": tool_call.arguments.len(),
                "result_len": content.len(),
            }),
        );
        if permission_denied {
            crate::observability::event_log::record(
                app,
                session_id,
                Some(turn_id),
                "approval",
                serde_json::json!({
                    "tool": &tool_call.name,
                    "decision": "denied",
                    "call_id": &tool_call.id,
                }),
            );
        }

        BatchToolResult {
            call_id: tool_call.id.clone(),
            name: tool_call.name.clone(),
            content,
            is_error,
            permission_denied,
            image,
            app: mcp_app,
            args: tool_args,
            arguments: tool_call.arguments.clone(),
            hook_blocked: false,
            hook_contexts,
        }
    }

    /// Emit a blocked result and push it to chat_state.
    pub(crate) fn emit_blocked_result(
        &self,
        app: &AppHandle,
        turn_id: &str,
        chat_state: &mut crate::agent::chat_state::ChatState,
        result: &BatchToolResult,
    ) {
        emit_stream(
            app,
            StreamEvent::ToolCallResult {
                turn_id: turn_id.to_string(),
                call_id: result.call_id.clone(),
                name: result.name.clone(),
                result: result.content.clone(),
                is_error: true,
            },
        );
        chat_state.push_tool_result(
            &result.call_id,
            crate::core::str_util::spill_tool_output(&result.content),
            true,
        );
    }

    /// Emit a completed result and push it to chat_state.
    pub(crate) fn emit_and_push_result(
        &self,
        app: &AppHandle,
        turn_id: &str,
        chat_state: &mut crate::agent::chat_state::ChatState,
        result: &BatchToolResult,
        can_see_images: bool,
    ) {
        // Cap the event payload with the same guard as the conversation
        // injection — parallel-path results are never emitted unbounded.
        let capped = crate::core::str_util::spill_tool_output(&result.content);
        emit_stream(
            app,
            StreamEvent::ToolCallResult {
                turn_id: turn_id.to_string(),
                call_id: result.call_id.clone(),
                name: result.name.clone(),
                result: capped.clone(),
                is_error: result.is_error,
            },
        );
        chat_state.push_tool_result(&result.call_id, capped, result.is_error);
        if let Some(img) = &result.image {
            if can_see_images {
                chat_state.push_transient_image(img.media_type.clone(), img.data.clone());
            }
        }
        // MCP Apps: surface the interactive UI payload (emitted after the
        // text result — the frontend keys both on call_id).
        if let Some(app_json) = &result.app {
            if let (Some(server), Some(resource_uri), Some(html), Some(is_error)) = (
                app_json.get("server").and_then(|v| v.as_str()),
                app_json.get("resource_uri").and_then(|v| v.as_str()),
                app_json.get("html").and_then(|v| v.as_str()),
                app_json.get("is_error").and_then(|v| v.as_bool()),
            ) {
                emit_stream(
                    app,
                    StreamEvent::McpApp {
                        turn_id: turn_id.to_string(),
                        call_id: result.call_id.clone(),
                        name: result.name.clone(),
                        server: server.to_string(),
                        resource_uri: resource_uri.to_string(),
                        html: html.to_string(),
                        is_error,
                        csp: app_json.get("csp").cloned(),
                    },
                );
            }
        }
    }

    /// Pull LSP diagnostics for a just-edited file from an ALREADY-RUNNING
    /// language server. Returns `None` when no server is running for the
    /// workspace or the pull fails — the loop must never stall or fail over
    /// missing auto-verification. The formatted note (same `[severity]
    /// file:line: message` shape as the `lsp` tool) is appended to the tool
    /// result so the model sees type-check feedback without an extra call.
    pub(crate) async fn pull_auto_diagnostics(&self, app: &AppHandle, path: &str) -> Option<String> {
        let workspace = self.context_builder.workspace()?;
        let state = app.state::<crate::bootstrap::AppState>();
        // get() never spawns a server — only reuse one the model (or a
        // previous turn) already started.
        let client = state.lsp_manager.get(&workspace)?;
        let resolved = crate::tools::builtin::resolve_path(Some(&workspace), path);
        let language_id = crate::tools::builtin::lsp::client::language_id_for_path(&resolved);
        let diags = tokio::time::timeout(
            std::time::Duration::from_secs(12),
            client.diagnostics(&resolved, language_id),
        )
        .await
        .ok()?
        .ok()?;
        if diags.is_empty() {
            return Some("[auto-lsp-diagnostics] clean — no diagnostics".to_string());
        }
        let mut lines: Vec<String> = diags
            .iter()
            .take(20)
            .map(|d| format!("[{}] {}:{}: {}", d.severity, d.file, d.line, d.message))
            .collect();
        lines.insert(0, "[auto-lsp-diagnostics]".to_string());
        if diags.len() > 20 {
            lines.push(format!("... {} more", diags.len() - 20));
        }
        Some(lines.join("\n"))
    }

    /// Record file touch for skill activation.
    pub(crate) async fn track_skill_file_touch(
        &self,
        engine: &crate::skills::activation::SkillActivationEngine,
        name: &str,
        args: &serde_json::Value,
    ) {
        if is_file_tool(name) {
            if let Some(path_str) = extract_file_path(name, args) {
                engine
                    .record_file_touch(std::path::Path::new(&path_str))
                    .await;
            }
        }
    }

    /// Parse tool call arguments into a JSON value, feeding errors back to the LLM on failure.
    ///
    /// Returns `None` when parsing failed — the error has already been
    /// emitted and pushed as a single tool result for this call. Callers
    /// MUST skip execution when `None` is returned (a second result for the
    /// same `call_id` would corrupt the conversation structure).
    pub(crate) async fn parse_tool_args(
        &self,
        tool_call: &ToolCall,
        app: &AppHandle,
        turn_id: &str,
        chat_state: &mut crate::agent::chat_state::ChatState,
    ) -> Option<serde_json::Value> {
        if tool_call.arguments.trim().is_empty() {
            return Some(json!({}));
        }
        match tool_call.parse_arguments() {
            Ok(v) => Some(v),
            Err(e) => {
                warn!(
                    tool = %tool_call.name,
                    error = %e,
                    "Invalid JSON arguments for tool — feeding error back to LLM"
                );
                // The message deliberately excludes the raw arguments — they
                // may carry secrets, and this text is fed back to the LLM.
                let msg = format!("Invalid JSON arguments: {}", e);
                emit_stream(
                    app,
                    StreamEvent::ToolCallResult {
                        turn_id: turn_id.to_string(),
                        call_id: tool_call.id.clone(),
                        name: tool_call.name.clone(),
                        result: msg.clone(),
                        is_error: true,
                    },
                );
                chat_state.push_tool_result(
                    &tool_call.id,
                    crate::core::str_util::spill_tool_output(&msg),
                    true,
                );
                None
            }
        }
    }
}
