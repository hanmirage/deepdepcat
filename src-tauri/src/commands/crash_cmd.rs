//! Crash report commands — crash upload pipeline (anonymous, no login).
//!
//! The server exposes an anonymous pipeline (`/api/v1/crash`):
//!   1. POST /api/v1/crash                — submit the bare crash (no auth)
//!   2. POST /api/v1/crash/conversation   — attach conversation JSON (opt-in)
//!
//! Privacy model: the crash dialog tells the user "DeepDepCat 非常尊重您的
//! 隐私" and offers two opt-in options — (a) send the crash report only,
//! (b) additionally attach a JSON export of the conversation. Nothing is sent
//! unless the user picks an option on that dialog.

use crate::core::crash::PendingCrash;
use crate::bootstrap::AppState;
use serde::Serialize;
use tauri::State;
use tracing::{info, warn};

/// Result of a crash submission.
#[derive(Debug, Clone, Serialize)]
pub struct CrashSubmitResult {
    pub status: String,
    pub crash_id: Option<i64>,
}

/// Read the pending crash payload (if any) for the crash dialog.
#[tauri::command]
pub async fn get_pending_crash() -> Result<Option<PendingCrash>, String> {
    Ok(crate::core::crash::read_pending_crash())
}

/// Dismiss the pending crash (user chose not to send, or it was sent).
#[tauri::command]
pub async fn dismiss_pending_crash() -> Result<(), String> {
    crate::core::crash::clear_pending_crash();
    Ok(())
}

/// Get the current diagnostics (anonymous error telemetry) toggle.
#[tauri::command]
pub async fn get_diagnostics_enabled() -> Result<bool, String> {
    Ok(crate::observability::diagnostics::is_enabled())
}

/// Set the diagnostics (anonymous error telemetry) toggle from Settings.
///
/// Persisted to the settings KV table so the choice survives restarts
/// (restored in `AppState::initialize`).
#[tauri::command]
pub async fn set_diagnostics_enabled(
    enabled: bool,
    state: tauri::State<'_, crate::bootstrap::AppState>,
) -> Result<(), String> {
    crate::observability::diagnostics::set_enabled(enabled);
    let _ = state
        .db
        .set_setting("diagnostics_enabled", &enabled.to_string());
    Ok(())
}

/// Submit a client-side error to the anonymous telemetry endpoint.
///
/// The frontend error reporter (`reportClientError`) used to POST via browser
/// `fetch`, which the Tauri webview's CORS blocks (the telemetry server sends
/// no `Access-Control-Allow-Origin`). Routing through a Rust command sends the
/// payload natively (reqwest — no CORS), so client errors actually reach the
/// server in the packaged app. Best-effort: a failure is surfaced as an error
/// and the reporter swallows it.
#[tauri::command]
pub async fn submit_client_error(
    server_url: String,
    payload: serde_json::Value,
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;
    let base = server_url.trim_end_matches('/').to_string();
    let resp = client
        .post(format!("{base}/api/v1/telemetry/collect"))
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Telemetry upload failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("Telemetry upload rejected: HTTP {}", resp.status()));
    }
    Ok(())
}

/// Export the current session's conversation as a JSON string.
///
/// Called by the crash dialog ONLY after the user opts in to sharing the
/// conversation. Returns the raw conversation JSON for two-phase upload.
#[tauri::command]
pub async fn export_session_conversation(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let messages = state
        .db
        .load_messages(&session_id)
        .map_err(|e| format!("Failed to load conversation: {e}"))?;
    serde_json::to_string(&messages).map_err(|e| format!("Failed to serialize: {e}"))
}

/// Submit the pending crash report to the server (anonymous, no auth).
///
/// Phase 1: POST /api/v1/crash with the bare crash payload. If the user also
/// opted in to sharing the conversation, `conversation_json` is uploaded in
/// phase 2 as a separate request.
#[tauri::command]
pub async fn submit_crash_report(
    server_url: String,
    include_conversation: bool,
    conversation_json: Option<String>,
) -> Result<CrashSubmitResult, String> {
    let pending = crate::core::crash::read_pending_crash()
        .ok_or_else(|| "No pending crash report to submit".to_string())?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let base = server_url.trim_end_matches('/').to_string();

    // ── Phase 1: bare crash ──
    let body = serde_json::json!({
        "app_version": pending.app_version,
        "os": pending.os,
        "arch": pending.arch,
        "client_id": pending.client_id,
        "pid": pending.pid,
        "panic_message": pending.panic_message,
        "backtrace": pending.backtrace,
        "include_conversation": include_conversation,
        "client_timestamp": chrono::Utc::now().timestamp() as f64,
    });
    let resp = client
        .post(format!("{base}/api/v1/crash"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Crash upload failed: {e}"))?;

    if !resp.status().is_success() {
        // Keep the payload pending so the user can retry.
        return Err(format!("Crash upload rejected: HTTP {}", resp.status()));
    }
    let parsed: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let crash_id = parsed.get("crash_id").and_then(|v| v.as_i64());

    // ── Phase 2: conversation (only if the user opted in AND we have one) ──
    if include_conversation {
        if let Some(conv) = conversation_json {
            let conv_resp = client
                .post(format!("{base}/api/v1/crash/conversation"))
                .json(&serde_json::json!({
                    "crash_id": crash_id,
                    "conversation_json": conv,
                }))
                .send()
                .await;
            match conv_resp {
                Ok(r) if r.status().is_success() => {
                    info!("Crash conversation attached to report {crash_id:?}");
                }
                Ok(r) => warn!(
                    "Crash conversation attach rejected: HTTP {} (report still saved)",
                    r.status()
                ),
                Err(e) => warn!("Crash conversation attach failed: {e} (report still saved)"),
            }
        } else {
            warn!("User opted into conversation sharing but none was provided");
        }
    }

    // Payload has been sent — clear it.
    crate::core::crash::clear_pending_crash();
    info!(?crash_id, "Crash report submitted");

    Ok(CrashSubmitResult {
        status: "accepted".to_string(),
        crash_id,
    })
}
