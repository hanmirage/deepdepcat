//! Permission commands — respond to async permission requests from the frontend.

use crate::bootstrap::AppState;
use tauri::{AppHandle, State};

/// Record the permission decision in the replay-exact event log. Decision
/// is normalized to `allowed` / `granted` / `denied`; granted carries the
/// scope (durable = always allow, session = session-scoped).
fn record_approval(
    app: &AppHandle,
    decision: &str,
    meta: &Option<(String, serde_json::Value, String)>,
    reason: Option<&str>,
) {
    let Some((tool_name, _args, session_id)) = meta else {
        return;
    };
    let (normalized, scope) = match decision {
        "always_allow" => ("granted", "durable"),
        "session_allow" => ("granted", "session"),
        "allow" => ("allowed", "none"),
        "deny" => ("denied", "none"),
        _ => (decision, "none"),
    };
    let mut payload = serde_json::json!({
        "tool": tool_name,
        "decision": normalized,
        "scope": scope,
    });
    if let Some(reason) = reason.filter(|r| !r.trim().is_empty()) {
        payload["reason"] = serde_json::Value::String(reason.to_string());
    }
    crate::observability::event_log::record(app, session_id, None, "approval", payload);
}

/// Respond to a permission request emitted by the backend.
///
/// The `request_id` matches the `PermissionRequest::request_id` sent via
/// the `permission-request` event. `decision` is "allow", "always_allow",
/// "session_allow", or "deny". Both allow variants grant this one request;
/// `always_allow` additionally records a durable grant (per tool +
/// argument pattern) and `session_allow` records a session-scoped grant so
/// future matching calls skip the prompt for the rest of this session.
#[tauri::command]
pub async fn respond_permission(
    request_id: String,
    decision: String,
    app: AppHandle,
    state: State<'_, AppState>,
    scope: Option<String>,
    reason: Option<String>,
) -> Result<(), String> {
    let allow = matches!(
        decision.as_str(),
        "allow" | "always_allow" | "session_allow"
    );
    let metadata = state
        .respond_permission(&request_id, allow, reason.clone())
        .await;
    record_approval(&app, decision.as_str(), &metadata, reason.as_deref());
    match decision.as_str() {
        "always_allow" => {
            if let Some((tool_name, args, session_id)) = metadata {
                // Whole-tool `*` is never offered for bash (dangerous
                // commands must stay un-grantable), so it cannot leak in.
                if scope.as_deref() == Some("tool") && tool_name != "bash" {
                    state.grant_store.record_whole_tool(&tool_name);
                } else {
                    state.grant_store.record(&tool_name, &args);
                }
                state.auto_resolve_pending_permissions(&session_id).await;
                crate::permissions::plan::broadcast_pending_interactions(&app, &session_id).await;
            }
        }
        "session_allow" => {
            if let Some((tool_name, args, session_id)) = metadata {
                if scope.as_deref() == Some("tool") && tool_name != "bash" {
                    state
                        .record_session_grant_pattern(&session_id, &tool_name, "*")
                        .await;
                } else {
                    state
                        .record_session_grant(&session_id, &tool_name, &args)
                        .await;
                }
                state.auto_resolve_pending_permissions(&session_id).await;
                crate::permissions::plan::broadcast_pending_interactions(&app, &session_id).await;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Clear all session-scoped grants for a session (used on session switch).
#[tauri::command]
pub async fn clear_session_grants(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.clear_session_grants(&session_id).await;
    Ok(())
}

/// Clear all remembered "always allow" permission grants.
#[tauri::command]
pub async fn clear_permission_grants(state: State<'_, AppState>) -> Result<(), String> {
    state.grant_store.clear();
    Ok(())
}

/// Auto-Review enable state (Settings → 权限).
#[tauri::command]
pub fn get_auto_review_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state
        .config()
        .map_err(|e| e.to_string())?
        .permissions
        .auto_review)
}

/// Toggle Auto-Review and persist to config.toml (no restart needed).
#[tauri::command]
pub async fn set_auto_review_enabled(
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let app_data_dir = state.app_data_dir.clone();
    {
        let mut config = state.config_write().map_err(|e| e.to_string())?;
        config.permissions.auto_review = enabled;
        config.save(&app_data_dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// User override for an Auto-Review denial: records a session-scoped grant
/// for the exact tool+args so the agent's retry of the SAME action passes
/// (one-retry semantics; lasts for the session, dangerous classes stay
/// un-grantable).
#[tauri::command]
pub async fn override_auto_review_denial(
    session_id: String,
    tool_name: String,
    args: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .record_session_grant(&session_id, &tool_name, &args)
        .await;
    Ok(())
}
