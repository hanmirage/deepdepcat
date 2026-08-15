//! Session-scoped runtime registries — usage trackers, cancellation tokens,
//! pause channels, and pending permission/user-input requests.
//!
//! Split out of `core/state.rs`; the `pub` API of `AppState` is unchanged.

use super::AppState;
use crate::observability::usage::SessionUsageTracker;

impl AppState {
    /// Get or create a usage tracker for a session.
    ///
    /// Every record is mirrored into the durable global aggregate so the
    /// settings usage page accumulates across sessions and restarts.
    pub async fn usage_tracker(&self, session_id: &str) -> SessionUsageTracker {
        let global = crate::storage::database::GlobalUsageStore::new(self.db.clone());
        let mut trackers = self.usage_trackers.lock().await;
        trackers
            .entry(session_id.to_string())
            .or_insert_with(|| SessionUsageTracker::new(session_id).with_global(global.clone()))
            .clone()
    }

    /// Drop the in-memory usage tracker of a session that was evicted as
    /// idle (the durable global aggregate keeps the cumulative totals; the
    /// next touch transparently recreates a fresh tracker).
    pub async fn drop_usage_tracker(&self, session_id: &str) {
        if let Some(tracker) = self.usage_trackers.lock().await.remove(session_id) {
            // Flush the tracker's pending (< FLUSH_THRESHOLD_OPS) deltas
            // before the in-memory atomics are dropped — otherwise the
            // session's last batch is lost on eviction.
            tracker.flush_global();
        }
    }

    /// Register a cancellation token for a session.
    pub async fn register_cancellation(
        &self,
        session_id: &str,
        token: tokio_util::sync::CancellationToken,
    ) {
        self.cancellation_tokens
            .lock()
            .await
            .insert(session_id.to_string(), token);
    }

    /// Cancel a session's current operation.
    pub async fn cancel_session(&self, session_id: &str) -> bool {
        if let Some(token) = self.cancellation_tokens.lock().await.remove(session_id) {
            token.cancel();
            true
        } else {
            false
        }
    }

    /// Remove a cancellation token after completion.
    pub async fn remove_cancellation(&self, session_id: &str) {
        self.cancellation_tokens.lock().await.remove(session_id);
    }

    /// Register the pause channel for a session (initial state: running).
    /// Returns the receiver the agent loop polls at its checkpoints.
    pub async fn register_pause(&self, session_id: &str) -> tokio::sync::watch::Receiver<bool> {
        let (tx, rx) = tokio::sync::watch::channel(false);
        self.paused_sessions
            .lock()
            .await
            .insert(session_id.to_string(), tx);
        rx
    }

    /// Remove the pause channel after the session's run completes.
    pub async fn remove_pause(&self, session_id: &str) {
        self.paused_sessions.lock().await.remove(session_id);
    }

    /// Pause the session's current operation. Returns false if no operation
    /// is running for the session.
    pub async fn pause_session(&self, session_id: &str) -> bool {
        let map = self.paused_sessions.lock().await;
        match map.get(session_id) {
            Some(tx) => {
                if !*tx.borrow() {
                    let _ = tx.send(true);
                }
                self.running_turns.set_paused(session_id, true).await;
                true
            }
            None => false,
        }
    }

    /// Resume the session's paused operation. Returns false if the session
    /// is not paused.
    pub async fn resume_session(&self, session_id: &str) -> bool {
        let map = self.paused_sessions.lock().await;
        match map.get(session_id) {
            Some(tx) => {
                if *tx.borrow() {
                    let _ = tx.send(false);
                }
                self.running_turns.set_paused(session_id, false).await;
                true
            }
            None => false,
        }
    }

    /// Subscribe to the session's pause state (for the agent loop's gate).
    pub async fn session_paused_receiver(
        &self,
        session_id: &str,
    ) -> Option<tokio::sync::watch::Receiver<bool>> {
        self.paused_sessions
            .lock()
            .await
            .get(session_id)
            .map(|tx| tx.subscribe())
    }

    /// Respond to a pending permission request.
    ///
    /// Returns the tool metadata of the resolved request (tool name + args
    /// + session id) so callers can record an "always allow" / session grant.
    pub async fn respond_permission(
        &self,
        request_id: &str,
        allow: bool,
        reason: Option<String>,
    ) -> Option<(String, serde_json::Value, String)> {
        if let Some(pending) = self.pending_permissions.lock().await.remove(request_id) {
            let _ = pending
                .sender
                .send(crate::permissions::grant_store::PermissionReply { allow, reason });
            self.resolve_pending_interaction(&pending.session_id, request_id)
                .await;
            Some((pending.tool_name, pending.args, pending.session_id))
        } else {
            None
        }
    }

    /// Auto-complete pending permission requests that a newly recorded
    /// grant now covers (durable or session-scoped). The user said
    /// "always allow"; every queued request for this session matching the
    /// remembered rule can proceed without another dialog. Requests not
    /// covered (or from other sessions) stay parked.
    pub async fn auto_resolve_pending_permissions(&self, session_id: &str) {
        let durable: Vec<(String, String, bool)> = self
            .grant_store
            .list_grants()
            .iter()
            .map(|g| {
                (
                    g.tool_name.clone(),
                    g.pattern.clone(),
                    g.explicit_whole_tool,
                )
            })
            .collect();
        let session_pairs: Vec<(String, String, bool)> = self
            .session_grants
            .lock()
            .await
            .get(session_id)
            .map(|list| {
                list.iter()
                    .map(|(t, p)| (t.clone(), p.clone(), false))
                    .collect()
            })
            .unwrap_or_default();
        let pairs: Vec<(String, String, bool)> = durable.into_iter().chain(session_pairs).collect();

        let mut pending = self.pending_permissions.lock().await;
        let drained: Vec<(String, crate::permissions::grant_store::PendingPermission)> =
            pending.drain().collect();
        drop(pending);

        let mut unresolved = Vec::new();
        for (request_id, perm) in drained {
            // Sensitive writes are never grant-covered: a new grant must
            // not auto-approve a parked .env/key edit.
            let sensitive =
                crate::permissions::sensitive::is_sensitive_edit_call(&perm.tool_name, &perm.args);
            if !sensitive
                && perm.session_id == session_id
                && crate::permissions::grant_store::grants_cover(
                    &pairs,
                    &perm.tool_name,
                    &perm.args,
                )
            {
                let _ = perm
                    .sender
                    .send(crate::permissions::grant_store::PermissionReply {
                        allow: true,
                        reason: None,
                    });
                self.resolve_pending_interaction(&perm.session_id, &request_id)
                    .await;
            } else {
                unresolved.push((request_id, perm));
            }
        }

        let mut pending = self.pending_permissions.lock().await;
        pending.extend(unresolved);
    }

    /// Register a pending user input request.
    pub async fn register_user_input_request(
        &self,
        request_id: &str,
        sender: tokio::sync::oneshot::Sender<String>,
    ) {
        self.pending_user_inputs
            .lock()
            .await
            .insert(request_id.to_string(), sender);
    }

    /// Respond to a pending user input request.
    pub async fn respond_user_input(&self, request_id: &str, response: String) -> bool {
        if let Some(sender) = self.pending_user_inputs.lock().await.remove(request_id) {
            let _ = sender.send(response);
            true
        } else {
            false
        }
    }

    /// Remove a pending user input request without answering it.
    ///
    /// Cleanup for the timeout / channel-closed paths of the ask_user tool:
    /// the parked sender would otherwise leak in the map for the app's
    /// whole lifetime (the user never answers a prompt that already ended).
    pub async fn remove_user_input_request(&self, request_id: &str) {
        self.pending_user_inputs.lock().await.remove(request_id);
    }

    // ── Unattended (scheduled-run) posture ──────────────────────────────

    /// Mark a session as unattended: permission prompts become denials and
    /// `ask_user` refuses, so a background run can never stall on a human.
    pub async fn mark_unattended(&self, session_id: &str) {
        self.unattended_sessions
            .lock()
            .await
            .insert(session_id.to_string());
    }

    /// Remove the unattended marker after a scheduled run finishes.
    pub async fn unmark_unattended(&self, session_id: &str) {
        self.unattended_sessions.lock().await.remove(session_id);
    }

    /// Whether a session is currently running unattended.
    pub async fn is_unattended(&self, session_id: &str) -> bool {
        self.unattended_sessions
            .lock()
            .await
            .contains(session_id)
    }
}
