//! Agent commands — agent status, permission mode, skills, definitions.

use crate::bootstrap::AppState;
use crate::permissions::mode::PermissionMode;
use tauri::{AppHandle, State};

/// List agent definitions for a work mode (built-in + user + project).
///
/// Filters by `work_mode` ("code"/"depwork"): definitions declared for the
/// other mode are hidden. Powers the agent customization UI and lets the
/// AgentTool's Custom agent type resolve `.deepdepcat/agents/*.md`.
#[tauri::command]
pub async fn list_agent_definitions(
    work_mode: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::agent::definition::AgentDefinition>, String> {
    let workspace = state.workspace.read().map(|w| w.clone()).unwrap_or(None);
    let mode = crate::toolkit::WorkMode::parse(work_mode.as_deref());
    Ok(crate::agent::definition::filter_by_work_mode(
        crate::agent::definition::discover_all(workspace.as_deref()),
        mode,
    ))
}

/// Get the current permission mode.
#[tauri::command]
pub async fn get_permission_mode(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.permissions.mode().as_str().to_string())
}

/// Set the permission mode.
///
/// Switching INTO full-access drains every queued permission request with an
/// allow — the user just signalled "don't ask me", so the already-parked
/// prompts are approved once (never recorded as grants). Switching to any
/// other mode leaves queued requests parked (they keep their own timeout).
#[tauri::command]
pub async fn set_permission_mode(
    mode: String,
    session_id: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let parsed = PermissionMode::from_str(&mode);
    match session_id.as_deref() {
        // Per-session scope (the input-bar combo belongs to one chat): the
        // mode is a session override AND persists to the session row — each
        // conversation owns its permission mode, Code and Depwork included.
        // ReadOnly is the exception: it is a TRANSIENT posture (a read-only
        // planning phase), so it becomes a memory-only override and is never
        // written to the session row. Persisting it was the "一直卡在计划模式"
        // bug — the session came back read-only on every restart.
        Some(sid) => {
            if parsed == PermissionMode::ReadOnly {
                state.set_session_mode(sid, parsed).await;
            } else {
                state.persist_session_mode(sid, parsed.as_str()).await;
            }
        }
        // No session id (legacy / global callers): the global default mode.
        None => {
            state.permissions.set_mode(parsed);
            crate::permissions::mode::persist_mode(&state.app_data_dir, parsed);
        }
    }
    // Mode-switch linkage: resolve every parked permission request according
    // to the NEW mode (read-only → deny all, full access → allow all,
    // accept-edits → allow edit tools). Stale dialogs must not keep the
    // agent waiting on a decision the new mode already made.
    let affected = state.resolve_permission_requests_for_mode(parsed).await;
    for session_id in affected {
        crate::permissions::plan::broadcast_pending_interactions(&app, &session_id).await;
    }
    Ok(())
}

/// Respond to a pending user input request from the ask_user tool.
#[tauri::command]
pub async fn respond_to_user_input(
    request_id: String,
    response: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    Ok(state.respond_user_input(&request_id, response).await)
}

/// List available skills.
///
/// When `work_mode` is provided ("code"/"depwork"), skills declared for the
/// other mode are hidden (a skill with empty `work_modes` is visible in all
/// modes). `None` returns the full list (management views).
#[tauri::command]
pub async fn list_skills(
    work_mode: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::skills::types::Skill>, String> {
    let workspace = state.workspace.read().map_err(|e| e.to_string())?.clone();
    // Same ecosystem compat gate as the skill engine's loader
    // (state/lifecycle.rs): a user who disabled Claude skills must
    // not see them in this list either.
    let claude_enabled = match state.config() {
        Ok(c) => c.skills.claude_enabled,
        Err(_) => true,
    };
    let loader = crate::skills::SkillLoader::new(&state.app_data_dir)
        .with_workspace(workspace)
        .with_compat(claude_enabled);
    let mut skills = loader.load_all().map_err(|e| e.to_string())?;
    if let Some(mode) = work_mode {
        // Same normalization as the rest of the mode chain (scope.rs):
        // "DEPWORK " → "depwork", unknown → "code".
        let mode = crate::toolkit::WorkMode::parse(Some(&mode))
            .as_str()
            .to_string();
        skills.retain(|s| {
            s.work_modes.is_empty() || s.work_modes.iter().any(|m| m.eq_ignore_ascii_case(&mode))
        });
    }
    Ok(skills)
}

/// Save a skill.
#[tauri::command]
pub async fn save_skill(
    skill: crate::skills::types::Skill,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let loader = crate::skills::SkillLoader::new(&state.app_data_dir);
    loader.save_skill(&skill).map_err(|e| e.to_string())
}

/// Delete a skill.
#[tauri::command]
pub async fn delete_skill(skill_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let loader = crate::skills::SkillLoader::new(&state.app_data_dir);
    loader.delete_skill(&skill_id).map_err(|e| e.to_string())
}
