//! Hook commands — list, save, and delete hook definitions.
//!
//! Hooks are persisted as TOML in the user-level `hooks.toml` file.
//! Saving a hook replaces an existing one with the same
//! (event, type, content) tuple; deleting removes it.

use crate::bootstrap::AppState;
use crate::hooks::trust::fingerprint;
use crate::hooks::types::{HookDefinition, HookEvent};
use serde::Serialize;
use tauri::State;

/// A hook definition plus its trust status — the settings page needs both:
/// the definition for editing and the CURRENT content fingerprint so a
/// changed hook is visibly "needs review" until trusted again.
#[derive(Debug, Clone, Serialize)]
pub struct HookView {
    #[serde(flatten)]
    pub definition: HookDefinition,
    pub trusted: bool,
    pub fingerprint: String,
}

/// List all user-level hooks with trust status.
#[tauri::command]
pub async fn list_hooks(state: State<'_, AppState>) -> Result<Vec<HookView>, String> {
    let defs = crate::hooks::discovery::list_hooks(&state.app_data_dir).map_err(|e| e.to_string())?;
    Ok(defs
        .into_iter()
        .map(|definition| {
            let fp = fingerprint(&definition);
            HookView {
                trusted: state.hook_trust.is_trusted(&definition),
                fingerprint: fp,
                definition,
            }
        })
        .collect())
}

/// Expanded preview of a hook's executable fields (UI display only).
///
/// Env variables are expanded so users can verify `$VAR` references, but
/// values of sensitive env vars and sensitive URL query parameters are
/// masked — previews must never surface secrets verbatim.
#[derive(Debug, Clone, Serialize)]
pub struct HookPreview {
    /// Expanded + redacted command (Command hooks).
    pub command: Option<String>,
    /// Expanded + redacted prompt (Prompt hooks).
    pub prompt: Option<String>,
    /// Expanded + redacted URL (Http hooks).
    pub url: Option<String>,
}

/// Compute an expanded, redacted preview of a hook definition.
#[tauri::command]
pub async fn preview_hook(hook: HookDefinition) -> Result<HookPreview, String> {
    let redact = |input: &str| {
        crate::hooks::env_expand::redact_sensitive(&crate::hooks::env_expand::preview_expansion(
            input,
        ))
    };
    Ok(HookPreview {
        command: hook.command.as_deref().map(redact),
        prompt: hook.prompt.as_deref().map(redact),
        url: hook.url.as_deref().map(redact),
    })
}

/// Save a hook definition to the user-level hooks.toml.
#[tauri::command]
pub async fn save_hook(hook: HookDefinition, state: State<'_, AppState>) -> Result<usize, String> {
    let count = crate::hooks::discovery::save_hook(&state.app_data_dir, &hook)
        .map_err(|e| e.to_string())?;
    // Saving IS the review step: the user saw and saved this exact
    // definition, so it is trusted immediately. Editing a hook later
    // changes its fingerprint and requires re-saving (a fresh review).
    state.hook_trust.trust(&fingerprint(&hook));
    reload_runtime_hooks(&state);
    Ok(count)
}

/// Explicitly trust a hook by its current content fingerprint (project or
/// externally edited hooks). Persisted across restarts.
#[tauri::command]
pub async fn trust_hook(fingerprint: String, state: State<'_, AppState>) -> Result<(), String> {
    state.hook_trust.trust(&fingerprint);
    Ok(())
}

/// Revoke trust for a hook fingerprint — it stops running until trusted
/// again.
#[tauri::command]
pub async fn untrust_hook(fingerprint: String, state: State<'_, AppState>) -> Result<(), String> {
    state.hook_trust.untrust(&fingerprint);
    Ok(())
}

/// Delete a hook by (event, type, content) from the user-level hooks.toml.
#[tauri::command]
pub async fn delete_hook(
    event: String,
    hook_type: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let count =
        crate::hooks::discovery::delete_hook(&state.app_data_dir, &event, &hook_type, &content)
            .map_err(|e| e.to_string())?;
    reload_runtime_hooks(&state);
    Ok(count)
}

/// Rebuild the runtime hook registry from disk.
///
/// The executor reads THIS registry — without a reload, saves/deletes via
/// the settings page would never take effect.
fn reload_runtime_hooks(state: &AppState) {
    let workspace = state.workspace.read().map(|w| w.clone()).unwrap_or(None);
    let enable_project = state
        .config()
        .map(|c| c.hooks.enable_project_hooks)
        .unwrap_or(false);
    let mut guard = state.hooks.write().unwrap_or_else(|e| e.into_inner());
    guard.clear_all();
    match crate::hooks::discovery::discover_and_register(
        &mut guard,
        &state.app_data_dir,
        workspace.as_deref(),
        enable_project,
    ) {
        Ok(count) => {
            tracing::info!(count, "Runtime hooks reloaded after config change");
        }
        Err(e) => {
            tracing::warn!(error = %e, "Hook reload failed");
        }
    }
}

/// List project-level hooks from `<workspace>/.deepdepcat/hooks.toml`.
/// Read-only audit view: project hooks are never editable from the UI —
/// they can only be globally disabled via the master switch.
#[tauri::command]
pub async fn list_project_hooks(state: State<'_, AppState>) -> Result<Vec<HookView>, String> {
    let workspace = state.workspace.read().map(|w| w.clone()).unwrap_or(None);
    let Some(ws) = workspace else {
        return Ok(Vec::new());
    };
    let path = ws.join(".deepdepcat").join("hooks.toml");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let defs = crate::hooks::discovery::list_hooks_file(&path).map_err(|e| e.to_string())?;
    Ok(defs
        .into_iter()
        .map(|definition| {
            let fp = fingerprint(&definition);
            HookView {
                trusted: state.hook_trust.is_trusted(&definition),
                fingerprint: fp,
                definition,
            }
        })
        .collect())
}

/// Whether project-level hooks are enabled (default: off — a cloned repo
/// must not execute arbitrary commands without an explicit opt-in).
#[tauri::command]
pub async fn get_project_hooks_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    let config = state.config().map_err(|e| e.to_string())?;
    Ok(config.hooks.enable_project_hooks)
}

/// Enable/disable project-level hooks — persists the config and reloads
/// the runtime registry so the change takes effect immediately.
#[tauri::command]
pub async fn set_project_hooks_enabled(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    {
        let mut config = state.config_write().map_err(|e| e.to_string())?;
        config.hooks.enable_project_hooks = enabled;
        config
            .save(&state.app_data_dir)
            .map_err(|e| e.to_string())?;
    }
    reload_runtime_hooks(&state);
    Ok(())
}

/// List all supported hook events (for the UI dropdown).
#[tauri::command]
pub async fn list_hook_events() -> Result<Vec<String>, String> {
    Ok(ALL_HOOK_EVENTS
        .iter()
        .map(|e| e.as_str().to_string())
        .collect())
}

/// All supported hook events, in a sensible UI order.
const ALL_HOOK_EVENTS: &[HookEvent] = &[
    HookEvent::PreToolUse,
    HookEvent::PostToolUse,
    HookEvent::PostToolUseFailure,
    HookEvent::PostToolBatch,
    HookEvent::ToolError,
    HookEvent::PreLLMCall,
    HookEvent::PostLLMCall,
    HookEvent::Stop,
    HookEvent::StopFailure,
    HookEvent::SubagentStart,
    HookEvent::SubagentStop,
    HookEvent::TaskUpdated,
    HookEvent::TaskCompleted,
    HookEvent::UserMessage,
    HookEvent::AssistantMessage,
    HookEvent::SessionStart,
    HookEvent::SessionEnd,
    HookEvent::AgentLoopStart,
    HookEvent::AgentLoopEnd,
    HookEvent::AgentLoopTurn,
    HookEvent::PreCompaction,
    HookEvent::PostCompaction,
    HookEvent::FileChanged,
    HookEvent::FileCreated,
    HookEvent::FileDeleted,
    HookEvent::PermissionDenied,
    HookEvent::PermissionAsked,
    HookEvent::Notification,
    HookEvent::MemoryStored,
    HookEvent::MemorySearched,
    HookEvent::McpServerConnected,
    HookEvent::McpServerDisconnected,
    HookEvent::Error,
    HookEvent::FatalError,
];
