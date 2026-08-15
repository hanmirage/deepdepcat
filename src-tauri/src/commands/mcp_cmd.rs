//! MCP commands — manage MCP server connections.

use crate::core::config::McpServerConfig;
use crate::bootstrap::AppState;
use crate::mcp::auto_setup;
use crate::mcp::types::McpTool;
use tauri::{Emitter, Manager, State};

/// List configured MCP servers.
#[tauri::command]
pub async fn list_mcp_servers(state: State<'_, AppState>) -> Result<Vec<McpServerConfig>, String> {
    let config = state.config().map_err(|e| e.to_string())?;
    Ok(config.mcp.servers.clone())
}

/// Add (or update) an MCP server in the persisted config.
#[tauri::command]
pub async fn add_mcp_server(
    config: McpServerConfig,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let mut current = state.config_write().map_err(|e| e.to_string())?;
    let servers = &mut current.mcp.servers;
    match servers.iter_mut().find(|s| s.name == config.name) {
        Some(existing) => *existing = config,
        None => servers.push(config),
    }
    current
        .save(&state.app_data_dir)
        .map_err(|e| e.to_string())?;
    state.refresh_llm_providers();
    Ok(())
}

/// Remove an MCP server from the persisted config (and disconnect it).
#[tauri::command]
pub async fn remove_mcp_server(name: String, state: State<'_, AppState>) -> Result<(), String> {
    // Backend-authoritative teardown: disconnect the live client (process
    // + registered tools) and forget its config so a pending reconnect
    // never resurrects the removed server. The frontend also disconnects
    // first, but a stale status must not leave a zombie running.
    let _ = state.mcp_manager.disconnect(&name).await;
    state.mcp_manager.forget_config(&name).await;

    let mut current = state.config_write().map_err(|e| e.to_string())?;
    let before = current.mcp.servers.len();
    current.mcp.servers.retain(|s| s.name != name);
    if current.mcp.servers.len() == before {
        return Err(format!("MCP server '{name}' not found"));
    }
    current
        .save(&state.app_data_dir)
        .map_err(|e| e.to_string())?;
    state.refresh_llm_providers();
    Ok(())
}

/// Connect to an MCP server.
#[tauri::command]
pub async fn connect_mcp_server(
    config: McpServerConfig,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Remember the config for the reconnect handler BEFORE connecting: a
    // server added from settings has no entry in the startup sync snapshot,
    // and without one a dropped connection can never be re-established.
    state.mcp_manager.remember_config(&config).await;
    let result = state
        .mcp_manager
        .connect(&config, &state.tools, &app)
        .await;

    // Auto-setup: the bundled wps-office server failed because its Python
    // package isn't installed. Build the app-managed venv + install it, then
    // reconnect with the venv interpreter. Only for the bundled WPS server —
    // a random third-party MCP missing a module must NOT trigger an install.
    if let Err(e) = &result {
        let err_text = e.to_string();
        if auto_setup::needs_setup(&config, &err_text) {
            emit_status(&app, &config.name, "installing", None);
            match auto_setup::ensure_venv(&state.app_data_dir, &source_dir(&app)).await {
                Ok(venv_python) => {
                    let mut patched = config.clone();
                    patched.command = Some(venv_python.to_string_lossy().into_owned());
                    return state
                        .mcp_manager
                        .connect(&patched, &state.tools, &app)
                        .await
                        .map_err(|e| e.to_string());
                }
                Err(setup_err) => {
                    emit_status(&app, &config.name, "error", Some(&setup_err));
                    return Err(setup_err);
                }
            }
        }
    }
    result.map_err(|e| e.to_string())
}

/// Resolve the bundled depwork-mcp source dir for auto-setup.
fn source_dir(app: &tauri::AppHandle) -> std::path::PathBuf {
    let resource = app
        .path()
        .resource_dir()
        .unwrap_or_else(|_| std::path::PathBuf::new());
    auto_setup::depwork_mcp_source_dir(&resource)
}

/// Emit an MCP status event (mirrors the manager's internal helper, which is
/// private — auto-setup happens above the manager so it emits its own).
fn emit_status(app: &tauri::AppHandle, name: &str, status: &str, error: Option<&str>) {
    let mut payload = serde_json::json!({ "name": name, "status": status });
    if let Some(error) = error {
        payload["error"] = serde_json::json!(error);
    }
    let _ = app.emit("mcp-status-changed", payload);
}

/// Disconnect from an MCP server.
#[tauri::command]
pub async fn disconnect_mcp_server(name: String, state: State<'_, AppState>) -> Result<(), String> {
    state
        .mcp_manager
        .disconnect(&name)
        .await
        .map_err(|e| e.to_string())
}

/// Get tools from a connected MCP server.
#[tauri::command]
pub async fn get_mcp_tools(
    server_name: String,
    state: State<'_, AppState>,
) -> Result<Vec<McpTool>, String> {
    let all_tools = state.mcp_manager.list_all_tools().await;
    Ok(all_tools
        .into_iter()
        .filter(|(name, _)| name == &server_name)
        .map(|(_, tool)| tool)
        .collect())
}

/// List all connected MCP servers.
#[tauri::command]
pub async fn list_connected_mcp_servers(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    Ok(state.mcp_manager.list_servers().await)
}

/// List prompts exposed by a connected MCP server.
#[tauri::command]
pub async fn list_mcp_prompts(
    server_name: String,
    state: State<'_, AppState>,
) -> Result<Vec<crate::mcp::types::McpPrompt>, String> {
    state
        .mcp_manager
        .list_prompts(&server_name)
        .await
        .map_err(|e| e.to_string())
}

/// Get a prompt template from an MCP server with arguments filled in.
#[tauri::command]
pub async fn call_mcp_prompt(
    server_name: String,
    prompt_name: String,
    arguments: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    state
        .mcp_manager
        .get_prompt(&server_name, &prompt_name, arguments)
        .await
        .map_err(|e| e.to_string())
}

/// Save an OAuth credential for an MCP server (persisted to app data dir).
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn save_mcp_credential(
    server_name: String,
    server_url: String,
    access_token: String,
    token_type: String,
    expires_at: Option<String>,
    refresh_token: Option<String>,
    token_endpoint: Option<String>,
    client_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let cred = crate::mcp::credentials::McpCredential {
        access_token,
        token_type,
        expires_at,
        refresh_token,
        server_url: server_url.clone(),
        token_endpoint,
        client_id,
    };
    let mut store = crate::mcp::credentials::McpCredentialStore::load_from(&state.app_data_dir)
        .map_err(|e| e.to_string())?;
    store
        .insert_and_save(&server_name, &server_url, cred, &state.app_data_dir)
        .map_err(|e| e.to_string())
}

/// Remove a stored credential for an MCP server.
#[tauri::command]
pub async fn delete_mcp_credential(
    server_name: String,
    server_url: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let mut store = crate::mcp::credentials::McpCredentialStore::load_from(&state.app_data_dir)
        .map_err(|e| e.to_string())?;
    let removed = store.remove(&server_name, &server_url);
    if removed.is_some() {
        store
            .save_to(&state.app_data_dir)
            .map_err(|e| e.to_string())?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// List stored MCP credential server names (never the tokens themselves).
#[tauri::command]
pub async fn list_mcp_credentials(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let store = crate::mcp::credentials::McpCredentialStore::load_from(&state.app_data_dir)
        .map_err(|e| e.to_string())?;
    Ok(store.server_names())
}

/// Proxy an MCP Apps view request to its server (MCP Apps spec — the view
/// acts as an MCP client over postMessage; the host forwards tools/call and
/// resources/read only).
#[tauri::command]
pub async fn mcp_app_proxy(
    server: String,
    method: String,
    params: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    state
        .mcp_manager
        .proxy_ui_request(&server, &method, params)
        .await
        .map_err(|e| e.to_string())
}

/// Forward an MCP App's log/console message to the backend replay-exact
/// event log (debug aid) — payload is summary-shaped and capped.
#[tauri::command]
pub fn mcp_app_log(
    server: String,
    level: String,
    message: String,
    session_id: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    crate::observability::event_log::record(
        &app,
        session_id.as_deref().unwrap_or(""),
        None,
        "mcp_app_log",
        serde_json::json!({
            "server": server,
            "level": level,
            "message": message.chars().take(500).collect::<String>(),
        }),
    );
    Ok(())
}
