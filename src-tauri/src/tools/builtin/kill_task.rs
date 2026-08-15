//! Kill task tool — terminates a running background task by its ID.
//!
//! Background tasks are spawned by the `bash` tool's `background: true`
//! mode and tracked in the shared [`BackgroundTaskRegistry`]. This tool
//! looks the task up in that registry and actually terminates the
//! process tree.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use crate::tools::background::BackgroundTaskRegistry;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::Manager;

/// Tool for killing a background task.
pub struct KillTaskTool {
    registry: Arc<BackgroundTaskRegistry>,
}

impl KillTaskTool {
    pub fn new(registry: Arc<BackgroundTaskRegistry>) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for KillTaskTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Code
    }
    fn name(&self) -> &str {
        "kill_task"
    }

    fn description(&self) -> &str {
        "Kill a running background task by its task ID (returned by bash with background=true). \
        Use this to stop long-running commands that were started in the background."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The ID of the task to kill"
                }
            },
            "required": ["task_id"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Side-effecting — never run in parallel with other tools.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> AppResult<ToolResult> {
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::core::error::AppError::ToolNotFound("missing 'task_id'".into())
            })?;

        let running = self.registry.list(&ctx.session_id);
        let known = running.iter().find(|t| t.id == task_id);
        let known_pid = known.map(|t| t.pid);

        match self.registry.kill(task_id) {
            Ok(true) => {
                // Fire the TaskUpdated observe hook so external tooling can
                // react to the task being killed.
                use crate::hooks::{HookContext, HookEvent};
                let hook_ctx = HookContext::new(HookEvent::TaskUpdated, ctx.session_id.clone())
                    .with_data("task_id", serde_json::json!(task_id))
                    .with_data("status", serde_json::json!("killed"));
                let executor = crate::hooks::HookExecutor::new(
                    ctx.app
                        .state::<crate::bootstrap::AppState>()
                        .hooks
                        .clone(),
                );
                executor.execute_observe(&hook_ctx).await;
                Ok(ToolResult::success(format!(
                    "Task {task_id} (PID {}) has been terminated.",
                    known_pid.unwrap_or(0)
                )))
            }
            Ok(false) => Ok(ToolResult::error(format!(
                "Unknown task ID '{task_id}'. No matching background task. \
                 Background tasks are started by bash with background=true, \
                 which returns the task ID."
            ))),
            Err(e) => Ok(ToolResult::error(format!("Failed to kill task: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_name() {
        let tool = KillTaskTool::new(Arc::new(BackgroundTaskRegistry::new()));
        assert_eq!(tool.name(), "kill_task");
    }

    #[test]
    fn tool_not_read_only() {
        let tool = KillTaskTool::new(Arc::new(BackgroundTaskRegistry::new()));
        assert!(!tool.is_read_only());
    }
}
