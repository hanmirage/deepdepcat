//! Session lifecycle handler — session dormancy notification.
//!
//! Emits `session-lifecycle` events to the frontend so the UI can update
//! its session list and status indicators. The create/restore/archive/
//! delete handlers were removed as unwired dead code — only the dormancy
//! path (idle-reaper → `on_session_dormant`) is live.

use crate::core::error::AppResult;
use crate::core::types::Session;
use serde::{Deserialize, Serialize};
use tauri::Emitter;

/// The type of session lifecycle event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycle {
    /// Idle-reaper evicted the session's in-memory state (Cat's Dormant).
    Dormant,
}

/// Event emitted when a session lifecycle event occurs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub struct SessionLifecycleEvent {
    pub session_id: String,
    pub lifecycle: SessionLifecycle,
    pub title: String,
    pub model: String,
}

/// Handle session dormancy — the idle-reaper evicted the in-memory state.
pub fn on_session_dormant(app: &tauri::AppHandle, session: &Session) -> AppResult<()> {
    tracing::info!(session_id = %session.id, "Session idled out — memory evicted");
    let _ = app.emit(
        "session-lifecycle",
        SessionLifecycleEvent {
            session_id: session.id.clone(),
            lifecycle: SessionLifecycle::Dormant,
            title: session.title.clone(),
            model: session.model.clone(),
        },
    );
    Ok(())
}
