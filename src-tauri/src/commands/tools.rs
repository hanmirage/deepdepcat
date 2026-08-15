//! Tool commands — worker and background-task management.

use crate::agent::multi_agent::WorkerState;
use crate::bootstrap::AppState;
use tauri::State;

/// List all tracked subagent workers with their lifecycle state.
///
/// Powers the "子任务活动" panel: every spawned subagent is recorded by the
/// coordinator state machine, so the frontend can show running/completed/
/// failed workers without holding events in memory.
#[tauri::command]
pub async fn list_active_workers(state: State<'_, AppState>) -> Result<Vec<WorkerState>, String> {
    Ok(state.coordinator.list_active_workers().await)
}

/// List background tasks (bash background:true) for a session.
#[tauri::command]
pub async fn list_background_tasks(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<crate::tools::background::BackgroundTask>, String> {
    Ok(state.background_tasks.list(&session_id))
}

/// Read new output of a background task from a byte offset.
#[tauri::command]
pub async fn read_task_output(
    task_id: String,
    offset: u64,
    max_bytes: usize,
    state: State<'_, AppState>,
) -> Result<Option<crate::tools::background::TaskOutputChunk>, String> {
    Ok(state
        .background_tasks
        .read_output(&task_id, offset, max_bytes))
}

/// Kill a background task by ID (from the task panel).
#[tauri::command]
pub async fn kill_background_task(
    task_id: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    state
        .background_tasks
        .kill(&task_id)
        .map_err(|e| e.to_string())
}
