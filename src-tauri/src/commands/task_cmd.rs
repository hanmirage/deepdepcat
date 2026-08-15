//! Task commands — the depwork task API for the frontend.
//!
//! These commands bridge the frontend's `depworkApi` to the backend's
//! `TaskManager`. The frontend calls:
//! - `list_tasks` → list all tasks
//! - `create_task` → create a new task with description + context paths

use crate::bootstrap::AppState;
use crate::core::types::{CoworkTask, TaskType};
use tauri::State;

/// List all depwork tasks.
#[tauri::command]
pub async fn list_tasks(state: State<'_, AppState>) -> Result<Vec<CoworkTask>, String> {
    Ok(state.task_manager.list_tasks().await)
}

/// Create a new depwork task.
#[tauri::command]
pub async fn create_task(
    description: String,
    context_paths: Vec<String>,
    state: State<'_, AppState>,
) -> Result<CoworkTask, String> {
    let task_id = state
        .task_manager
        .create_task(description, TaskType::LocalWorkflow, context_paths, None)
        .await;

    state
        .task_manager
        .get_task(&task_id)
        .await
        .ok_or_else(|| format!("Task '{}' created but not found", task_id))
}
