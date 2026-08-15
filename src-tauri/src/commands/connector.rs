//! Connector commands — manage external service connections and plugins.
//!
//! ⚠️  These are UI placeholder stubs only.
//! The Settings view's ConnectorsCard and PluginsCard call these commands.
//! They return empty data until a real connector subsystem is implemented.

use crate::bootstrap::AppState;
use crate::core::types::{Connector, Plugin};
use tauri::State;

/// List available connectors.
#[tauri::command]
pub async fn list_connectors(_state: State<'_, AppState>) -> Result<Vec<Connector>, String> {
    Ok(vec![])
}

/// Connect to an external service by connector ID.
#[tauri::command]
pub async fn connect_connector(
    connector_id: String,
    _state: State<'_, AppState>,
) -> Result<bool, String> {
    Err(format!(
        "Connector '{}' not available — connector subsystem is not yet implemented",
        connector_id
    ))
}

/// Disconnect from a service.
#[tauri::command]
pub async fn disconnect_connector(
    connector_id: String,
    _state: State<'_, AppState>,
) -> Result<(), String> {
    Err(format!(
        "Connector '{}' not available — connector subsystem is not yet implemented",
        connector_id
    ))
}

/// List available plugins.
#[tauri::command]
pub async fn list_plugins(_state: State<'_, AppState>) -> Result<Vec<Plugin>, String> {
    Ok(vec![])
}

/// Install a plugin.
#[tauri::command]
pub async fn install_plugin(plugin_id: String, state: State<'_, AppState>) -> Result<(), String> {
    if state.plugin_policy.is_blocked(&plugin_id) {
        return Err(format!(
            "Plugin '{plugin_id}' is BLOCKED by the plugin policy — an admin \
             must set it to 'available' before it can be installed"
        ));
    }
    Err(format!(
        "Plugin '{}' not available — plugin subsystem is not yet implemented",
        plugin_id
    ))
}

/// Uninstall a plugin.
#[tauri::command]
pub async fn uninstall_plugin(
    plugin_id: String,
    _state: State<'_, AppState>,
) -> Result<(), String> {
    Err(format!(
        "Plugin '{}' not available — plugin subsystem is not yet implemented",
        plugin_id
    ))
}

/// Enable/disable a plugin.
#[tauri::command]
pub async fn toggle_plugin(
    plugin_id: String,
    _enabled: bool,
    _state: State<'_, AppState>,
) -> Result<(), String> {
    Err(format!(
        "Plugin '{}' not available — plugin subsystem is not yet implemented",
        plugin_id
    ))
}
