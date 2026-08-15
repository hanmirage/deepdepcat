//! Per-session permission-mode overrides (M18), run-end mode restoration
//! (M50), and per-session state cleanup (M49).
//!
//! The GLOBAL permission mode (user-facing `set_permission_mode` / startup
//! restore) stays the default. Plan-mode machinery (`enter_plan_mode`, the
//! PlanExecute gate) writes a SESSION-scoped override instead, so one
//! session's plan phase can never lock write tools for every other session.
//! An override expires after [`SESSION_MODE_TTL`] and silently falls back to
//! the global mode — a stranded read-only session self-heals even when no
//! cancel/abort path ever fires.

use super::AppState;
use crate::permissions::mode::PermissionMode;
use std::time::{Duration, Instant};
use tauri::Emitter;

/// How long a session-scoped mode override stays fresh. A session parked in
/// plan mode past this (dead loop, lost approval, abandoned run) falls back
/// to the global mode instead of staying read-only forever. Long enough for
/// any real plan phase (the approval wait itself is capped at 10 minutes).
const SESSION_MODE_TTL: Duration = Duration::from_secs(60 * 60);

impl AppState {
    // ── Session-scoped mode override ────────────────────────────────────

    /// The session's own effective mode: its override when fresh, otherwise
    /// the global mode.
    pub async fn session_mode(&self, session_id: &str) -> PermissionMode {
        self.effective_session_mode(session_id, None).await
    }

    /// The effective mode for a session, with subagent inheritance: the
    /// session's own override (fresh) wins, then the parent session's
    /// override (a subagent spawned inside a parent plan phase stays
    /// read-only), then the global mode. Expired entries are pruned as they
    /// are encountered.
    pub async fn effective_session_mode(
        &self,
        session_id: &str,
        parent_session: Option<&str>,
    ) -> PermissionMode {
        let mut overrides = self.session_modes.lock().await;
        let now = Instant::now();
        let mut candidates: Vec<&str> = vec![session_id];
        if let Some(parent) = parent_session {
            if !parent.is_empty() && parent != session_id {
                candidates.push(parent);
            }
        }
        for sid in candidates {
            let Some((mode, set_at)) = overrides.get(sid) else {
                // No live override for this candidate — fall through to its
                // PERSISTED row below (a subagent id has no row, so the
                // parent's row is inherited here).
                let persisted = self.persisted_session_mode(sid).await;
                if let Some(mode) = persisted {
                    return mode;
                }
                continue;
            };
            if now.duration_since(*set_at) < SESSION_MODE_TTL {
                return *mode;
            }
            overrides.remove(sid);
            // Expired override — still check the candidate's persisted row
            // before moving on.
            let persisted = self.persisted_session_mode(sid).await;
            if let Some(mode) = persisted {
                return mode;
            }
        }
        self.permissions.mode()
    }

    /// Read a session's persisted permission-mode row (`""` = inherit the
    /// global default). Subagent ids are not session rows, so this returns
    /// `None` for them — the caller falls through to the parent candidate.
    async fn persisted_session_mode(&self, session_id: &str) -> Option<PermissionMode> {
        // try_lock: called while `session_modes` may already be held; never
        // block on a second lock in a fixed order (deadlock avoidance).
        let mut sessions = self.sessions.try_lock().ok()?;
        let session = sessions.get_session(session_id).ok()?;
        let persisted = session.permission_mode.trim();
        if persisted.is_empty() {
            return None;
        }
        Some(PermissionMode::from_str(persisted))
    }

    /// Set (or refresh) a session-scoped mode override.
    pub async fn set_session_mode(&self, session_id: &str, mode: PermissionMode) {
        self.session_modes
            .lock()
            .await
            .insert(session_id.to_string(), (mode, Instant::now()));
    }

    /// Persist a per-session permission mode (override now + session row so
    /// it survives restarts). Best-effort: DB failure logs and keeps the
    /// in-memory override.
    pub async fn persist_session_mode(&self, session_id: &str, mode: &str) {
        self.set_session_mode(
            session_id,
            crate::permissions::mode::PermissionMode::from_str(mode),
        )
        .await;
        let mut sessions = self.sessions.lock().await;
        if let Err(e) = sessions.set_permission_mode(session_id, mode) {
            tracing::warn!(session_id, error = %e, "Failed to persist session permission mode");
        }
    }

    /// Drop a persisted "plan" row left by older builds. Plan is a transient
    /// posture (read-only planning phase), never a durable session mode; a
    /// stranded row keeps the session read-only across restarts — the
    /// "一直卡在计划模式" symptom. Other persisted modes are untouched (they
    /// are the user's real choices).
    async fn clear_persisted_plan_row(&self, session_id: &str) {
        let needs_clear = {
            let mut sessions = self.sessions.lock().await;
            match sessions.get_session(session_id) {
                Ok(session) => {
                    let persisted = session.permission_mode.trim();
                    !persisted.is_empty()
                        && crate::permissions::mode::PermissionMode::from_str(persisted)
                            == crate::permissions::mode::PermissionMode::ReadOnly
                }
                Err(_) => false,
            }
        };
        if needs_clear {
            let mut sessions = self.sessions.lock().await;
            if let Err(e) = sessions.set_permission_mode(session_id, "") {
                tracing::warn!(session_id, error = %e, "Failed to clear stranded plan mode row");
            }
        }
    }

    /// Drop the session's mode override — it falls back to the global mode.
    pub async fn clear_session_mode(&self, session_id: &str) {
        self.session_modes.lock().await.remove(session_id);
    }

    /// Broadcast the session's current effective mode so the frontend can
    /// reflect a live plan-mode posture (the input-bar indicator). Call AFTER
    /// any mode change so the payload is authoritative; reading the mode here
    /// (rather than taking it as an argument) also covers TTL expiry and
    /// run-end restoration uniformly.
    pub async fn broadcast_plan_mode(&self, app: &tauri::AppHandle, session_id: &str) {
        let mode = self.session_mode(session_id).await;
        let _ = app.emit(
            "plan-mode-changed",
            serde_json::json!({ "session_id": session_id, "mode": mode.as_str() }),
        );
    }

    // ── Run-end restoration (M50) ───────────────────────────────────────

    /// Restore a session's permission state after its run ends — called on
    /// EVERY run end (success, failure, or user cancel) from `send_message`
    /// and from the subagent runner. Without this, a cancelled plan phase
    /// leaves the session read-only forever, the parked approval leaks, and
    /// the "waiting for you" card never closes.
    pub async fn restore_session_after_run(&self, app: &tauri::AppHandle, session_id: &str) {
        // 1. Abandon every parked plan approval for this session (their
        //    waiting tool future belongs to the finished/cancelled run).
        let abandoned: Vec<String> = {
            let mut approvals = self.pending_plan_approvals.lock().await;
            let mut ids = Vec::new();
            approvals.retain(|request_id, pending| {
                if pending.session_id == session_id {
                    ids.push(request_id.clone());
                    false
                } else {
                    true
                }
            });
            ids
        };
        let mut restored = false;
        for request_id in &abandoned {
            self.resolve_pending_interaction(session_id, request_id)
                .await;
        }
        if !abandoned.is_empty() {
            crate::permissions::plan::broadcast_pending_interactions(app, session_id).await;
        }

        // 2. Restore the pre-plan mode (recorded by enter_plan_mode / the
        //    PlanExecute gate) and forget the record. With no record the
        //    override (if any) is dropped — the session falls back to the
        //    global mode (or its parent's override for subagents). The
        //    restored mode is ALSO persisted to the session row so a reload
        //    shows the same posture, and any stranded "plan" row self-heals.
        if let Some(previous) = self.plan_previous_modes.lock().await.remove(session_id) {
            let mode = PermissionMode::from_str(&previous);
            if mode.is_read_only() {
                self.clear_session_mode(session_id).await;
                self.clear_persisted_plan_row(session_id).await;
            } else {
                self.persist_session_mode(session_id, &previous).await;
            }
            restored = true;
        } else {
            self.clear_session_mode(session_id).await;
            self.clear_persisted_plan_row(session_id).await;
        }
        if restored {
            crate::permissions::plan::broadcast_pending_interactions(app, session_id).await;
        }

        // 3. Drop the plan-phase bookkeeping (memory + persisted row).
        self.take_active_plan_steps(session_id).await;

        // 4. Sync the frontend's plan-mode indicator to whatever the session's
        //    effective mode now is — a cancelled/failed run must un-stick it.
        self.broadcast_plan_mode(app, session_id).await;
    }

    /// The single teardown a session run MUST perform on EVERY exit path —
    /// success, tool error, user cancel, or session lost mid-backlog. It
    /// finishes the running-turn registration, drops the cancel/pause
    /// registrations, and restores the session's permission state.
    ///
    /// Entry points that run a session turn must call exactly this (never a
    /// partial copy): a forgotten `restore_session_after_run` leaves a
    /// cancelled plan-phase session read-only, and a missed
    /// `remove_cancellation` leaves a stale token targeting the wrong
    /// invocation. One method owns the invariant so it cannot drift.
    pub async fn finalize_run(&self, app: &tauri::AppHandle, session_id: &str, status: &str) {
        crate::agent::running::finish_running_turn(app, self, session_id, status).await;
        self.remove_cancellation(session_id).await;
        self.remove_pause(session_id).await;
        self.restore_session_after_run(app, session_id).await;
        // A run that executed an approved plan archives the plan Markdown to
        // `.deepdepcat/plans/<session>.md` and tells the frontend to drop it.
        self.archive_approved_plan(app, session_id).await;
    }

    /// Archive an approved plan's raw Markdown to the workspace's
    /// `.deepdepcat/plans/<session>.md` after the executing run completes.
    /// Always signals the frontend to drop its live plan (a rejected or
    /// abandoned plan never reaches the disk write, but the pane must still
    /// clear); the file write itself is best-effort.
    async fn archive_approved_plan(&self, app: &tauri::AppHandle, session_id: &str) {
        if let Some(plan) = self.take_approved_plan_text(session_id).await {
            let Some(ws) = self.workspace.read().ok().and_then(|g| (*g).clone()) else {
                return;
            };
            if let Err(e) = write_plan_archive(&ws, session_id, &plan) {
                tracing::warn!(session_id, error = %e, "archive_approved_plan: failed to write plan file");
            }
            // Hindsight: what the plan predicted vs what actually happened.
            // Fed back to the next plan-writer (via enter_plan_mode) so it
            // calibrates against real outcomes instead of starting blind.
            let steps = self
                .active_plan_steps
                .lock()
                .await
                .get(session_id)
                .cloned()
                .unwrap_or_default();
            let feedback = self.last_reject_feedback.lock().await.remove(session_id);
            let hint: String = plan
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .unwrap_or("")
                .chars()
                .take(80)
                .collect();
            let reflection = crate::permissions::plan::PlanReflection {
                at: crate::permissions::plan::now_secs(),
                session_id: session_id.to_string(),
                plan_hint: hint,
                steps_total: (!steps.is_empty()).then_some(steps.len()),
                steps_done: (!steps.is_empty())
                    .then(|| steps.iter().filter(|s| s.done).count()),
                feedback,
            };
            crate::permissions::plan::append_plan_reflection(&ws, reflection);
        }
        let _ = app.emit(
            "plan-archived",
            serde_json::json!({ "session_id": session_id }),
        );
    }

    // ── Session cleanup (M49) ───────────────────────────────────────────

    /// Purge every per-session map entry for a deleted session. Called on
    /// the session delete paths (UI delete + ACP session/close); the maps
    /// are pure memory and would otherwise grow without bound as sessions
    /// accumulate.
    pub async fn cleanup_session(&self, session_id: &str) {
        if let Some(tracker) = self.usage_trackers.lock().await.remove(session_id) {
            // Flush the deleted session's pending (< FLUSH_THRESHOLD_OPS)
            // deltas before the in-memory atomics are dropped — otherwise
            // its last usage batch is lost on delete.
            tracker.flush_global();
        }
        {
            let mut cache = self.visual_describe_cache.lock().await;
            cache.retain(|(sid, _, _, _), _| sid != session_id);
        }
        self.file_seen_hashes.lock().await.remove(session_id);
        self.prompt_queues.lock().await.remove(session_id);
        self.session_grants.lock().await.remove(session_id);
        self.auto_review_trackers.lock().await.remove(session_id);
        self.plan_previous_modes.lock().await.remove(session_id);
        self.last_reject_feedback.lock().await.remove(session_id);
        self.pending_interactions.lock().await.remove(session_id);
        self.session_outputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(session_id);
        self.file_state_trackers.lock().await.remove(session_id);
        self.take_active_plan_steps(session_id).await;
        self.clear_session_mode(session_id).await;
        // Parked plan approvals of the deleted session are abandoned (the
        // senders drop, the frontend card closes on the next broadcast).
        {
            let mut approvals = self.pending_plan_approvals.lock().await;
            approvals.retain(|_, pending| pending.session_id != session_id);
        }
        // A deleted session must not keep a running loop's registrations.
        if let Some(token) = self.cancellation_tokens.lock().await.remove(session_id) {
            token.cancel();
        }
        self.paused_sessions.lock().await.remove(session_id);
    }
}

/// Write the approved plan Markdown to `<workspace>/.deepdepcat/plans/<session>.md`.
fn write_plan_archive(workspace: &std::path::Path, session_id: &str, plan: &str) -> std::io::Result<()> {
    let dir = workspace.join(".deepdepcat").join("plans");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(format!("{session_id}.md")), plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_plan_archive_creates_deepdepcat_plans_file() {
        let tmp = std::env::temp_dir().join(format!("ddc-plan-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        write_plan_archive(&tmp, "sess-1", "# 计划\n\n1. 步骤").unwrap();

        let path = tmp.join(".deepdepcat").join("plans").join("sess-1.md");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# 计划\n\n1. 步骤"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
