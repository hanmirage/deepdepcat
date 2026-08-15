//! Coordinator — worker state tracking + the four-phase orchestration
//! state machine for the multi-agent activity panel.
//!
//! The coordinator mode runs prompt-driven (`AgentLoopMode::Coordinator`
//! suffix in agent_loop/mod.rs): the model delegates via the `agent` tool.
//! This module tracks every spawned worker's lifecycle state so the
//! frontend activity panel can display them, and drives the REAL phase
//! machine: workers register under the phase current at spawn time; when
//! every worker of the current phase is terminal the machine advances to
//! the next phase. The loop injects the live phase into the model's
//! context (`<coordinator_phase>`) so the delegation workflow is
//! structural, not just a prompt wish. `reset_if_idle` restarts a fresh
//! orchestration at Research once the previous batch fully finished.
//!
//! Workers are "invisible" — their system prompts don't mention the
//! coordinator or other workers. This prevents workers from trying to
//! communicate directly with each other.
//!
//! The coordinator can send follow-up messages to workers via the
//! `SendMessage` tool and stop workers via `TaskStop`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// The four phases of the coordinator pattern, in workflow order:
/// research (map the problem) → synthesis (design) → implementation
/// (write code) → verification (independent check).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorPhase {
    Research,
    Synthesis,
    Implementation,
    Verification,
}

/// Whether an agent type is a natural fit for the current phase
/// (advisory — see `register_worker_if_no_conflict`).
///
/// Research maps the problem (explore/plan workers), synthesis designs
/// (general), implementation writes (general), verification checks
/// (evaluator). Custom agent types are unknown by design and never match.
fn phase_fits_role(phase: CoordinatorPhase, agent_type: &str) -> bool {
    match phase {
        CoordinatorPhase::Research => matches!(agent_type, "explore" | "plan"),
        CoordinatorPhase::Synthesis => matches!(agent_type, "general" | "explore" | "plan"),
        CoordinatorPhase::Implementation => matches!(agent_type, "general"),
        CoordinatorPhase::Verification => matches!(agent_type, "evaluator"),
    }
}

impl CoordinatorPhase {
    /// Stable string identifier for context injection and event payloads.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Research => "research",
            Self::Synthesis => "synthesis",
            Self::Implementation => "implementation",
            Self::Verification => "verification",
        }
    }

    /// The phase after this one (Verification is terminal).
    fn next(self) -> CoordinatorPhase {
        match self {
            Self::Research => Self::Synthesis,
            Self::Synthesis => Self::Implementation,
            Self::Implementation => Self::Verification,
            Self::Verification => Self::Verification,
        }
    }
}

/// State of a single worker in the coordinator system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerState {
    /// Unique worker ID.
    pub worker_id: String,
    /// The task assigned to this worker.
    pub task: String,
    /// Subagent type label ("explore" | "plan" | "general" | custom name).
    #[serde(default = "default_agent_type")]
    pub agent_type: String,
    /// Current status of the worker.
    pub status: WorkerStatus,
    /// Which phase this worker belongs to.
    pub phase: CoordinatorPhase,
    /// The worker's result (when completed).
    pub result: Option<String>,
    /// File paths the worker declared it intends to WRITE (agent tool
    /// `paths` argument) — used to preflight write conflicts against other
    /// active workers of the same session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub planned_files: Vec<String>,
    /// File paths the worker actually wrote (successful writes only) —
    /// surfaced to the parent's verification/acceptance gates so worker
    /// edits are reviewed like the parent's own edits (a Coordinator turn
    /// that changed files through workers is no longer invisible to the
    /// harness).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edited_files: Vec<String>,
    /// The parent session that spawned this worker — edits are surfaced
    /// per-session (the state machine is process-global across sessions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Unix millis when the worker reached a terminal state (0 = active).
    /// Drives the run-scoped edit collection (`edited_files_since`).
    #[serde(default)]
    pub ended_at_ms: u64,
    /// Unix millis when the worker was registered — drives the live
    /// "elapsed time" display in the activity panel.
    pub started_at_ms: u64,
}

fn default_agent_type() -> String {
    "general".to_string()
}

/// Status of a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Stopped,
}

impl WorkerStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Stopped)
    }
}

/// The coordinator's internal state — tracks all workers and per-session
/// phases.
///
/// The phase machine is BUCKETED BY SESSION: concurrent sessions each run
/// their own four-phase orchestration (a shared global phase would let one
/// session's workers advance another's machine). Workers are keyed by
/// worker_id; a worker's record carries the session that spawned it.
#[derive(Debug)]
pub struct CoordinatorState {
    /// Per-session current phase (absent = Research for that session).
    /// Interior-mutable: shared behind `Arc`, advanced by worker
    /// completions and reset by the loop.
    phases: Mutex<HashMap<String, CoordinatorPhase>>,
    /// All workers, keyed by worker_id.
    workers: Arc<Mutex<HashMap<String, WorkerState>>>,
}

/// Hard cap on tracked workers — long-lived sessions spawn many subagents
/// over time; terminal records beyond this are dropped (oldest first) so
/// the panel state never grows without bound.
const MAX_TRACKED_WORKERS: usize = 200;

/// Bucket key for a session's phase machine — workers without a session id
/// (legacy / test callers) share the default bucket.
fn session_key(session_id: Option<&str>) -> &str {
    session_id.unwrap_or("")
}

impl CoordinatorState {
    /// Create a new coordinator state — every session starts in Research.
    pub fn new() -> Self {
        Self {
            phases: Mutex::new(HashMap::new()),
            workers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The session's current phase (the loop injects it as
    /// `<coordinator_phase>`).
    pub async fn current_phase(&self, session_id: &str) -> CoordinatorPhase {
        self.phase_for(Some(session_id)).await
    }

    /// The current phase of a session's machine, falling back to Research
    /// when the session has never advanced.
    async fn phase_for(&self, session_id: Option<&str>) -> CoordinatorPhase {
        let phases = self.phases.lock().await;
        phases
            .get(session_key(session_id))
            .copied()
            .unwrap_or(CoordinatorPhase::Research)
    }

    /// Restart THIS session's phase machine at Research for a NEW
    /// orchestration.
    ///
    /// Returns `true` when the reset happened (the previous batch of THIS
    /// session's workers fully finished); `false` while any of its workers
    /// are still active — an in-flight orchestration must keep its phase.
    /// Other sessions' workers never block the reset.
    pub async fn reset_if_idle(&self, session_id: &str) -> bool {
        {
            let workers = self.workers.lock().await;
            if workers.values().any(|w| {
                w.session_id.as_deref().unwrap_or("") == session_id && !w.status.is_terminal()
            }) {
                return false;
            }
        }
        self.phases
            .lock()
            .await
            .insert(session_id.to_string(), CoordinatorPhase::Research);
        true
    }

    /// Advance the session's phase when every worker of that session
    /// registered during its current phase has finished. Returns the new
    /// phase, or `None` when the phase did not change (workers still
    /// running, or the machine is already at the terminal phase).
    pub async fn maybe_advance_phase(&self, session_id: &str) -> Option<CoordinatorPhase> {
        let phase = self.phase_for(Some(session_id)).await;
        let all_terminal = {
            let workers = self.workers.lock().await;
            let phase_workers: Vec<&WorkerState> = workers
                .values()
                .filter(|w| w.session_id.as_deref().unwrap_or("") == session_id && w.phase == phase)
                .collect();
            !phase_workers.is_empty() && phase_workers.iter().all(|w| w.status.is_terminal())
        };
        if !all_terminal {
            return None;
        }
        let next = phase.next();
        if next == phase {
            return None;
        }
        self.phases
            .lock()
            .await
            .insert(session_id.to_string(), next);
        Some(next)
    }

    /// Register a new worker under the SESSION's CURRENT phase.
    #[cfg(test)]
    pub async fn register_worker(
        &self,
        worker_id: String,
        task: String,
        agent_type: String,
        session_id: Option<String>,
        planned_files: Vec<String>,
    ) {
        let phase = self.phase_for(session_id.as_deref()).await;
        let mut workers = self.workers.lock().await;
        insert_worker(
            &mut workers,
            phase,
            worker_id,
            task,
            agent_type,
            session_id,
            planned_files,
        );
    }

    /// Atomically check a spawn's declared write paths against the ACTIVE
    /// workers of the same session AND register — the check and the insert
    /// share a single lock, so two parallel spawns declaring the same file
    /// cannot both pass the preflight (a check-then-register split would be
    /// a TOCTOU race). Returns the conflicting paths (and does NOT register)
    /// when a collision is found.
    pub async fn register_worker_if_no_conflict(
        &self,
        worker_id: String,
        task: String,
        agent_type: String,
        session_id: Option<String>,
        planned_files: Vec<String>,
    ) -> Result<(), Vec<String>> {
        let phase = self.phase_for(session_id.as_deref()).await;
        // Phase-role discipline is ADVISORY: the model stays in control of
        // its delegation (a Research phase may legitimately want a
        // general worker), but a mismatch is logged so orchestration drift
        // is observable instead of silent.
        if !phase_fits_role(phase, &agent_type) {
            tracing::warn!(
                worker_id,
                phase = phase.as_str(),
                agent_type,
                "Coordinator worker type does not match the current phase"
            );
        }
        let mut workers = self.workers.lock().await;
        let conflicts =
            collect_conflicts(&workers, session_key(session_id.as_deref()), &planned_files);
        if !conflicts.is_empty() {
            return Err(conflicts);
        }
        insert_worker(
            &mut workers,
            phase,
            worker_id,
            task,
            agent_type,
            session_id,
            planned_files,
        );
        Ok(())
    }

    /// Set a worker's result and mark as completed, recording the files the
    /// worker actually wrote (surfaced to the parent's verification gates).
    pub async fn complete_worker(
        &self,
        worker_id: &str,
        result: String,
        edited_files: Vec<String>,
    ) {
        let session_id = {
            let mut workers = self.workers.lock().await;
            let Some(worker) = workers.get_mut(worker_id) else {
                return;
            };
            worker.result = Some(result);
            worker.status = WorkerStatus::Completed;
            worker.edited_files = edited_files;
            worker.ended_at_ms = now_millis();
            worker.session_id.clone().unwrap_or_default()
        };
        self.maybe_advance_phase(&session_id).await;
    }

    /// Mark a worker as failed (with an optional error message as result),
    /// still recording whatever files it wrote before failing — partial
    /// edits are real edits and must reach the verification gates.
    pub async fn fail_worker(&self, worker_id: &str, error: String, edited_files: Vec<String>) {
        let session_id = {
            let mut workers = self.workers.lock().await;
            let Some(worker) = workers.get_mut(worker_id) else {
                return;
            };
            worker.result = Some(error);
            worker.status = WorkerStatus::Failed;
            worker.edited_files = edited_files;
            worker.ended_at_ms = now_millis();
            worker.session_id.clone().unwrap_or_default()
        };
        self.maybe_advance_phase(&session_id).await;
    }

    /// Mark a worker as stopped (cancelled via task_stop or prompt cancel).
    pub async fn stop_worker(&self, worker_id: &str) {
        let session_id = {
            let mut workers = self.workers.lock().await;
            let Some(worker) = workers.get_mut(worker_id) else {
                return;
            };
            worker.status = WorkerStatus::Stopped;
            worker.ended_at_ms = now_millis();
            worker.session_id.clone().unwrap_or_default()
        };
        self.maybe_advance_phase(&session_id).await;
    }

    /// Force-stop every ACTIVE worker of a session.
    ///
    /// Used when a parent worker's wall-clock timeout DROPS its in-flight
    /// children (their futures are discarded with the parent's, so their own
    /// cleanup — which lives after their run — never executes). The children
    /// run under the parent's subagent_id as their session, so this sweeps
    /// them to Stopped so the activity panel and phase machine converge
    /// instead of showing permanent ghost "Running" records.
    pub async fn stop_workers_for_session(&self, session_id: &str) {
        let mut workers = self.workers.lock().await;
        for w in workers.values_mut() {
            if w.session_id.as_deref() == Some(session_id) && !w.status.is_terminal() {
                w.status = WorkerStatus::Stopped;
                w.ended_at_ms = now_millis();
            }
        }
    }

    /// Collect file edits made by this session's workers that reached a
    /// terminal state AFTER `since_ms`.
    ///
    /// The parent loop merges these into its RUN-scoped edit evidence
    /// (edited_files) so the verification gate and the default acceptance
    /// gate cover worker-written files in Coordinator mode — a turn that
    /// changed files only through workers was previously invisible to the
    /// harness.
    pub async fn edited_files_since(&self, session_id: &str, since_ms: u64) -> Vec<String> {
        let workers = self.workers.lock().await;
        let mut files: Vec<String> = Vec::new();
        for w in workers.values() {
            // `>=`: millisecond resolution means a worker finishing in the
            // same millisecond as the run start would otherwise vanish from
            // the evidence — over-collecting is the safe direction.
            if w.session_id.as_deref() == Some(session_id) && w.ended_at_ms >= since_ms {
                for f in &w.edited_files {
                    if !files.contains(f) {
                        files.push(f.clone());
                    }
                }
            }
        }
        files
    }

    /// Check a spawn's DECLARED write paths against the active workers of
    /// the same session — two parallel workers writing the same file would
    /// race each other's edits. Returns the conflicting paths (empty = no
    /// conflict). Terminal workers are skipped (they are done writing).
    ///
    /// Note: production spawns use `register_worker_if_no_conflict` (atomic
    /// check + register); this read-only variant is for inspection/tests.
    #[cfg(test)]
    pub async fn find_path_conflicts(&self, session_id: &str, planned: &[String]) -> Vec<String> {
        if planned.is_empty() {
            return Vec::new();
        }
        let workers = self.workers.lock().await;
        collect_conflicts(&workers, session_id, planned)
    }

    /// Get all workers (for frontend display).
    pub async fn list_workers(&self) -> Vec<WorkerState> {
        let workers = self.workers.lock().await;
        workers.values().cloned().collect()
    }
}

/// Declared write paths of a spawn that collide with ACTIVE workers of the
/// same session (terminal workers are done writing).
fn collect_conflicts(
    workers: &HashMap<String, WorkerState>,
    session_id: &str,
    planned: &[String],
) -> Vec<String> {
    let mut conflicts: Vec<String> = Vec::new();
    for w in workers.values() {
        if w.session_id.as_deref().unwrap_or("") != session_id || w.status.is_terminal() {
            continue;
        }
        for declared in &w.planned_files {
            if planned.contains(declared) && !conflicts.contains(declared) {
                conflicts.push(declared.clone());
            }
        }
    }
    conflicts
}

/// Insert a worker record under a phase and bound the table: terminal
/// records beyond the cap are dropped (oldest terminal first, by end time).
fn insert_worker(
    workers: &mut HashMap<String, WorkerState>,
    phase: CoordinatorPhase,
    worker_id: String,
    task: String,
    agent_type: String,
    session_id: Option<String>,
    planned_files: Vec<String>,
) {
    let started_at_ms = now_millis();
    workers.insert(
        worker_id.clone(),
        WorkerState {
            worker_id,
            task,
            agent_type,
            status: WorkerStatus::Pending,
            phase,
            result: None,
            planned_files,
            edited_files: Vec::new(),
            session_id,
            ended_at_ms: 0,
            started_at_ms,
        },
    );
    if workers.len() > MAX_TRACKED_WORKERS {
        let overflow = workers.len() - MAX_TRACKED_WORKERS;
        // Evict the OLDEST terminal workers first. `std::collections::HashMap`
        // iterates in arbitrary (hash) order, so a plain `retain` that drops
        // "the first N terminal records" could evict a just-finished worker
        // whose `edited_files` the parent's verification gate still needs,
        // while stale, already-consumed workers are kept. Sort terminal
        // workers by their end time (falling back to start time) and drop the
        // oldest.
        let mut terminal: Vec<(String, u64)> = workers
            .iter()
            .filter(|(_, w)| w.status.is_terminal())
            .map(|(id, w)| {
                let t = if w.ended_at_ms > 0 {
                    w.ended_at_ms
                } else {
                    w.started_at_ms
                };
                (id.clone(), t)
            })
            .collect();
        terminal.sort_by_key(|(_, t)| *t);
        for (id, _) in terminal.into_iter().take(overflow) {
            workers.remove(&id);
        }
    }
}

/// Unix millis (0 on clock failure — a failed clock makes `edited_files_since`
/// behave as "everything counts", the safe direction).
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Default for CoordinatorState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn register_and_complete_worker() {
        let state = CoordinatorState::new();
        state
            .register_worker(
                "w1".into(),
                "explore src/".into(),
                "explore".into(),
                None,
                vec![],
            )
            .await;
        let workers = state.list_workers().await;
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].status, WorkerStatus::Pending);

        state
            .complete_worker("w1", "found 3 files".into(), vec![])
            .await;

        let workers = state.list_workers().await;
        assert_eq!(workers[0].status, WorkerStatus::Completed);
        assert_eq!(workers[0].result, Some("found 3 files".to_string()));
    }

    #[tokio::test]
    async fn worker_edits_surface_per_session_and_run() {
        let state = CoordinatorState::new();
        // Two sessions spawn workers in parallel (the state machine is
        // process-global); each worker records what it wrote.
        state
            .register_worker(
                "w-a".into(),
                "impl".into(),
                "general".into(),
                Some("s1".into()),
                vec![],
            )
            .await;
        state
            .register_worker(
                "w-b".into(),
                "impl".into(),
                "general".into(),
                Some("s2".into()),
                vec![],
            )
            .await;
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        state
            .complete_worker(
                "w-a",
                "done".into(),
                vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
            )
            .await;
        state
            .complete_worker("w-b", "done".into(), vec!["src/other.rs".to_string()])
            .await;

        // Session-scoped + run-scoped: s1 sees only its own worker's edits.
        let s1 = state.edited_files_since("s1", before).await;
        assert!(s1.contains(&"src/a.rs".to_string()));
        assert!(s1.contains(&"src/b.rs".to_string()));
        assert!(
            !s1.contains(&"src/other.rs".to_string()),
            "no cross-session edits"
        );
        // A run that started AFTER the workers finished sees nothing
        // (future timestamp — strictly past the workers' ended_at_ms).
        let later = now_millis() + 1000;
        assert!(state.edited_files_since("s1", later).await.is_empty());
    }

    #[test]
    fn phase_role_matrix_is_advisory_and_typed() {
        // Research maps the problem — explore/plan fit, implementers do not.
        assert!(phase_fits_role(CoordinatorPhase::Research, "explore"));
        assert!(phase_fits_role(CoordinatorPhase::Research, "plan"));
        assert!(!phase_fits_role(CoordinatorPhase::Research, "general"));
        assert!(!phase_fits_role(CoordinatorPhase::Research, "evaluator"));
        // Implementation writes — general fits, exploration does not.
        assert!(phase_fits_role(CoordinatorPhase::Implementation, "general"));
        assert!(!phase_fits_role(
            CoordinatorPhase::Implementation,
            "explore"
        ));
        // Verification checks — evaluator only.
        assert!(phase_fits_role(CoordinatorPhase::Verification, "evaluator"));
        assert!(!phase_fits_role(CoordinatorPhase::Verification, "general"));
        // Synthesis is the design seat — a few types are reasonable.
        assert!(phase_fits_role(CoordinatorPhase::Synthesis, "general"));
        assert!(phase_fits_role(CoordinatorPhase::Synthesis, "explore"));
        // Custom agent types never match (unknown by design).
        assert!(!phase_fits_role(CoordinatorPhase::Implementation, "custom"));
        assert!(!phase_fits_role(CoordinatorPhase::Research, "custom"));
    }

    #[tokio::test]
    async fn failed_workers_surface_partial_edits() {
        let state = CoordinatorState::new();
        state
            .register_worker(
                "w1".into(),
                "impl".into(),
                "general".into(),
                Some("s1".into()),
                vec![],
            )
            .await;
        let before = now_millis();
        state
            .fail_worker("w1", "boom".into(), vec!["src/partial.rs".to_string()])
            .await;
        let files = state.edited_files_since("s1", before).await;
        assert!(
            files.contains(&"src/partial.rs".to_string()),
            "partial writes from a failed worker are still real edits"
        );
    }

    #[tokio::test]
    async fn path_conflicts_only_against_active_same_session_workers() {
        let state = CoordinatorState::new();
        // w-a (active, s1) owns src/shared.rs; w-b (active, s2) owns the same
        // file — but cross-session, so it must not conflict with s1 spawns.
        state
            .register_worker(
                "w-a".into(),
                "impl".into(),
                "general".into(),
                Some("s1".into()),
                vec!["src/shared.rs".to_string()],
            )
            .await;
        state
            .register_worker(
                "w-b".into(),
                "impl".into(),
                "general".into(),
                Some("s2".into()),
                vec!["src/shared.rs".to_string()],
            )
            .await;
        // Same-session collision is rejected.
        let conflicts = state
            .find_path_conflicts("s1", &["src/shared.rs".to_string()])
            .await;
        assert_eq!(conflicts, vec!["src/shared.rs".to_string()]);
        // Cross-session is fine.
        assert!(state
            .find_path_conflicts("s3", &["src/shared.rs".to_string()])
            .await
            .is_empty());
        // No overlap with a different path.
        assert!(state
            .find_path_conflicts("s1", &["src/other.rs".to_string()])
            .await
            .is_empty());
        // Terminal workers are done writing — no conflict after completion.
        state.complete_worker("w-a", "done".into(), vec![]).await;
        assert!(state
            .find_path_conflicts("s1", &["src/shared.rs".to_string()])
            .await
            .is_empty());
        // No declared paths → never conflicts.
        assert!(state.find_path_conflicts("s1", &[]).await.is_empty());
    }

    #[tokio::test]
    async fn phase_advances_when_all_phase_workers_terminal() {
        let state = CoordinatorState::new();
        assert_eq!(state.current_phase("s1").await, CoordinatorPhase::Research);
        // Workers register under the CURRENT phase of THEIR session —
        // research workers here.
        state
            .register_worker(
                "w1".into(),
                "explore".into(),
                "explore".into(),
                Some("s1".into()),
                vec![],
            )
            .await;
        state
            .register_worker(
                "w2".into(),
                "explore 2".into(),
                "explore".into(),
                Some("s1".into()),
                vec![],
            )
            .await;
        assert_eq!(state.current_phase("s1").await, CoordinatorPhase::Research);
        // One of two still running → no advance.
        state.complete_worker("w1", "found".into(), vec![]).await;
        assert_eq!(state.current_phase("s1").await, CoordinatorPhase::Research);
        // Last one terminal → advance to synthesis.
        state.complete_worker("w2", "found 2".into(), vec![]).await;
        assert_eq!(state.current_phase("s1").await, CoordinatorPhase::Synthesis);
        // A failed worker also counts as terminal — the phase must not
        // block on a worker that can never finish.
        state
            .register_worker(
                "w3".into(),
                "implement".into(),
                "general".into(),
                Some("s1".into()),
                vec![],
            )
            .await;
        state.fail_worker("w3", "boom".into(), vec![]).await;
        assert_eq!(
            state.current_phase("s1").await,
            CoordinatorPhase::Implementation
        );
        // Stopped workers advance the phase too.
        state
            .register_worker(
                "w4".into(),
                "verify".into(),
                "general".into(),
                Some("s1".into()),
                vec![],
            )
            .await;
        state.stop_worker("w4").await;
        assert_eq!(
            state.current_phase("s1").await,
            CoordinatorPhase::Verification
        );
        // Terminal phase stays terminal.
        state
            .register_worker(
                "w5".into(),
                "late".into(),
                "general".into(),
                Some("s1".into()),
                vec![],
            )
            .await;
        state.complete_worker("w5", "x".into(), vec![]).await;
        assert_eq!(
            state.current_phase("s1").await,
            CoordinatorPhase::Verification
        );
    }

    #[tokio::test]
    async fn phase_does_not_advance_without_workers() {
        // A phase with no workers never blocks the machine — and an empty
        // machine never advances (nothing to measure).
        let state = CoordinatorState::new();
        assert!(state.maybe_advance_phase("s1").await.is_none());
        assert_eq!(state.current_phase("s1").await, CoordinatorPhase::Research);
    }

    #[tokio::test]
    async fn reset_happens_only_when_idle() {
        let state = CoordinatorState::new();
        state
            .register_worker(
                "w1".into(),
                "t".into(),
                "general".into(),
                Some("s1".into()),
                vec![],
            )
            .await;
        assert!(
            !state.reset_if_idle("s1").await,
            "an active worker must block the reset"
        );
        state.complete_worker("w1", "done".into(), vec![]).await;
        assert!(state.reset_if_idle("s1").await, "idle machine resets");
        assert_eq!(state.current_phase("s1").await, CoordinatorPhase::Research);
    }

    #[tokio::test]
    async fn stop_workers_for_session_clears_active_children() {
        // A parent that times out DROPS its in-flight children — their own
        // cleanup never runs, leaving ghost "Running" records. The sweep
        // force-stops them so the panel converges.
        let state = CoordinatorState::new();
        state
            .register_worker(
                "parent".into(),
                "impl".into(),
                "general".into(),
                Some("s1".into()),
                vec![],
            )
            .await;
        // Children run under the parent's subagent_id as their session.
        state
            .register_worker(
                "child-a".into(),
                "sub 1".into(),
                "general".into(),
                Some("parent".into()),
                vec![],
            )
            .await;
        state
            .register_worker(
                "child-b".into(),
                "sub 2".into(),
                "general".into(),
                Some("parent".into()),
                vec![],
            )
            .await;
        // The parent itself completed normally.
        state.complete_worker("parent", "done".into(), vec![]).await;

        // Timeout sweep on the parent's id — both children go Stopped.
        state.stop_workers_for_session("parent").await;
        let workers = state.list_workers().await;
        let child_a = workers.iter().find(|w| w.worker_id == "child-a").unwrap();
        let child_b = workers.iter().find(|w| w.worker_id == "child-b").unwrap();
        assert_eq!(child_a.status, WorkerStatus::Stopped);
        assert_eq!(child_b.status, WorkerStatus::Stopped);
        // The completed parent is untouched.
        let parent = workers.iter().find(|w| w.worker_id == "parent").unwrap();
        assert_eq!(parent.status, WorkerStatus::Completed);
    }

    #[tokio::test]
    async fn phase_machines_are_bucketed_per_session() {
        // Two concurrent sessions must not advance each other's phase.
        let state = CoordinatorState::new();
        state
            .register_worker(
                "a1".into(),
                "explore".into(),
                "explore".into(),
                Some("s1".into()),
                vec![],
            )
            .await;
        state.complete_worker("a1", "found".into(), vec![]).await;
        assert_eq!(
            state.current_phase("s1").await,
            CoordinatorPhase::Synthesis,
            "s1 advanced past research"
        );
        assert_eq!(
            state.current_phase("s2").await,
            CoordinatorPhase::Research,
            "s2 is untouched by s1's advancement"
        );
        // s2's active worker blocks its own reset but not s1's.
        state
            .register_worker(
                "b1".into(),
                "impl".into(),
                "general".into(),
                Some("s2".into()),
                vec![],
            )
            .await;
        assert!(state.reset_if_idle("s1").await, "s1's workers are terminal");
        assert!(
            !state.reset_if_idle("s2").await,
            "s2 still has an active worker"
        );
    }

    #[tokio::test]
    async fn atomic_register_rejects_same_file_parallel_claims() {
        // Two parallel spawns declaring the same file: the second must be
        // rejected atomically — and must NOT leave a record behind.
        let state = CoordinatorState::new();
        assert!(state
            .register_worker_if_no_conflict(
                "w1".into(),
                "impl".into(),
                "general".into(),
                Some("s1".into()),
                vec!["src/shared.rs".to_string()],
            )
            .await
            .is_ok());
        let err = state
            .register_worker_if_no_conflict(
                "w2".into(),
                "impl 2".into(),
                "general".into(),
                Some("s1".into()),
                vec!["src/shared.rs".to_string()],
            )
            .await
            .unwrap_err();
        assert_eq!(err, vec!["src/shared.rs".to_string()]);
        let workers = state.list_workers().await;
        assert_eq!(
            workers.len(),
            1,
            "the rejected spawn must not be registered"
        );
        // A different file in the same session passes.
        assert!(state
            .register_worker_if_no_conflict(
                "w3".into(),
                "impl 3".into(),
                "general".into(),
                Some("s1".into()),
                vec!["src/other.rs".to_string()],
            )
            .await
            .is_ok());
        // Cross-session claims on the same file are allowed.
        assert!(state
            .register_worker_if_no_conflict(
                "w4".into(),
                "impl 4".into(),
                "general".into(),
                Some("s2".into()),
                vec!["src/shared.rs".to_string()],
            )
            .await
            .is_ok());
    }

    #[test]
    fn phase_labels_are_stable() {
        assert_eq!(CoordinatorPhase::Research.as_str(), "research");
        assert_eq!(CoordinatorPhase::Synthesis.as_str(), "synthesis");
        assert_eq!(CoordinatorPhase::Implementation.as_str(), "implementation");
        assert_eq!(CoordinatorPhase::Verification.as_str(), "verification");
    }
}
