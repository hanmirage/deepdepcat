//! Background subagent result injection — worker completions surface into
//! the parent conversation as transient system messages and edit evidence.

use super::AgentLoop;
use crate::agent::chat_state::ChatState;
use tauri::{AppHandle, Manager};
use tracing::info;

impl AgentLoop {
    /// Drain completed background subagent results into the parent
    /// conversation. Worker-written files surface to the session's edit
    /// evidence; harness-internal completions (surface_completion = false)
    /// become a transient interjection instead of polluting the history.
    pub(super) async fn inject_background_results(
        &self,
        app: &AppHandle,
        session_id: &str,
        chat_state: &mut ChatState,
    ) {
        let bg_results = {
            let state = app.state::<crate::bootstrap::AppState>();
            state.coordinator.drain_background_results(session_id).await
        };
        if bg_results.is_empty() {
            return;
        }
        for bg in &bg_results {
            // Worker-written files surface to the session's edit evidence: a
            // background subagent's successful writes are real changes the
            // acceptance gates must know about.
            for f in &bg.result.modified_files {
                chat_state.record_edited_path(f.clone());
            }
            // Harness-internal subagents (surface_completion = false) are
            // collected but never injected into the parent's conversation —
            // the parent must not see them. They are surfaced as a transient
            // interjection instead, so the parent knows work is progressing
            // without polluting conversation history.
            if !bg.surface_completion {
                let status = if bg.result.success {
                    "completed"
                } else {
                    "failed"
                };
                self.register_interjection(
                    crate::agent::interjection::Interjection::new(
                        "background",
                        crate::agent::interjection::InterjectionPriority::High,
                        format!(
                            "A background subagent {status}: {}",
                            bg.task.chars().take(120).collect::<String>()
                        ),
                    )
                    .with_dedup_key(format!("bg:{}", bg.task_id)),
                )
                .await;
                info!(
                    task_id = %bg.task_id,
                    success = bg.result.success,
                    "Background subagent result collected (not surfaced)"
                );
                continue;
            }
            let response = if bg.result.success {
                bg.result.response.clone()
            } else {
                bg.result
                    .error
                    .clone()
                    .unwrap_or_else(|| "unknown error".to_string())
            };
            // Inject as a structured <task-notification> so the model can
            // parse the completion reliably.
            let notification = crate::agent::notification::from_background_result(
                &bg.task_id,
                &bg.task,
                bg.result.success,
                &response,
            );
            chat_state.push_transient_system(notification.to_xml());
            info!(
                task_id = %bg.task_id,
                success = bg.result.success,
                "Background subagent result injected"
            );
        }
    }
}
