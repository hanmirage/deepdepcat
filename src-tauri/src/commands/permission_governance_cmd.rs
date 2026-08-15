//! Permission governance commands — grant audit/revoke, hot rule updates,
//! and the plugin policy layer.

use crate::bootstrap::AppState;
use crate::permissions::grant_store::PermissionGrant;
use serde::{Deserialize, Serialize};
use tauri::State;

/// Snapshot of the settings rules (allow/deny/ask) for the governance UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRulesView {
    pub mode: String,
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    pub ask: Vec<String>,
}

/// List all durable "always allow" grants (audit view).
#[tauri::command]
pub fn list_permission_grants(state: State<'_, AppState>) -> Result<Vec<PermissionGrant>, String> {
    Ok(state.grant_store.list_grants())
}

/// Revoke ONE grant by tool + pattern — takes effect immediately.
#[tauri::command]
pub fn remove_permission_grant(
    tool_name: String,
    pattern: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    Ok(state.grant_store.remove(&tool_name, &pattern))
}

/// Read the current settings rules.
#[tauri::command]
pub fn get_permission_rules(state: State<'_, AppState>) -> Result<PermissionRulesView, String> {
    let config = state.config().map_err(|e| e.to_string())?;
    Ok(PermissionRulesView {
        mode: config.permissions.mode.clone(),
        allow: config.permissions.allow.clone(),
        deny: config.permissions.deny.clone(),
        ask: config.permissions.ask.clone(),
    })
}

/// Replace the settings rules (allow/deny/ask): persist `config.toml` AND
/// hot-swap the in-memory rule set — running sessions see the new rules
/// immediately, no restart.
#[tauri::command]
pub fn set_permission_rules(
    allow: Vec<String>,
    deny: Vec<String>,
    ask: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    {
        let mut config = state.config_write().map_err(|e| e.to_string())?;
        config.permissions.allow = allow;
        config.permissions.deny = deny;
        config.permissions.ask = ask;
        config
            .save(&state.app_data_dir)
            .map_err(|e| e.to_string())?;
        let section = config.permissions.clone();
        state.permissions.reload_rules(&section);
    }
    Ok(())
}

/// Snapshot of the plugin policy map.
#[tauri::command]
pub fn list_plugin_policy(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let policy = state.plugin_policy.snapshot();
    serde_json::to_value(policy.plugins).map_err(|e| e.to_string())
}

/// Set a plugin's policy (`available` | `blocked`). A blocked plugin cannot
/// be installed until the policy changes.
#[tauri::command]
pub fn set_plugin_policy(
    plugin_id: String,
    action: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !state.plugin_policy.set(&plugin_id, &action) {
        return Err(format!(
            "Invalid policy action '{action}' — use available or blocked"
        ));
    }
    Ok(())
}
