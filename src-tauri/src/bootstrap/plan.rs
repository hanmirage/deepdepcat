//! Plan-approval flow and session-grant registries — parked approvals,
//! previous-mode records, active plan steps, per-session grants, and the
//! unified "waiting for you" interaction list.
//!
//! Split out of `core/state.rs`; the `pub` API of `AppState` is unchanged.

use super::AppState;
use crate::permissions::grant_store::{extract_pattern, grants_cover};
use crate::permissions::plan::{now_secs, PendingInteraction, PendingPlanApproval, PlanStep};

impl AppState {
    // ── Plan approval ───────────────────────────────────────────────────

    /// Park a plan-approval request (exit_plan_mode blocks on it).
    pub async fn register_plan_approval(
        &self,
        request_id: &str,
        sender: tokio::sync::oneshot::Sender<crate::permissions::plan::PlanDecision>,
        session_id: &str,
    ) {
        self.pending_plan_approvals.lock().await.insert(
            request_id.to_string(),
            PendingPlanApproval {
                sender,
                session_id: session_id.to_string(),
            },
        );
    }

    /// Resolve a parked plan approval (frontend decision). Returns the
    /// session id of the resolved request, if any.
    pub async fn respond_plan_approval(
        &self,
        request_id: &str,
        decision: crate::permissions::plan::PlanDecision,
    ) -> Option<String> {
        if let Some(pending) = self.pending_plan_approvals.lock().await.remove(request_id) {
            // Remember the user's pushback so the run-end reflection can tell
            // the next plan-writer what was rejected before this plan passed.
            if let crate::permissions::plan::PlanDecision::Rejected(feedback) = &decision {
                if !feedback.trim().is_empty() {
                    self.last_reject_feedback
                        .lock()
                        .await
                        .insert(pending.session_id.clone(), feedback.clone());
                }
            }
            let _ = pending.sender.send(decision);
            self.resolve_pending_interaction(&pending.session_id, request_id)
                .await;
            Some(pending.session_id)
        } else {
            None
        }
    }

    /// Abandon a parked plan approval (timeout / channel closed) — drops
    /// the sender so the waiting tool future resolves, and clears the
    /// interaction. Returns the session id, if any.
    pub async fn abandon_plan_approval(&self, request_id: &str) -> Option<String> {
        if let Some(pending) = self.pending_plan_approvals.lock().await.remove(request_id) {
            self.resolve_pending_interaction(&pending.session_id, request_id)
                .await;
            Some(pending.session_id)
        } else {
            None
        }
    }

    /// Remember the mode to restore after plan approval.
    pub async fn set_plan_previous_mode(&self, session_id: &str, mode: String) {
        self.plan_previous_modes
            .lock()
            .await
            .insert(session_id.to_string(), mode);
    }

    /// Take (and forget) the mode to restore after plan approval.
    pub async fn take_plan_previous_mode(&self, session_id: &str) -> Option<String> {
        self.plan_previous_modes.lock().await.remove(session_id)
    }

    // ── Active plan steps (structured planner, P2-5) ────────────────────

    /// Store the parsed steps of an approved plan for a session.
    ///
    /// Persisted to the session row so the plan gate survives app restarts;
    /// a db write failure degrades to memory-only (the gate is a nudge, not
    /// a safety boundary).
    pub async fn set_active_plan_steps(&self, session_id: &str, steps: Vec<PlanStep>) {
        let json = serde_json::to_string(&steps).ok();
        let payload = json
            .as_deref()
            .filter(|j| *j != "[]")
            .map(|j| j.to_string());
        if let Err(e) = self
            .db
            .set_session_plan_steps(session_id, payload.as_deref())
        {
            tracing::warn!(session_id = session_id, error = %e, "set_active_plan_steps: persist failed");
        }

        let mut plans = self.active_plan_steps.lock().await;
        if steps.is_empty() {
            plans.remove(session_id);
        } else {
            plans.insert(session_id.to_string(), steps);
        }
    }

    /// Take (and forget) the session's approved plan steps.
    ///
    /// When the in-memory map is empty (e.g. the app restarted since the
    /// plan was approved), the persisted row is consulted and cleared — a
    /// resumed session still gets its approved-plan checklist gate.
    pub async fn take_active_plan_steps(&self, session_id: &str) -> Option<Vec<PlanStep>> {
        let removed = self.active_plan_steps.lock().await.remove(session_id);
        let steps = removed.or_else(|| self.restore_persisted_plan_steps(session_id));
        if steps.is_some() {
            if let Err(e) = self.db.set_session_plan_steps(session_id, None) {
                tracing::warn!(session_id = session_id, error = %e, "take_active_plan_steps: clear failed");
            }
        }
        steps
    }

    /// Read and clear the persisted plan steps row (restart recovery).
    fn restore_persisted_plan_steps(&self, session_id: &str) -> Option<Vec<PlanStep>> {
        let raw = self.db.get_session_plan_steps(session_id).ok().flatten()?;
        match serde_json::from_str::<Vec<PlanStep>>(&raw) {
            Ok(steps) if !steps.is_empty() => Some(steps),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(session_id = session_id, error = %e, "take_active_plan_steps: persisted steps unparseable");
                None
            }
        }
    }

    // ── Approved-plan archive (raw plan MD → .deepdepcat/plans) ───────

    /// Remember the raw plan Markdown a session was approved with, so the
    /// run-end hook can archive it to disk after execution completes.
    pub async fn set_approved_plan_text(&self, session_id: &str, plan: String) {
        self.approved_plan_text
            .lock()
            .await
            .insert(session_id.to_string(), plan);
    }

    /// Take (and forget) the session's raw approved-plan text.
    pub async fn take_approved_plan_text(&self, session_id: &str) -> Option<String> {
        self.approved_plan_text.lock().await.remove(session_id)
    }

    // ── Session grants ──────────────────────────────────────────────────

    /// Record an "allow for this session" grant (pure memory).
    pub async fn record_session_grant(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) {
        self.record_session_grant_pattern(session_id, tool_name, &extract_pattern(tool_name, args))
            .await;
    }

    /// Record an "allow for this session" grant for an explicit pattern
    /// (`*` = whole tool). Pure memory, scoped to the session.
    pub async fn record_session_grant_pattern(
        &self,
        session_id: &str,
        tool_name: &str,
        pattern: &str,
    ) {
        let mut grants = self.session_grants.lock().await;
        let entry = grants.entry(session_id.to_string()).or_default();
        entry.retain(|(t, p)| !(t == tool_name && p == pattern));
        entry.push((tool_name.to_string(), pattern.to_string()));
    }

    /// Whether a session grant covers this tool call.
    ///
    /// Session grants mirror the durable grants' rules (`grants_cover`):
    /// dangerous bash is never covered, a bash command is only covered when
    /// every statement is covered, and MCP grants are server-scoped.
    pub async fn session_grant_allows(
        &self,
        session_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> bool {
        let grants = self.session_grants.lock().await;
        let pairs: Vec<(String, String, bool)> = grants
            .get(session_id)
            .map(|list| {
                list.iter()
                    .map(|(t, p)| (t.clone(), p.clone(), false))
                    .collect()
            })
            .unwrap_or_default();
        grants_cover(&pairs, tool_name, args)
    }

    /// Forget all session grants for a session (new session, cleanup).
    pub async fn clear_session_grants(&self, session_id: &str) {
        self.session_grants.lock().await.remove(session_id);
    }

    // ── Pending interactions (unified "waiting for you" status) ─────────

    /// Record an interaction the user must answer for this session.
    pub async fn register_pending_interaction(
        &self,
        session_id: &str,
        kind: &'static str,
        request_id: &str,
        summary: String,
    ) {
        let mut all = self.pending_interactions.lock().await;
        let list = all.entry(session_id.to_string()).or_default();
        list.retain(|i| i.request_id != request_id);
        list.push(PendingInteraction {
            kind,
            request_id: request_id.to_string(),
            summary,
            since: now_secs(),
        });
    }

    /// Mark an interaction resolved (the user answered).
    pub async fn resolve_pending_interaction(&self, session_id: &str, request_id: &str) {
        let mut all = self.pending_interactions.lock().await;
        if let Some(list) = all.get_mut(session_id) {
            list.retain(|i| i.request_id != request_id);
        }
    }

    /// Snapshot of the session's pending interactions (for the frontend).
    pub async fn pending_interactions_snapshot(&self, session_id: &str) -> Vec<PendingInteraction> {
        self.pending_interactions
            .lock()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Resolve every queued permission request according to the NEWLY
    /// selected permission mode — mode switches must not leave stale dialogs
    /// hanging while the agent waits for an answer that will never come:
    /// - Read-only             → deny all (nothing write-related may proceed)
    /// - Full access           → allow all (the old drain behavior)
    /// - Accept-edits          → allow file-edit tools, deny everything else
    ///
    /// Resolved requests are one-time decisions, never recorded as grants.
    /// Returns the affected session ids so callers can re-broadcast the
    /// pending-interactions status.
    pub async fn resolve_permission_requests_for_mode(
        &self,
        mode: crate::permissions::mode::PermissionMode,
    ) -> Vec<String> {
        let mut affected: Vec<String> = Vec::new();
        let mut pending = self.pending_permissions.lock().await;
        let drained: Vec<_> = pending.drain().collect();
        drop(pending);
        for (request_id, perm) in drained {
            // Sensitive writes always stay parked for a real user decision
            // — switching to full access or accept-edits must not silently
            // approve a .env/key edit that was waiting on a prompt.
            let sensitive =
                crate::permissions::sensitive::is_sensitive_edit_call(&perm.tool_name, &perm.args);
            let allow = match mode {
                crate::permissions::mode::PermissionMode::AcceptEdits => {
                    !sensitive
                        && crate::permissions::rules::RuleSet::is_accept_edits_tool(&perm.tool_name)
                }
                crate::permissions::mode::PermissionMode::FullAccess => !sensitive,
                _ => false, // ReadOnly
            };
            let _ = perm
                .sender
                .send(crate::permissions::grant_store::PermissionReply {
                    allow,
                    reason: None,
                });
            self.resolve_pending_interaction(&perm.session_id, &request_id)
                .await;
            if !affected.contains(&perm.session_id) {
                affected.push(perm.session_id);
            }
        }
        affected
    }
}
