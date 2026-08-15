//! wait_tasks tool — polls a background task's output incrementally.
//!
//! Background tasks spawned by `bash` with `background: true` run detached
//! and stream stdout/stderr to a per-task log file. This tool reads the log
//! from a caller-supplied byte offset (stateless across calls — the model
//! passes back the returned `output_offset` to resume), waiting up to a
//! timeout for new output.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use crate::tools::background::BackgroundTaskRegistry;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// Tool for waiting on / polling background task output.
pub struct WaitTasksTool {
    background_tasks: Arc<BackgroundTaskRegistry>,
}

impl WaitTasksTool {
    pub fn new(background_tasks: Arc<BackgroundTaskRegistry>) -> Self {
        Self { background_tasks }
    }
}

#[async_trait]
impl Tool for WaitTasksTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Code
    }
    fn name(&self) -> &str {
        "wait_tasks"
    }

    fn description(&self) -> &str {
        "Wait for or poll a background task's output (started by bash with background=true). \
         Returns new output since the given offset plus the new offset to resume from. \
         Poll repeatedly with the returned output_offset until status is completed/failed/killed."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The background task ID returned by bash with background=true"
                },
                "output_offset": {
                    "type": "integer",
                    "description": "Byte offset to read from. Pass the output_offset returned by the previous call. Defaults to 0."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Maximum time to wait for new output in milliseconds. Defaults to 30000."
                },
                "max_chars": {
                    "type": "integer",
                    "description": "Maximum characters of output to return in this call. Defaults to 8000."
                }
            },
            "required": ["task_id"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, _context: &ToolContext) -> AppResult<ToolResult> {
        let task_id = args
            .get("task_id")
            .and_then(|t| t.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'task_id'".into()))?;

        if self.background_tasks.get(task_id).is_none() {
            return Ok(ToolResult::error(format!(
                "Unknown task ID '{task_id}'. Background tasks are started by bash with background=true."
            )));
        }

        let output_offset = args
            .get("output_offset")
            .and_then(|o| o.as_u64())
            .unwrap_or(0);
        let timeout_ms = args
            .get("timeout_ms")
            .and_then(|t| t.as_u64())
            .unwrap_or(30_000)
            .min(120_000);
        let max_chars = args
            .get("max_chars")
            .and_then(|m| m.as_u64())
            .unwrap_or(8000)
            .min(32_000) as usize;

        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut offset = output_offset;

        // First immediate read, then poll until the task finishes or the
        // deadline passes.
        let (mut output, mut new_offset, _done) = self.read_chunk(task_id, offset, max_chars);
        if output.is_empty() {
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let (chunk, next_offset, task_done) = self.read_chunk(task_id, offset, max_chars);
                if !chunk.is_empty() {
                    output = chunk;
                    new_offset = next_offset;
                    break;
                }
                if task_done {
                    break;
                }
            }
        }
        offset = new_offset;

        let task = self
            .background_tasks
            .get(task_id)
            .ok_or_else(|| AppError::TaskNotFound(task_id.to_string()))?;
        let status = task.status.clone();
        let exit_code = task.exit_code;

        Ok(ToolResult::success(
            json!({
                "task_id": task_id,
                "status": status,
                "exit_code": exit_code,
                "completed": !task.is_running(),
                "output": output,
                "output_offset": offset,
            })
            .to_string(),
        ))
    }
}

impl WaitTasksTool {
    /// Read one chunk of output for a task, returning (content, new offset, done).
    fn read_chunk(&self, task_id: &str, offset: u64, max_chars: usize) -> (String, u64, bool) {
        match self
            .background_tasks
            .read_output(task_id, offset, max_chars)
        {
            Some(chunk) => (chunk.content, chunk.offset, chunk.done),
            None => (String::new(), offset, true),
        }
    }
}
