//! System commands — system info, agent status, circuit breaker, etc.

use crate::bootstrap::AppState;
use crate::core::types::{AgentStatus, SystemInfo};
use serde::Serialize;
use sysinfo::System;
use tauri::State;
use tauri_plugin_fs::FsExt;

/// Port of the loopback SSE transport — the frontend opens an EventSource
/// on `http://127.0.0.1:{port}/sse/chat-stream` to receive raw
/// `chat-stream` events over real HTTP streaming.
#[tauri::command]
pub async fn get_sse_port(state: State<'_, AppState>) -> Result<u16, String> {
    state
        .sse_port
        .lock()
        .await
        .ok_or_else(|| "SSE transport is not ready yet".to_string())
}

/// Get system information.
#[tauri::command]
pub async fn get_system_info(state: State<'_, AppState>) -> Result<SystemInfo, String> {
    let mut sys = System::new_all();
    sys.refresh_all();

    Ok(SystemInfo {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        cpu_count: sys.cpus().len(),
        total_memory_mb: sys.total_memory() / 1024 / 1024,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        app_data_dir: Some(state.app_data_dir.to_string_lossy().to_string()),
    })
}

/// Get the current agent status — snake_case string ("idle" / "thinking" /
/// "tool_running" / "connecting" / "error" / "paused"), the SAME encoding
/// as the agent-status-changed event channel (single contract since #79).
#[tauri::command]
pub async fn get_agent_status(state: State<'_, AppState>) -> Result<String, String> {
    Ok(state.agent_status().as_str().to_string())
}

/// Set the agent status (snake_case string; unknown values fall back to
/// idle — same leniency as the old u8 contract's default).
#[tauri::command]
pub async fn set_agent_status(status: String, state: State<'_, AppState>) -> Result<(), String> {
    state.set_agent_status(AgentStatus::from_str(&status));
    Ok(())
}

/// Cancel the current operation for a session.
#[tauri::command]
pub async fn cancel_operation(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    Ok(state.cancel_session(&session_id).await)
}

/// List main-agent turns still running in the background (persistence
/// view) — sessions the user switched away from but that keep executing.
#[tauri::command]
pub async fn list_running_sessions(
    state: State<'_, AppState>,
) -> Result<Vec<crate::agent::running::RunningTurnInfo>, String> {
    Ok(state.running_turns.list().await)
}

/// Pause the current operation for a session. The agent loop suspends at its
/// next checkpoint (between tool rounds) — the turn is NOT lost and can be
/// resumed with `resume_operation`. Returns false if no operation is running.
#[tauri::command]
pub async fn pause_operation(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    Ok(state.pause_session(&session_id).await)
}

/// Resume a paused operation for a session. Returns false if the session is
/// not paused.
#[tauri::command]
pub async fn resume_operation(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    Ok(state.resume_session(&session_id).await)
}

/// Enable or disable debug tracing.
#[tauri::command]
pub async fn set_debug_mode(enabled: bool, state: State<'_, AppState>) -> Result<(), String> {
    state.set_debug_mode(enabled);
    Ok(())
}

/// Check if debug tracing is enabled.
#[tauri::command]
pub async fn get_debug_mode(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.debug_mode())
}

/// Reload skills from all sources (own + ecosystem). Called after a
/// `[skills]` compat toggle so the change takes effect without a restart.
#[tauri::command]
pub async fn refresh_skills(state: State<'_, AppState>) -> Result<(), String> {
    state.reload_skills().await;
    Ok(())
}

/// Set the workspace path at runtime.
///
/// Called by the frontend when the user selects a project folder.
/// Updates `state.workspace` so the agent loop operates in the correct directory,
/// and grants the frontend fs plugin read access to that directory (the dialog
/// picker does NOT auto-authorize fs access — without this, `readDir` on a
/// dialog-selected path is rejected by the fs scope).
#[tauri::command]
pub async fn set_workspace(
    path: Option<String>,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let old_workspace = {
        let mut workspace = state.workspace.write().map_err(|e| e.to_string())?;
        let old = workspace.clone();
        *workspace = path.clone().map(std::path::PathBuf::from);
        old
    };

    // Recycle LSP clients of the workspace we just LEFT — their server
    // child processes are killed on drop (kill_on_drop), so switching
    // projects stops stale rust-analyzer / tsserver instances instead of
    // letting them accumulate.
    let actually_changed = old_workspace
        .as_deref()
        .map(std::path::Path::to_path_buf)
        .as_deref()
        != path.as_deref().map(std::path::Path::new);
    let scope = app.fs_scope();
    if actually_changed {
        if let Some(old) = old_workspace {
            if old.is_dir() {
                state.lsp_manager.drop_workspace(&old).await;
            }
            // Revoke the fs scope grant of the workspace we just LEFT — a
            // stale grant would keep giving the frontend read access to a
            // project the user switched away from. Skipped when the new
            // workspace lives INSIDE the old one: forbidding the old tree
            // recursively would cut off the new one (forbidden beats
            // allowed in the fs scope).
            let new_inside_old = path
                .as_deref()
                .is_some_and(|p| std::path::Path::new(p).starts_with(&old));
            if !new_inside_old {
                let _ = scope.forbid_directory(&old, true);
                let _ = scope.forbid_file(&old);
            }
        }
    }

    // Load `.claude/settings.json` permission rules for the new project
    // (project settings override the global permission config).
    if let Some(p) = &path {
        state
            .permissions
            .load_project_settings(std::path::Path::new(p));
    } else {
        state
            .permissions
            .load_project_settings(std::path::Path::new(""));
    }

    // Refresh skills so project-level ecosystem skills (.claude/skills,
    // .deepdepcat/skills) take effect for the new workspace.
    state.reload_skills().await;

    // Grant the fs scope read access to the new workspace (recursively) so the
    // frontend can list/read files there. The previous workspace's grant is
    // revoked above on switch.
    if let Some(p) = &path {
        let dir = std::path::PathBuf::from(p);
        scope
            .allow_directory(&dir, true)
            .map_err(|e| format!("failed to grant fs access to workspace: {e}"))?;
        // Allow reading the exact path too (allow_directory covers the tree,
        // but a bare file path passed as the workspace root needs allow_file).
        scope.allow_file(&dir).map_err(|e| e.to_string())?;
    }

    // Persist the last workspace so a restart re-opens the same project
    // (restored in `AppState::initialize` when no explicit workspace is
    // given). Clearing the workspace writes an empty value (never restored).
    let _ = state
        .db
        .set_setting("last_workspace", path.as_deref().unwrap_or(""));
    Ok(())
}

/// Resolve a file path against the current workspace: canonicalize both
/// sides and require strict containment, so the frontend can only open
/// files inside the workspace (mirrors the fs-scope grant). Symlink
/// escapes and `..` traversal are rejected by the canonicalization.
fn resolve_workspace_file(
    workspace: &std::path::Path,
    path: &str,
) -> Result<std::path::PathBuf, String> {
    let ws = workspace
        .canonicalize()
        .map_err(|e| format!("workspace unavailable: {e}"))?;
    let file = std::path::Path::new(path);
    let canon = file
        .canonicalize()
        .map_err(|e| format!("file unavailable: {e}"))?;
    if !canon.starts_with(&ws) {
        return Err(format!("path is outside the workspace: {path}"));
    }
    Ok(canon)
}

/// Reveal a file in the system file manager (Explorer on Windows).
#[cfg(windows)]
fn reveal_in_folder(path: &std::path::Path) -> std::io::Result<()> {
    // `explorer /select,<path>` opens the parent folder with the file
    // pre-selected. The whole argument (including the comma) must stay in
    // ONE argv element — Command quotes it when the path contains spaces.
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", path.display()))
        .spawn()
        .map(|_| ())
}

/// Non-Windows fallback: open the containing folder in the file manager.
#[cfg(not(windows))]
fn reveal_in_folder(path: &std::path::Path) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or(path);
    open::that_detached(parent)
}

/// Open a workspace file with the system default app, or reveal it in the
/// file manager (`reveal=true`). Paths are validated against the current
/// workspace before any external process is launched.
#[tauri::command]
pub async fn open_workspace_file(
    path: String,
    reveal: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let target = {
        let workspace = state.workspace.read().map_err(|e| e.to_string())?;
        let Some(ws) = workspace.as_deref() else {
            return Err("no workspace is open".to_string());
        };
        // Scope the guard: the read lock must be released before the await
        // below (a `RwLockReadGuard` is not Send, and holding it across
        // `spawn_blocking(...).await` makes the command future non-Send).
        resolve_workspace_file(ws, &path)?
    };
    let target_display = target.display().to_string();
    tokio::task::spawn_blocking(move || {
        if reveal {
            reveal_in_folder(&target)
        } else {
            open::that_detached(&target)
        }
    })
    .await
    .map_err(|e| format!("open task panicked: {e}"))?
    .map_err(|e| format!("Failed to open {target_display}: {e}"))?;
    Ok(())
}

/// Circuit breaker state for a single provider.
#[derive(Serialize)]
pub struct ProviderCircuitState {
    pub provider: String,
    pub state: String,
    pub consecutive_failures: u32,
}

/// Get the circuit breaker state for all providers.
#[tauri::command]
pub async fn get_circuit_breaker_states(
    state: State<'_, AppState>,
) -> Result<Vec<ProviderCircuitState>, String> {
    let config = state.config().map_err(|e| e.to_string())?;
    let cb = &state.circuit_breaker;

    let mut result: Vec<ProviderCircuitState> = config
        .llm
        .providers
        .iter()
        .filter(|p| p.enabled)
        .map(|p| {
            let circuit_state = cb.state(&p.name);
            ProviderCircuitState {
                provider: p.name.clone(),
                state: circuit_state.as_str().to_string(),
                consecutive_failures: 0,
            }
        })
        .collect();

    // Fill in the live state + consecutive failure count from the breaker.
    let all = cb.all_states();
    for entry in &mut result {
        if let Some((_, circuit_state, failures)) =
            all.iter().find(|(name, _, _)| name == &entry.provider)
        {
            entry.state = circuit_state.as_str().to_string();
            entry.consecutive_failures = *failures;
        }
    }

    Ok(result)
}

/// List all crash reports.
#[tauri::command]
pub async fn list_crash_reports() -> Result<Vec<crate::core::crash::CrashReportInfo>, String> {
    Ok(crate::core::crash::list_crash_reports())
}

/// Read a specific crash report by filename.
#[tauri::command]
pub async fn read_crash_report(filename: String) -> Result<Option<String>, String> {
    Ok(crate::core::crash::read_crash_report(&filename))
}

/// Delete a crash report by filename.
#[tauri::command]
pub async fn delete_crash_report(filename: String) -> Result<bool, String> {
    Ok(crate::core::crash::delete_crash_report(&filename))
}

/// Get all feature flags.
#[tauri::command]
pub async fn get_feature_flags(
    state: State<'_, AppState>,
) -> Result<Vec<crate::core::feature_flag::FeatureFlag>, String> {
    Ok(state.feature_flags.list_flags())
}

/// Override a feature flag locally.
#[tauri::command]
pub async fn set_feature_flag(
    key: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.feature_flags.set_flag(&key, enabled);
    Ok(())
}

/// Manually reset a provider's circuit breaker.
#[tauri::command]
pub async fn reset_circuit_breaker(
    provider: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.circuit_breaker.reset(&provider);
    Ok(())
}

/// Respond to a pending MCP elicitation request.
///
/// Called by the frontend after the user provides or declines input
/// in response to an MCP server's elicitation request.
#[tauri::command]
pub async fn respond_elicitation(
    elicitation_id: String,
    action: String,
    content: Option<serde_json::Value>,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let result = crate::mcp::types::ElicitationResult { action, content };
    let mut elicitations = state.pending_elicitations.lock().await;
    if let Some(sender) = elicitations.remove(&elicitation_id) {
        let _ = sender.send(result);
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_accepts_file_inside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let file = ws.join("报告.csv");
        std::fs::write(&file, "a,b\n1,2\n").unwrap();

        let resolved = resolve_workspace_file(ws, file.to_str().unwrap()).unwrap();
        assert_eq!(resolved, file.canonicalize().unwrap());
    }

    #[test]
    fn resolve_rejects_file_outside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let file = outside.path().join("secret.csv");
        std::fs::write(&file, "s\n").unwrap();

        let err = resolve_workspace_file(dir.path(), file.to_str().unwrap()).unwrap_err();
        assert!(err.contains("outside the workspace"), "{err}");
    }

    #[test]
    fn resolve_rejects_dotdot_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.csv"), "s\n").unwrap();

        // `ws/../<outside>/secret.csv` canonicalizes to the outside file.
        let parent = dir.path().parent().unwrap();
        let escape = parent
            .join(outside.path().file_name().unwrap())
            .join("secret.csv");
        let err = resolve_workspace_file(dir.path(), escape.to_str().unwrap()).unwrap_err();
        assert!(err.contains("outside the workspace"), "{err}");
    }

    #[test]
    fn resolve_rejects_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("ghost.csv");
        let err = resolve_workspace_file(dir.path(), missing.to_str().unwrap()).unwrap_err();
        assert!(err.contains("file unavailable"), "{err}");
    }
}
