//! Replay-exact event log recording — best-effort, never breaks the loop.
//!
//! `record` is the single entry point used by the agent loop, tool batch,
//! and permission commands. A database failure (or missing state in
//! headless/test contexts) is logged at debug level and swallowed: the
//! audit trail is a byproduct of the run, not a dependency of it.

use serde_json::Value;
use tauri::{AppHandle, Manager};

/// Append one agent event. Payloads must be SUMMARY-SHAPED — lengths and
/// statuses, never full command output, file contents, or secrets.
pub fn record(
    app: &AppHandle,
    session_id: &str,
    turn_id: Option<&str>,
    kind: &str,
    payload: Value,
) {
    let Some(state) = app.try_state::<crate::bootstrap::AppState>() else {
        return;
    };
    if let Err(e) =
        crate::storage::database::append_event(&state.db, session_id, turn_id, kind, payload)
    {
        tracing::debug!(
            session_id,
            kind,
            error = %e,
            "Failed to append agent event (best-effort)"
        );
    }
}
