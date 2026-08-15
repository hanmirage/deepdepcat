//! Sync commands — push/pull sessions and settings to the cloud (P0-1).
//!
//! Uses the OAuth Device Flow access token for auth. Push uploads local
//! session metadata (id/title/model/updated_at) and a redacted settings
//! blob; pull downloads remote sessions and restores any that are missing
//! locally (idempotent upsert). Settings are synced BOTH ways: pushed with
//! secrets stripped, and pulled back with the local API keys re-injected
//! (the cloud blob never contains them).

use crate::bootstrap::AppState;
use serde::Serialize;
use tauri::State;
use tracing::{info, warn};

/// Summary of one sync run.
#[derive(Debug, Clone, Serialize)]
pub struct SyncSummary {
    /// Sessions uploaded to the server.
    pub pushed: usize,
    /// Sessions restored from the server.
    pub pulled: usize,
    /// Whether settings were uploaded.
    pub settings_pushed: bool,
    /// Whether remote settings were applied back to the local config.
    pub settings_applied: bool,
}

/// Push local sessions + settings, then pull and restore remote sessions
/// and apply remote settings.
#[tauri::command]
pub async fn sync_now(
    server_url: String,
    token: String,
    state: State<'_, AppState>,
) -> Result<SyncSummary, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let base = server_url.trim_end_matches('/').to_string();

    // ── 1. Collect local sessions ──
    let sessions = {
        let db = state.db.clone();
        // Offloaded: a 1000-row history scan must not block the worker.
        db.list_sessions_async(1000)
            .await
            .map_err(|e| e.to_string())?
    };
    let session_payload: Vec<serde_json::Value> = sessions
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "title": s.title,
                "model": s.model,
                "updated_at": s.updated_at.timestamp() as f64,
                "deleted": false,
            })
        })
        .collect();

    // ── 2. Upload sessions ──
    let push_resp = client
        .post(format!("{base}/api/v1/sync/sessions"))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "sessions": session_payload }))
        .send()
        .await
        .map_err(|e| format!("Push failed: {e}"))?;
    if !push_resp.status().is_success() {
        return Err(format!("Push rejected: HTTP {}", push_resp.status()));
    }
    let pushed = sessions.len();
    info!(pushed, "Cloud sync: sessions pushed");

    // ── 3. Upload redacted settings ──
    let config_value = {
        let guard = state.config().map_err(|e| e.to_string())?;
        serde_json::to_value(&*guard).map_err(|e| e.to_string())?
    };
    let settings_pushed = upload_settings(&client, &base, &token, config_value).await?;

    // ── 4. Pull remote sessions and restore missing ones ──
    let pull_resp = client
        .get(format!("{base}/api/v1/sync/sessions"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("Pull failed: {e}"))?;
    if !pull_resp.status().is_success() {
        return Err(format!("Pull rejected: HTTP {}", pull_resp.status()));
    }
    let pulled = restore_sessions(&state, pull_resp.json().await.map_err(|e| e.to_string())?)
        .await
        .map_err(|e| e.to_string())?;
    info!(pulled, "Cloud sync: sessions pulled");

    // ── 5. Pull remote settings and apply them back (secrets preserved) ──
    let settings_applied = pull_settings(&state, &client, &base, &token).await?;

    Ok(SyncSummary {
        pushed,
        pulled,
        settings_pushed,
        settings_applied,
    })
}

/// Upload the app config as a settings blob with secrets stripped.
async fn upload_settings(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    mut value: serde_json::Value,
) -> Result<bool, String> {
    // Strip API keys before anything leaves the machine.
    if let Some(providers) = value
        .get_mut("llm")
        .and_then(|l| l.get_mut("providers"))
        .and_then(|p| p.as_array_mut())
    {
        for provider in providers {
            if let Some(obj) = provider.as_object_mut() {
                obj.remove("api_key");
                if let Some(env) = obj.get_mut("api_key_env") {
                    *env = serde_json::Value::String(String::new());
                }
            }
        }
    }

    let resp = client
        .put(format!("{base}/api/v1/sync/settings"))
        .bearer_auth(token)
        .json(&serde_json::json!({ "settings": value }))
        .send()
        .await
        .map_err(|e| format!("Settings push failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Settings push rejected: HTTP {}", resp.status()));
    }
    Ok(true)
}

/// Restore remote sessions that do not exist locally.
async fn restore_sessions(state: &AppState, payload: serde_json::Value) -> Result<usize, String> {
    let remote = payload
        .get("sessions")
        .and_then(|s| s.as_array())
        .cloned()
        .unwrap_or_default();
    let db = state.db.clone();
    let local: std::collections::HashSet<String> = db
        .list_sessions_async(1000)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|s| s.id)
        .collect();

    let mut restored = 0;
    for item in remote {
        let id = match item.get("id").and_then(|v| v.as_str()) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => continue,
        };
        if item
            .get("deleted")
            .and_then(|d| d.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        if local.contains(&id) {
            continue;
        }
        let title = item
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("Synced Session")
            .to_string();
        let model = item
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // Remote sessions may carry a work mode — restore into the correct
        // product surface instead of always falling back to code.
        let work_mode = item
            .get("work_mode")
            .and_then(|v| v.as_str())
            .map(|m| m.to_ascii_lowercase())
            .filter(|m| m == "depwork")
            .unwrap_or_else(|| "code".to_string());

        let session = crate::core::types::Session {
            id: id.clone(),
            title,
            model,
            provider: String::new(),
            context_window: 0,
            status: crate::core::types::SessionStatus::Archived,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            workspace_path: None,
            total_usage: crate::core::types::TokenUsage::default(),
            turn_count: 0,
            system_prompt: String::new(),
            work_mode,
            permission_mode: String::new(),
            pinned: false,
            last_message: String::new(),
            is_streaming: false,
        };
        db.upsert_session(&session).map_err(|e| e.to_string())?;
        restored += 1;
        warn!(session_id = %id, "Cloud sync restored remote session");
    }
    Ok(restored)
}

/// Pull the remote settings blob and apply it back into the local config.
///
/// The cloud blob never contains API keys (they are stripped on upload), so
/// local keys are re-injected after the merge — pulling settings must never
/// wipe the user's credentials. Returns whether a blob existed and was
/// applied.
async fn pull_settings(
    state: &AppState,
    client: &reqwest::Client,
    base: &str,
    token: &str,
) -> Result<bool, String> {
    let resp = client
        .get(format!("{base}/api/v1/sync/settings"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("Settings pull failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Settings pull rejected: HTTP {}", resp.status()));
    }
    let payload: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let Some(remote) = payload.get("settings") else {
        return Ok(false);
    };
    if remote.is_null() || remote.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return Ok(false);
    }

    // Snapshot local API keys (name → (api_key, api_key_env)) before the
    // merge so they can be re-injected afterwards.
    let local_keys: Vec<(String, Option<String>, String)> = {
        let guard = state.config().map_err(|e| e.to_string())?;
        guard
            .llm
            .providers
            .iter()
            .map(|p| (p.name.clone(), p.api_key.clone(), p.api_key_env.clone()))
            .collect()
    };

    let mut merged = {
        let guard = state.config().map_err(|e| e.to_string())?;
        serde_json::to_value(&*guard).map_err(|e| e.to_string())?
    };
    crate::core::config::deep_merge_json(&mut merged, remote);

    // Re-inject local keys into the merged config (the cloud blob has them
    // stripped — remote `api_key` values must never win over local ones).
    if let Some(providers) = merged
        .get_mut("llm")
        .and_then(|l| l.get_mut("providers"))
        .and_then(|p| p.as_array_mut())
    {
        for provider in providers {
            let Some(obj) = provider.as_object_mut() else {
                continue;
            };
            let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if let Some((_, key, key_env)) = local_keys.iter().find(|(n, _, _)| n == name) {
                match key {
                    Some(k) => {
                        obj.insert("api_key".to_string(), serde_json::Value::String(k.clone()));
                    }
                    None => {
                        obj.remove("api_key");
                    }
                }
                obj.insert(
                    "api_key_env".to_string(),
                    serde_json::Value::String(key_env.clone()),
                );
            }
        }
    }

    let config: crate::core::config::AppConfig = serde_json::from_value(merged)
        .map_err(|e| format!("Failed to parse remote settings: {e}"))?;
    config
        .save(&state.app_data_dir)
        .map_err(|e| e.to_string())?;
    {
        let mut current = state.config_write().map_err(|e| e.to_string())?;
        *current = config;
    }
    // Hot-apply provider changes (API keys are re-injected above, so the
    // shared LLM client must see them without a restart).
    state.refresh_llm_providers();
    info!("Cloud sync: settings applied from remote");
    Ok(true)
}
