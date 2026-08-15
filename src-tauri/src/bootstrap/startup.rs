//! Startup wiring — the app's composition root.
//!
//! Assembles every background subsystem once at boot so `lib.rs` setup stays
//! thin and the startup sequence is readable top to bottom. Each subsystem is
//! one small function; subsystems that do real network I/O run on spawned
//! tasks so webview load is never blocked.

use crate::bootstrap::AppState;
use tauri::{App, Emitter, Listener, Manager};
use tauri_plugin_updater::UpdaterExt;

/// Start every background subsystem. Called once from setup, after AppState
/// is managed. Errors bubble to setup (which aborts the app build).
pub fn start_subsystems(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    sync_mcp(app)?;
    poll_updates(app);
    idle_reaper(app);
    serve_sse(app);
    serve_acp(app)?;
    serve_a2a(app)?;
    spawn_automation(app);
    Ok(())
}

/// Sync MCP servers from config at startup — connects configured servers and
/// disconnects removed ones (diff-based). Runs in the background: `connect()`
/// performs real network I/O and an unreachable server would otherwise block
/// webview load (white screen on first launch). The frontend loads
/// immediately; MCP tools appear once connections complete.
fn sync_mcp(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    let state = app.state::<AppState>();
    let servers = {
        let config = state.config().map_err(|e| e.to_string())?;
        config.mcp.servers.clone()
    };
    // `state` is a `State<'_, AppState>` (a reference tied to `app`'s
    // lifetime). Deref + clone to own the `AppState` itself so the spawned
    // task is `'static` (does not borrow `app`, which only lives for the
    // setup closure).
    let state_owned: AppState = (*state).clone();
    let app_handle = app.handle().clone();
    // Install the MCP reconnect handler (pool → manager) before the first
    // sync so dropped connections are re-established with exponential backoff.
    state_owned
        .mcp_manager
        .install_reconnect_handler(app_handle.clone());
    tauri::async_runtime::spawn(async move {
        let _ = state_owned
            .mcp_manager
            .sync_configs(&servers, state_owned.tools.clone(), &app_handle)
            .await;
    });
    Ok(())
}

/// Periodic update checks — poll the update server every hour so a freshly
/// published release surfaces the title-bar download button without requiring
/// an app restart. The check is cheap (one small HTTP request returning 204
/// when up-to-date) and failures are silently ignored.
fn poll_updates(app: &App) {
    let app_handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let updater = match app_handle.updater() {
                Ok(u) => u,
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        "Periodic update check unavailable (ignored)"
                    );
                    continue;
                }
            };
            match updater.check().await {
                Ok(Some(update)) => {
                    tracing::info!(
                        latest = %update.version,
                        "Periodic update check found new version"
                    );
                    // Notify the frontend so it can render the title-bar
                    // update button immediately.
                    let silent = update
                        .raw_json
                        .get("silent")
                        .is_some_and(|v| v.as_bool().unwrap_or(false));
                    let min_version = update
                        .raw_json
                        .get("min_version")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    let force = min_version.as_deref().is_some_and(|mv| {
                        !crate::commands::update::version_ge(&update.current_version, mv)
                    });
                    let _ = app_handle.emit(
                        "update-available",
                        crate::commands::update::UpdateInfo {
                            version: update.version.clone(),
                            current_version: update.current_version.clone(),
                            date: update.date.map(|d| d.to_string()),
                            body: update.body.clone(),
                            silent,
                            min_version,
                            force,
                        },
                    );
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::debug!(
                        error = %e,
                        "Periodic update check failed (ignored)"
                    );
                }
            }
        }
    });
}

/// Idle-reaper — evicts ChatState memory for sessions idle beyond the timeout
/// (Cat's Dormant analog). The database is the source of truth; the next
/// touch transparently reloads. Timeout is env-overridable
/// (`DDC_IDLE_TIMEOUT_SECS`) for testing.
fn idle_reaper(app: &App) {
    let state_owned: AppState = (*app.state::<AppState>()).clone();
    let app_handle = app.handle().clone();
    let idle_timeout = std::time::Duration::from_secs(
        std::env::var("DDC_IDLE_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30 * 60),
    );
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let evicted = {
                let mut sessions = state_owned.sessions.lock().await;
                sessions.evict_idle(idle_timeout)
            };
            for session_id in evicted {
                // Idle-evicted sessions drop their in-memory usage tracker
                // (the durable global aggregate keeps the cumulative totals;
                // the next touch recreates a fresh tracker). Without this the
                // map grows with every session that ever ran.
                state_owned.drop_usage_tracker(&session_id).await;
                let session = {
                    let mut sessions = state_owned.sessions.lock().await;
                    sessions.get_session(&session_id).ok().cloned()
                };
                if let Some(session) = session {
                    let _ = crate::agent::handlers::session::on_session_dormant(
                        &app_handle,
                        &session,
                    );
                }
            }
        }
    });
}

/// Real SSE transport — always-on loopback stream of raw `chat-stream` events
/// (the frontend EventSource endpoint).
fn serve_sse(app: &App) {
    let state_owned: AppState = (*app.state::<AppState>()).clone();
    let hub = state_owned.sse_hub.clone();
    let app_handle = app.handle().clone();
    app_handle.listen("chat-stream", move |event| {
        hub.emit_raw(event.payload());
    });
    tauri::async_runtime::spawn(async move {
        match crate::sse::serve(state_owned.sse_hub.clone(), 0).await {
            Ok(port) => {
                *state_owned.sse_port.lock().await = Some(port);
            }
            Err(e) => {
                tracing::error!(error = %e, "SSE transport failed to start");
            }
        }
    });
}

/// ACP (Agent Client Protocol) server — optional localhost JSON-RPC service
/// exposing the app as a remote agent to external clients. Disabled by
/// default (`app.acp_enabled` in config.toml).
fn serve_acp(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    let state_owned: AppState = (*app.state::<AppState>()).clone();
    let app_handle = app.handle().clone();
    let acp_enabled = {
        let config = state_owned.config().map_err(|e| e.to_string())?;
        config.app.acp_enabled
    };
    if acp_enabled {
        let port = {
            let config = state_owned.config().map_err(|e| e.to_string())?;
            config.app.acp_port
        };
        tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::acp::serve(app_handle, state_owned, port).await {
                tracing::error!(error = %e, "ACP server failed to start");
            }
        });
    }
    Ok(())
}

/// A2A (Agent2Agent) inbound server — optional localhost JSON-RPC exposing
/// DeepDepCat as an agent other agents can orchestrate (AgentCard +
/// tasks/send/get/cancel). Disabled by default (`app.a2a_enabled`).
fn serve_a2a(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    let state_owned: AppState = (*app.state::<AppState>()).clone();
    let app_handle = app.handle().clone();
    let (a2a_enabled, a2a_port) = {
        let config = state_owned.config().map_err(|e| e.to_string())?;
        (config.app.a2a_enabled, config.app.a2a_port)
    };
    if a2a_enabled {
        tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::a2a::serve(app_handle, state_owned, a2a_port).await {
                tracing::error!(error = %e, "A2A server failed to start");
            }
        });
    }
    Ok(())
}

/// Scheduled agent tasks (定时任务) — persistent polling runner. Fires due
/// tasks as background agent sessions with unattended permission posture;
/// runs survive app restarts through the DB.
fn spawn_automation(app: &App) {
    let state_owned: AppState = (*app.state::<AppState>()).clone();
    let app_handle = app.handle().clone();
    crate::automation::AutomationRunner::new(
        state_owned.automation_store.clone(),
        state_owned.clone(),
        state_owned.automation_running.clone(),
    )
    .spawn(app_handle);
}
