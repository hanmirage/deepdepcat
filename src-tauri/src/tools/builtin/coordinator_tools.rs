//! Coordinator worker tools — send messages to and stop running workers.
//!
//! The coordinator's `send_message` and `task_stop` tools let the parent
//! agent interact with workers mid-flight:
//! - `send_message` queues a follow-up instruction for a running worker.
//! - `task_stop` cancels a running worker's turn immediately.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use crate::bootstrap::AppState;
use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Manager;

// ── send_message ─────────────────────────────────────────────

/// Tool for sending a follow-up message to a running worker.
pub struct SendMessageTool;

impl SendMessageTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        "send_message"
    }

    fn description(&self) -> &str {
        "Send a follow-up message to a running worker subagent. \
         Use when a worker needs additional guidance, more context, \
         or a course correction while it is still executing."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "worker_id": {
                    "type": "string",
                    "description": "The ID of the running worker to message"
                },
                "message": {
                    "type": "string",
                    "description": "The follow-up instruction to send"
                }
            },
            "required": ["worker_id", "message"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let worker_id = args
            .get("worker_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'worker_id'".into()))?;
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'message'".into()))?;

        let state = context.app.state::<AppState>();
        if state
            .coordinator
            .send_worker_message(worker_id, message)
            .await
        {
            Ok(ToolResult::success(format!(
                "Message queued for worker {worker_id}."
            )))
        } else {
            Ok(ToolResult::error(format!(
                "Worker {worker_id} not found or not running."
            )))
        }
    }
}

// ── task_stop ────────────────────────────────────────────────

/// Tool for stopping a running worker subagent.
pub struct TaskStopTool;

impl TaskStopTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for TaskStopTool {
    fn name(&self) -> &str {
        "task_stop"
    }

    fn description(&self) -> &str {
        "Stop a running worker subagent immediately. \
         Use when a worker is going in the wrong direction, \
         stuck in a loop, or no longer needed."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "worker_id": {
                    "type": "string",
                    "description": "The ID of the worker to stop"
                }
            },
            "required": ["worker_id"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let worker_id = args
            .get("worker_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'worker_id'".into()))?;

        let state = context.app.state::<AppState>();
        if state.coordinator.stop_worker(worker_id).await {
            Ok(ToolResult::success(format!(
                "Stop signal sent to worker {worker_id}."
            )))
        } else {
            Ok(ToolResult::error(format!(
                "Worker {worker_id} not found or not running."
            )))
        }
    }
}
