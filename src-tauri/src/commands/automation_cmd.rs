//! Automation commands — the Scheduled (定时任务) API for the frontend.

use crate::automation::{AutomationRunner, ScheduleSpec, ScheduledRun, ScheduledTask};
use crate::bootstrap::AppState;
use chrono::Utc;
use tauri::{AppHandle, State};

/// List all scheduled tasks, newest first.
#[tauri::command]
pub fn list_scheduled_tasks(
    state: State<'_, AppState>,
) -> Result<Vec<ScheduledTask>, String> {
    Ok(state.automation_store.list_tasks())
}

/// Create a scheduled agent task.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn create_scheduled_task(
    name: String,
    prompt: String,
    schedule_kind: String,
    every_secs: Option<i64>,
    daily_time: Option<String>,
    project_path: Option<String>,
    use_worktree: Option<bool>,
    work_mode: Option<String>,
    model: Option<String>,
    persistent: Option<bool>,
    state: State<'_, AppState>,
) -> Result<ScheduledTask, String> {
    let schedule = ScheduleSpec::parse(
        &schedule_kind,
        every_secs.unwrap_or(0),
        daily_time.as_deref().unwrap_or(""),
    )?;
    let use_worktree = use_worktree.unwrap_or(false);
    let persistent = persistent.unwrap_or(false);
    if persistent && use_worktree {
        return Err("常驻模式不能使用 worktree — 常驻 agent 的文件累积在项目里，每次独立 worktree 无法延续".to_string());
    }
    let now = Utc::now();
    let task = ScheduledTask {
        id: crate::core::ids::generate_id(),
        name,
        prompt,
        schedule,
        project_path: project_path.unwrap_or_default(),
        use_worktree,
        work_mode: normalize_work_mode(work_mode.as_deref()),
        model: model.unwrap_or_default(),
        persistent,
        persistent_session_id: None,
        active: true,
        last_run_at_ms: None,
        run_count: 0,
        created_at: now,
        updated_at: now,
    };
    state.automation_store.upsert_task(&task)?;
    Ok(task)
}

/// Update a scheduled task (partial update; `None` keeps the old value).
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn update_scheduled_task(
    id: String,
    name: Option<String>,
    prompt: Option<String>,
    schedule_kind: Option<String>,
    every_secs: Option<i64>,
    daily_time: Option<String>,
    project_path: Option<String>,
    use_worktree: Option<bool>,
    work_mode: Option<String>,
    model: Option<String>,
    active: Option<bool>,
    persistent: Option<bool>,
    state: State<'_, AppState>,
) -> Result<ScheduledTask, String> {
    let mut task = state
        .automation_store
        .get_task(&id)
        .ok_or_else(|| format!("定时任务不存在: {id}"))?;
    if let Some(v) = name {
        task.name = v;
    }
    if let Some(v) = prompt {
        task.prompt = v;
    }
    if let Some(kind) = schedule_kind {
        task.schedule = ScheduleSpec::parse(
            &kind,
            every_secs.unwrap_or(0),
            daily_time.as_deref().unwrap_or(""),
        )?;
    }
    if let Some(v) = project_path {
        task.project_path = v;
    }
    if let Some(v) = use_worktree {
        task.use_worktree = v;
    }
    if let Some(v) = work_mode {
        task.work_mode = normalize_work_mode(Some(&v));
    }
    if let Some(v) = model {
        task.model = v;
    }
    if let Some(v) = active {
        task.active = v;
    }
    if let Some(v) = persistent {
        task.persistent = v;
    }
    if task.persistent && task.use_worktree {
        return Err("常驻模式不能使用 worktree — 常驻 agent 的文件累积在项目里，每次独立 worktree 无法延续".to_string());
    }
    task.updated_at = Utc::now();
    state.automation_store.upsert_task(&task)?;
    Ok(task)
}

/// Delete a task and its run history (runs cascade).
#[tauri::command]
pub fn delete_scheduled_task(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.automation_store.delete_task(&id)
}

/// List run history (all runs, or one task's runs).
#[tauri::command]
pub fn list_scheduled_runs(
    task_id: Option<String>,
    limit: Option<i64>,
    state: State<'_, AppState>,
) -> Result<Vec<ScheduledRun>, String> {
    Ok(state
        .automation_store
        .list_runs(task_id.as_deref(), limit.unwrap_or(50)))
}

/// Delete a run row (the session transcript stays).
#[tauri::command]
pub fn delete_scheduled_run(run_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.automation_store.delete_run(&run_id)
}

/// Trigger a scheduled task immediately.
#[tauri::command]
pub async fn run_scheduled_task_now(
    task_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    AutomationRunner::new(
        state.automation_store.clone(),
        (*state).clone(),
        state.automation_running.clone(),
    )
    .run_task_now(&app, &task_id)
    .await
}

/// Cancel a scheduled run (running sessions are interrupted).
#[tauri::command]
pub async fn cancel_scheduled_run(
    run_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    AutomationRunner::new(
        state.automation_store.clone(),
        (*state).clone(),
        state.automation_running.clone(),
    )
    .cancel_run(&run_id)
    .await
}

/// Remove a scheduled run's leftover worktree (refuses when dirty).
#[tauri::command]
pub async fn cleanup_scheduled_worktree(
    run_id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    AutomationRunner::new(
        state.automation_store.clone(),
        (*state).clone(),
        state.automation_running.clone(),
    )
    .cleanup_worktree(&run_id)
    .await
}

fn normalize_work_mode(mode: Option<&str>) -> String {
    match mode {
        Some("depwork") => "depwork".to_string(),
        _ => "code".to_string(),
    }
}
