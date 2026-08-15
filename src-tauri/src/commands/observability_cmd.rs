//! Observability commands — session usage tracking and queries.

use crate::bootstrap::AppState;
use crate::observability::usage::SessionUsageSummary;
use crate::storage::database::GlobalUsage;
use tauri::State;

/// Get usage summary for a session (token counts, tool calls, turn count).
#[tauri::command]
pub async fn get_session_usage(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<SessionUsageSummary, String> {
    let tracker = state.usage_tracker(&session_id).await;
    let mut summary = tracker.summary();
    // Attach the live model context window (0 = unknown) so the UI's usage
    // ring can compute a real percentage against the actual budget.
    summary.context_window = state
        .sessions
        .lock()
        .await
        .context_window(&session_id)
        .unwrap_or(0);
    Ok(summary)
}

/// Get the cumulative usage across ALL sessions — durable aggregate from
/// the `usage_aggregate` table (never reset, survives restarts).
#[tauri::command]
pub async fn get_global_usage(state: State<'_, AppState>) -> Result<GlobalUsage, String> {
    // Flush every LIVE session's pending deltas first. A fresh store's
    // `get()` only flushes its own (empty) atomics, so a session's trailing
    // <32 operations would otherwise be invisible to this read — and lost
    // forever on app exit.
    {
        let trackers = state.usage_trackers.lock().await;
        for tracker in trackers.values() {
            tracker.flush_global();
        }
    }
    Ok(crate::storage::database::GlobalUsageStore::new(state.db.clone()).get())
}

/// List the most recent replay-exact agent events of a session (newest
/// first) — the audit view of what actually happened: model calls, tool
/// runs, permission decisions, and file edits.
#[tauri::command]
pub fn get_session_events(
    session_id: String,
    limit: Option<usize>,
    turn_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Vec<crate::storage::database::AgentEvent>, String> {
    match turn_id {
        // Turn-scoped replay: exact execution order (seq ascending).
        Some(tid) => crate::storage::database::list_turn_events(&state.db, &session_id, &tid)
            .map_err(|e| e.to_string()),
        // Session view: newest first.
        None => crate::storage::database::list_events(&state.db, &session_id, limit.unwrap_or(200))
            .map_err(|e| e.to_string()),
    }
}
