//! Rewind commands — session state restoration.

use crate::bootstrap::AppState;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tracing::{info, warn};

/// The result of a rewind operation, serializable for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewindResult {
    pub success: bool,
    pub restored_files: Vec<String>,
    pub conflicts: Vec<RewindConflictInfo>,
    pub error: Option<String>,
}

/// Information about a conflict detected during rewind.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewindConflictInfo {
    pub path: String,
    pub conflict_type: String,
}

/// Rewind the workspace state to before the specified turn index.
#[tauri::command]
pub async fn rewind_to(
    session_id: String,
    target_turn: usize,
    _app: AppHandle,
    state: State<'_, AppState>,
) -> Result<RewindResult, String> {
    info!(
        session_id = %session_id,
        target_turn = target_turn,
        "Rewinding session"
    );

    // Get the workspace path.
    let workspace = state
        .workspace
        .read()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "No workspace configured".to_string())?;

    // A running session owns its live conversation + file state: rewinding
    // under it would roll back files while the loop keeps writing stale
    // state, desyncing the agent's cognition. Reject while running.
    {
        let sessions = state.sessions.lock().await;
        if sessions.is_streaming(&session_id) {
            return Err("Session is still running — stop the agent before rewinding".to_string());
        }
    }

    // Get the file state tracker for this session.
    let tracker = {
        let trackers = state.file_state_trackers.lock().await;
        trackers
            .get(&session_id)
            .cloned()
            .ok_or_else(|| format!("No file state tracker for session {}", session_id))?
    };

    // Perform the rewind.
    let result = tracker.rewind_to(target_turn, &workspace).await;

    // Persist the post-rewind state (truncated points) so the truncation
    // survives a restart too. Best-effort — a DB failure must not break rewind.
    if let Err(e) = tracker.save_to_db(&session_id, &state.db).await {
        warn!(
            session_id = %session_id,
            error = %e,
            "Failed to persist rewind points after rewind"
        );
    }

    let rewind_result = RewindResult {
        success: result.success,
        restored_files: result
            .restored_files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect(),
        conflicts: result
            .conflicts
            .into_iter()
            .map(|c| RewindConflictInfo {
                path: c.path.to_string_lossy().to_string(),
                conflict_type: format!("{:?}", c.conflict_type),
            })
            .collect(),
        error: result.error,
    };

    if rewind_result.success {
        // The restored workspace invalidates the codebase caches — the next
        // `search_symbols` / `file_dependencies` lookup must rebuild instead
        // of answering from pre-rewind content (same contract as the file
        // watcher and post-write invalidation).
        {
            let mut index = state
                .symbol_index
                .write()
                .unwrap_or_else(|e| e.into_inner());
            index.mark_stale();
            let mut graph = state
                .dependency_graph
                .write()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(graph) = graph.as_mut() {
                graph.mark_stale();
            }
        }
        // ── Agent cognition sync ─────────────────────────────────────
        // The workspace was restored, so the model's view of it must match:
        // truncate the live conversation to the rewind point and tell the
        // model the reverted work is VOID — otherwise the agent keeps
        // reasoning from its stale "I already did X" memory and re-breaks
        // the files. Persisted so a restart keeps the truncation.
        let restored: Vec<String> = rewind_result.restored_files.clone();
        {
            let mut sessions = state.sessions.lock().await;
            if let Ok(chat_state) = sessions.get_chat_state(&session_id) {
                if chat_state.prompt_index >= target_turn {
                    chat_state.truncate_to_prompt_index(target_turn);
                    // The reverted work no longer exists — drop its edited
                    // paths and auto-diagnostics from the record.
                    chat_state.agent_edited_paths.clear();
                    chat_state.auto_diagnostics.clear();
                    chat_state.push_transient_system(format!(
                        "<rewind_notice>\nThe workspace was rewound to checkpoint \
                         #{target_turn} and {} restored file(s) are back to their \
                         checkpoint state: {}.\nYour previous work on these files is \
                         VOID — do not continue from your earlier claims about them. \
                         Re-read the affected files before touching them again, and \
                         treat the restored state as the new ground truth.\n\
                         </rewind_notice>",
                        restored.len(),
                        if restored.is_empty() {
                            "(none — only in-memory state reverted)".to_string()
                        } else {
                            restored.join(", ")
                        }
                    ));
                    if let Err(e) = sessions.persist_messages(&session_id) {
                        warn!(
                            session_id = %session_id,
                            error = %e,
                            "Failed to persist truncated conversation after rewind"
                        );
                    }
                    info!(
                        session_id = %session_id,
                        target_turn,
                        "Rewind: conversation truncated to checkpoint, model notified"
                    );
                }
            }
        }
        info!(
            restored_files = rewind_result.restored_files.len(),
            "Rewind completed successfully"
        );
    } else {
        warn!(
            conflicts = rewind_result.conflicts.len(),
            "Rewind completed with conflicts"
        );
    }

    Ok(rewind_result)
}

/// Get the list of available rewind points for a session.
#[tauri::command]
pub async fn get_rewind_points(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<RewindPointInfo>, String> {
    let trackers = state.file_state_trackers.lock().await;
    let tracker = trackers
        .get(&session_id)
        .ok_or_else(|| format!("No file state tracker for session {}", session_id))?;

    let points = tracker.get_rewind_points().await;

    Ok(points
        .into_iter()
        .map(|p| RewindPointInfo {
            turn_index: p.turn_index,
            created_at: p.created_at.to_rfc3339(),
            file_count: p.before_snapshots.len(),
        })
        .collect())
}

/// Information about a rewind point for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewindPointInfo {
    pub turn_index: usize,
    pub created_at: String,
    pub file_count: usize,
}
