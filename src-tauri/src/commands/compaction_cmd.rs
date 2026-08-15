//! Compaction commands — manual "compact now" for a session.

use crate::agent::compaction::Compactor;
use crate::bootstrap::AppState;
use crate::core::stream::emit_stream;
use crate::core::types::StreamEvent;
use crate::llm::client::LlmClient;
use crate::llm::retry::RetryConfig;
use tauri::{AppHandle, State};

/// Compact a session's conversation NOW, regardless of the token threshold.
///
/// Contract (`Result<String, String>`):
/// - `Ok("compacted:<tokens>")` — compaction ran and freed `<tokens>` tokens
///   (a `StreamEvent::Compaction` is emitted so the UI history/toast updates
///   through the normal channel).
/// - `Ok("skipped")` — nothing worth compacting (conversation too short, or
///   the summary reduction guards rejected the result).
/// - `Ok("busy")` — an agent turn currently owns the session state; manual
///   compaction is refused to avoid clobbering a running loop.
/// - `Err(msg)` — hard failure (config read, LLM summarization error).
#[tauri::command]
pub async fn force_compact(
    session_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    // Take the ChatState OUT of the session manager like send_chat_message
    // does. A checked-out state means a loop is running — refuse rather than
    // race it.
    let mut chat_state = {
        let mut sessions = state.sessions.lock().await;
        sessions
            .take_chat_state(&session_id)
            .map_err(|_| "busy".to_string())?
    };

    // Run the fallible compaction work in a closure so the put-back below
    // executes on EVERY exit path. A `?` after take_chat_state would return
    // early with the state still checked out — the session then reports
    // "busy" forever and every later send queues (never drained) until app
    // restart (the exact bug class chat.rs hardened against in #88 H1).
    let run = async {
        let config = {
            let guard = state.config().map_err(|e| e.to_string())?;
            guard.clone()
        };

        let retry_config = RetryConfig::from_llm_config(&config.llm);
        let llm_client = LlmClient::new(
            config.llm.providers.clone(),
            retry_config,
            config.llm.prompt_caching_enabled,
            state.circuit_breaker.clone(),
        );
        let compactor =
            Compactor::new(llm_client, config.agent.compaction_model.clone()).with_two_pass(70);

        let tool_defs = state.tools.definitions();
        let threshold = config.agent.auto_compact_threshold_percent;

        // PreCompaction hook — manual compaction is a lifecycle point too.
        state
            .hook_executor
            .execute_observe(
                &crate::hooks::HookContext::new(
                    crate::hooks::HookEvent::PreCompaction,
                    &session_id,
                )
                .with_data("force", serde_json::json!(true))
                .with_data("reason", serde_json::json!("manual")),
            )
            .await;
        let workspace = state.workspace.read().ok().and_then(|w| w.clone());
        // DeepSeek optimization: cache-aware compaction only for DeepSeek
        // sessions with the setting on.
        let cache_optimize = state
            .config()
            .map(|c| c.agent.deepseek_auto_reasoning)
            .unwrap_or(false)
            && chat_state.model.to_lowercase().contains("deepseek");
        let result = compactor
            .compact_if_needed(
                &mut chat_state,
                &tool_defs,
                threshold,
                true,
                cache_optimize,
                None,
                state.memory.clone(),
                workspace.as_deref(),
            )
            .await
            .map_err(|e| e.to_string())?;
        // PostCompaction hook — freed tokens (0 when skipped).
        state
            .hook_executor
            .execute_observe(
                &crate::hooks::HookContext::new(
                    crate::hooks::HookEvent::PostCompaction,
                    &session_id,
                )
                .with_data(
                    "compacted_tokens",
                    serde_json::json!(result.unwrap_or(0)),
                )
                .with_data("reason", serde_json::json!("manual")),
            )
            .await;
        Ok::<Option<u64>, String>(result)
    }
    .await;

    // Put the state back and persist — ALWAYS, including error paths (the
    // compactor may still have snipped stale tool results, and a leaked
    // checkout bricks the session).
    {
        let mut sessions = state.sessions.lock().await;
        let _ = sessions.put_chat_state(&session_id, chat_state);
        let _ = sessions.persist_session(&session_id);
        let _ = sessions.persist_messages(&session_id);
    }

    let result = run?;

    match result {
        Some(compacted_tokens) => {
            emit_stream(
                &app,
                StreamEvent::Compaction {
                    session_id: session_id.clone(),
                    compacted_tokens,
                    summary: "Manual compaction".to_string(),
                },
            );
            Ok(format!("compacted:{compacted_tokens}"))
        }
        None => Ok("skipped".to_string()),
    }
}
