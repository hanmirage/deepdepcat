//! Ask user tool — prompts the user for input or decisions.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use crate::bootstrap::AppState;
use crate::hooks::{HookContext, HookEvent};
use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::{Emitter, Manager};

pub struct AskUserTool;

impl AskUserTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        "Ask the user a question and wait for their response. Use when you need clarification, confirmation, or additional information."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "options": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional list of choices for the user to select from"
                }
            },
            "required": ["question"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        // Unattended scheduled runs have no human to answer — refuse with a
        // deterministic result so the model reports the ambiguity instead of
        // parking a 5-minute pending interaction.
        let state = context.app.state::<AppState>();
        if state.is_unattended(&context.session_id).await {
            return Ok(ToolResult::error(
                "无人值守（定时任务）：无法询问用户。请在结果中说明歧义并继续。".to_string(),
            ));
        }

        let question = args
            .get("question")
            .and_then(|q| q.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'question'".into()))?;
        let options: Vec<String> = args
            .get("options")
            .and_then(|o| o.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let request_id = crate::core::ids::generate_id();

        let payload = json!({
            "request_id": request_id,
            "session_id": context.session_id,
            "question": question,
            "options": options,
        });

        let (tx, rx) = tokio::sync::oneshot::channel();

        // Register the pending user input request with AppState
        state.register_user_input_request(&request_id, tx).await;
        state
            .register_pending_interaction(
                &context.session_id,
                "question",
                &request_id,
                question.to_string(),
            )
            .await;
        // UserInputRequested hook — the loop is now waiting on a human;
        // observers (audit logs, status dashboards) see the question.
        state
            .hook_executor
            .execute_observe(
                &HookContext::new(HookEvent::UserInputRequested, &context.session_id)
                    .with_data("request_id", json!(request_id))
                    .with_data("question", json!(question)),
            )
            .await;
        crate::permissions::plan::broadcast_pending_interactions(&context.app, &context.session_id)
            .await;

        context.app.emit("ask-user", payload)?;

        // Wait for the user's response (5 minute timeout)
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(300), rx).await;
        let result = match outcome {
            Ok(Ok(response)) => {
                // The user's answer enters the model's context — sanitize it
                // so it cannot forge system frames or placeholder variables.
                Ok(ToolResult::success(
                    crate::agent::sanitize::sanitize_injection_slot(&response),
                ))
            }
            Ok(Err(_)) => {
                // Channel closed without a response — the parked request
                // entry must not leak (no one will ever answer it).
                state.remove_user_input_request(&request_id).await;
                Ok(ToolResult::error(
                    "User input channel closed without response".to_string(),
                ))
            }
            Err(_) => {
                // Timed out — the parked request entry must not leak (the
                // frontend card is gone; a late response would be dead).
                state.remove_user_input_request(&request_id).await;
                Ok(ToolResult::error(
                    "Timed out waiting for user response (5 minutes)".to_string(),
                ))
            }
        };
        state
            .resolve_pending_interaction(&context.session_id, &request_id)
            .await;
        crate::permissions::plan::broadcast_pending_interactions(&context.app, &context.session_id)
            .await;
        result
    }
}
