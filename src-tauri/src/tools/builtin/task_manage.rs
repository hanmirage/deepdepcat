//! Task management tool — create, update, and track todo items.
//!
//! Backed by the shared `TaskManager` so tasks the agent creates here are
//! visible in the frontend task list (and vice versa: tasks the user creates
//! in the sidebar are listable/updatable by the agent). The wire format the
//! model sees (`TaskItem`: id/content/status/active) is unchanged — this
//! tool was previously an unregistered in-memory duplicate.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use crate::core::types::{TaskStatus, TaskType};
use crate::task::TaskManager;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tauri::Emitter;

pub struct TaskManageTool {
    manager: Arc<TaskManager>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TaskItem {
    id: String,
    content: String,
    status: String, // "pending", "in_progress", "completed", "cancelled"
    active: bool,
}

impl TaskManageTool {
    pub fn new(manager: Arc<TaskManager>) -> Self {
        Self { manager }
    }

    /// Snapshot of THIS session's tasks in the model-facing wire shape.
    /// Session scoping is what keeps one agent's task list (and the
    /// one-active-task demotion) from clobbering another session's tasks.
    async fn snapshot(&self, session_id: &str) -> Vec<TaskItem> {
        self.manager
            .list_tasks_for_session(session_id)
            .await
            .into_iter()
            .map(|t| TaskItem {
                id: t.id,
                content: t.description,
                status: status_to_string(t.status),
                active: t.status == TaskStatus::Running,
            })
            .collect()
    }

    /// Whether `task_id` exists AND belongs to this session — foreign tasks
    /// are invisible to the agent (same rule as listing).
    async fn owns_task(&self, task_id: &str, session_id: &str) -> bool {
        self.manager
            .get_task(task_id)
            .await
            .is_some_and(|t| t.session_id.as_deref() == Some(session_id))
    }
}

fn status_to_string(status: TaskStatus) -> String {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "in_progress",
        TaskStatus::Completed => "completed",
        TaskStatus::Failed | TaskStatus::Killed => "cancelled",
    }
    .to_string()
}

fn string_to_status(s: &str) -> TaskStatus {
    match s {
        "in_progress" => TaskStatus::Running,
        "completed" => TaskStatus::Completed,
        "cancelled" => TaskStatus::Killed,
        _ => TaskStatus::Pending,
    }
}

#[async_trait]
impl Tool for TaskManageTool {
    fn name(&self) -> &str {
        "task_manage"
    }

    fn description(&self) -> &str {
        "Manage tasks/todos for the current session. Supports creating, updating, and listing tasks. Use this to track progress on multi-step work."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["create", "update", "list", "delete"],
                    "description": "The action to perform"
                },
                "id": {
                    "type": "string",
                    "description": "Task ID (for update/delete)"
                },
                "content": {
                    "type": "string",
                    "description": "Task content/description (for create)"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "cancelled"],
                    "description": "New status (for update)"
                }
            },
            "required": ["action"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Mutates the shared TaskManager (create/update/delete + the
    /// one-runner demotion pass) — never run in parallel with other tools.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        let action = args
            .get("action")
            .and_then(|a| a.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'action'".into()))?;

        match action {
            "create" => {
                let content = args
                    .get("content")
                    .and_then(|c| c.as_str())
                    .ok_or_else(|| AppError::Parse("Missing 'content' for create".into()))?;

                let task_id = self
                    .manager
                    .create_task(
                        content.to_string(),
                        TaskType::LocalWorkflow,
                        vec![],
                        Some(context.session_id.clone()),
                    )
                    .await;

                let tasks = self.snapshot(&context.session_id).await;
                let _ = context.app.emit("task-update", &tasks);

                Ok(ToolResult::success(format!(
                    "Task created: {} - {}",
                    task_id, content
                )))
            }
            "update" => {
                let id = args
                    .get("id")
                    .and_then(|i| i.as_str())
                    .ok_or_else(|| AppError::Parse("Missing 'id' for update".into()))?;
                if !self.owns_task(id, &context.session_id).await {
                    return Ok(ToolResult::error(format!("Task not found: {}", id)));
                }
                let new_status = args.get("status").and_then(|s| s.as_str());
                let found = if let Some(status) = new_status {
                    let parsed = string_to_status(status);
                    let ok = self.manager.update_task_status(id, parsed).await;

                    // Only one task can be active at a time: demote any other
                    // RUNNING task of THIS session back to pending when this
                    // one starts — other sessions' tasks are never touched.
                    if ok && parsed == TaskStatus::Running {
                        let others = self
                            .manager
                            .list_tasks_for_session(&context.session_id)
                            .await;
                        for task in others {
                            if task.id != id && task.status == TaskStatus::Running {
                                self.manager
                                    .update_task_status(&task.id, TaskStatus::Pending)
                                    .await;
                            }
                        }
                    }
                    ok
                } else {
                    // Status omitted — treat as a touch (just existence check).
                    self.manager.get_task(id).await.is_some()
                };

                let tasks = self.snapshot(&context.session_id).await;
                if found {
                    let _ = context.app.emit("task-update", &tasks);
                    Ok(ToolResult::success(format!("Task {} updated", id)))
                } else {
                    Ok(ToolResult::error(format!("Task not found: {}", id)))
                }
            }
            "list" => {
                let tasks = self.snapshot(&context.session_id).await;
                if tasks.is_empty() {
                    Ok(ToolResult::success("No tasks."))
                } else {
                    let mut output = String::from("Tasks:\n");
                    for task in tasks.iter() {
                        let status_icon = match task.status.as_str() {
                            "completed" => "[x]",
                            "in_progress" => "[>]",
                            "cancelled" => "[-]",
                            _ => "[ ]",
                        };
                        output.push_str(&format!(
                            "  {} {} - {}\n",
                            status_icon, task.id, task.content
                        ));
                    }
                    Ok(ToolResult::success(output))
                }
            }
            "delete" => {
                let id = args
                    .get("id")
                    .and_then(|i| i.as_str())
                    .ok_or_else(|| AppError::Parse("Missing 'id' for delete".into()))?;

                let deleted = if self.owns_task(id, &context.session_id).await {
                    self.manager.delete_task(id).await
                } else {
                    false
                };

                let tasks = self.snapshot(&context.session_id).await;
                if deleted {
                    let _ = context.app.emit("task-update", &tasks);
                    Ok(ToolResult::success(format!("Task {} deleted", id)))
                } else {
                    Ok(ToolResult::error(format!("Task not found: {}", id)))
                }
            }
            _ => Ok(ToolResult::error(format!("Unknown action: {}", action))),
        }
    }
}
