//! Loop exit housekeeping — AgentLoopEnd hook + turn tracking, shared by
//! every exit path.

use super::AgentLoop;
use crate::core::types::{emit_debug_trace, DebugEvent};
use crate::hooks::{HookContext, HookEvent};
use crate::workspace::checkpoint::FileStateTracker;
use tauri::{AppHandle, Manager};

impl AgentLoop {
    /// Emit the AgentLoopEnd hook and end turn tracking — runs on the normal
    /// exit path AND on early returns (budget / final answer) so the file
    /// state snapshots are never left dangling.
    pub(super) async fn finish_loop_housekeeping(
        &self,
        app: &AppHandle,
        session_id: &str,
        turn_index: usize,
        debug_mode: bool,
        file_state_tracker: &Option<FileStateTracker>,
    ) {
        // Trigger AgentLoopEnd hook (observe-only)
        let loop_end_ctx = HookContext::new(HookEvent::AgentLoopEnd, session_id);
        emit_debug_trace(
            app,
            debug_mode,
            DebugEvent::hook_trigger(session_id, "AgentLoopEnd"),
        );
        self.hook_executor.execute_observe(&loop_end_ctx).await;

        // End turn tracking — capture after-snapshots for all touched files.
        if let Some(ref tracker) = file_state_tracker {
            tracker.end_turn(turn_index).await;

            // Persist rewind points to the database so they survive restarts.
            // Best-effort: a DB failure must never break the loop exit path.
            if let Some(state) = app.try_state::<crate::bootstrap::AppState>() {
                let db = state.db.clone();
                if let Err(e) = tracker.save_to_db(session_id, &db).await {
                    tracing::warn!(session_id, error = %e, "Failed to persist rewind points");
                }
            }
        }
    }
}
