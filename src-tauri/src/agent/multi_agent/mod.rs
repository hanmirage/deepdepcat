//! Multi-agent system — subagent spawning, fork mode, and coordination.
//!
//! Implements three delegation patterns:
//! 1. **Subagent** — parent agent delegates a task to a child agent
//! 2. **Fork Subagent** — child inherits parent's conversation context
//! 3. **Coordinator** — a coordinator manages multiple parallel workers

mod coordinator;
mod fork;
mod spawn;
mod types;

#[cfg(test)]
mod tests;

pub use coordinator::{CoordinatorState, WorkerState};
pub use spawn::find_write_conflict;
#[cfg(test)]
pub(crate) use spawn::{GENERAL_SUBAGENT_BODY, SUBAGENT_BOUNDARY_SHELL};
pub use types::*;

use std::collections::HashMap;
use std::sync::Arc;

use crate::agent::chat_state::ChatState;
use crate::agent::context::ContextBuilder;
use crate::hooks::HookExecutor;
use crate::llm::client::LlmClient;
use crate::permissions::PermissionChecker;
use crate::tools::registry::ToolRegistry;
/// The multi-agent coordinator — manages subagent spawning and result collection.
///
/// Holds the shared dependencies needed to construct `AgentLoop` instances
/// for subagents. Each subagent gets its own `ChatState`, filtered tool set,
/// and cancellation token.
#[derive(Clone)]
pub struct MultiAgentCoordinator {
    max_depth: u32,
    enabled: bool,
    llm_client: LlmClient,
    tool_registry: Arc<ToolRegistry>,
    permissions: Arc<PermissionChecker>,
    context_builder: ContextBuilder,
    hook_executor: HookExecutor,
    max_output_chars: usize,
    default_model: String,
    default_provider: Option<String>,
    /// Per-role model matrix (P3-6): plan/explore/verify subagent types use
    /// a dedicated model when configured; `None` = inherit the parent model.
    plan_model: Option<String>,
    explore_model: Option<String>,
    verify_model: Option<String>,
    /// Pending permission requests (shared with AppState).
    pending_permissions: Arc<
        tokio::sync::Mutex<HashMap<String, crate::permissions::grant_store::PendingPermission>>,
    >,
    /// Completed background subagent results, awaiting injection into the
    /// parent's conversation at the start of the next turn.
    background_results: Arc<tokio::sync::Mutex<Vec<BackgroundSubagentResult>>>,
    /// Per-worker cancellation tokens — keyed by worker ID (the task ID for
    /// background subagents) so the `task_stop` tool can cancel an
    /// individual running worker.
    worker_cancels: Arc<tokio::sync::Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    /// task_id → subagent_id aliases for background workers — `task_stop`
    /// receives the tool-facing task ID while the worker state record is
    /// keyed by the internal subagent_id; stop resolves both keys.
    worker_subagent_ids: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
    /// Messages queued for a running worker (SendMessage tool support).
    worker_messages: Arc<tokio::sync::Mutex<HashMap<String, std::collections::VecDeque<String>>>>,
    /// Shared worker state machine — tracks every spawned worker and its
    /// lifecycle for the subagent activity panel.
    worker_state: Arc<CoordinatorState>,
    /// Worktree isolation — when configured, subagents with
    /// `isolation: Worktree` run in a dedicated git worktree.
    worktree_isolation: Option<Arc<crate::workspace::isolation::WorktreeIsolationManager>>,
    /// Global subagent concurrency cap (max_concurrent_tools). Spawning
    /// workers (decomposed or background) acquire a permit for their whole
    /// lifetime so parallel worker storms are bounded.
    worker_concurrency: Option<Arc<tokio::sync::Semaphore>>,
    /// Tool behavior version applied to subagent dispatchers — must match
    /// the parent's version or legacy config behaves differently per-agent.
    behavior_version: crate::toolkit::ToolBehaviorVersion,
    /// Per-subagent tool concurrency cap (each subagent's own dispatcher).
    tool_concurrency: u32,
    /// Durable "always allow" grants — shared with the parent dispatcher.
    grant_store: Arc<crate::permissions::grant_store::PermissionGrantStore>,
}

impl MultiAgentCoordinator {
    /// Create a new coordinator with the necessary dependencies.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        max_depth: u32,
        enabled: bool,
        llm_client: LlmClient,
        tool_registry: Arc<ToolRegistry>,
        permissions: Arc<PermissionChecker>,
        context_builder: ContextBuilder,
        hook_executor: HookExecutor,
        max_output_chars: usize,
        default_model: impl Into<String>,
        default_provider: Option<String>,
        pending_permissions: Arc<
            tokio::sync::Mutex<HashMap<String, crate::permissions::grant_store::PendingPermission>>,
        >,
    ) -> Self {
        Self {
            max_depth,
            enabled,
            llm_client,
            tool_registry,
            permissions,
            context_builder,
            hook_executor,
            max_output_chars,
            default_model: default_model.into(),
            default_provider,
            plan_model: None,
            explore_model: None,
            verify_model: None,
            pending_permissions,
            background_results: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            worker_cancels: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            worker_subagent_ids: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            worker_messages: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            worker_state: Arc::new(CoordinatorState::new()),
            worktree_isolation: None,
            worker_concurrency: None,
            behavior_version: crate::toolkit::ToolBehaviorVersion::Current,
            tool_concurrency: 1,
            grant_store: Arc::new(crate::permissions::grant_store::PermissionGrantStore::default()),
        }
    }

    /// Attach a worktree isolation manager (subagent worktree mode).
    pub fn with_worktree_isolation(
        mut self,
        manager: Arc<crate::workspace::isolation::WorktreeIsolationManager>,
    ) -> Self {
        self.worktree_isolation = Some(manager);
        self
    }

    /// Configure subagent dispatcher defaults: tool behavior version and
    /// per-subagent tool concurrency (mirrors the parent's settings so
    /// subagents behave identically).
    pub fn with_tool_defaults(
        mut self,
        behavior_version: crate::toolkit::ToolBehaviorVersion,
        tool_concurrency: usize,
    ) -> Self {
        self.behavior_version = behavior_version;
        self.tool_concurrency = tool_concurrency.max(1) as u32;
        self
    }

    /// Attach the durable "always allow" grant store (shared with parent).
    pub fn with_grant_store(
        mut self,
        store: Arc<crate::permissions::grant_store::PermissionGrantStore>,
    ) -> Self {
        self.grant_store = store;
        self
    }

    /// Cap concurrent subagent executions with a shared semaphore.
    pub fn with_worker_concurrency(mut self, semaphore: Arc<tokio::sync::Semaphore>) -> Self {
        self.worker_concurrency = Some(semaphore);
        self
    }

    /// The shared worker state machine (subagent activity panel queries this).
    pub fn worker_state(&self) -> Arc<CoordinatorState> {
        self.worker_state.clone()
    }

    /// Snapshot of all tracked workers, newest first (frontend display).
    pub async fn list_active_workers(&self) -> Vec<WorkerState> {
        let mut workers = self.worker_state.list_workers().await;
        workers.sort_by(|a, b| b.worker_id.cmp(&a.worker_id));
        workers
    }

    /// Check if we can spawn a subagent at the given depth.
    pub fn can_spawn(&self, depth: u32) -> bool {
        self.enabled && depth < self.max_depth
    }

    /// Whether multi-agent is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Maximum subagent spawning depth.
    pub fn max_depth(&self) -> u32 {
        self.max_depth
    }

    /// The default model the coordinator uses for subagents and task decomposition.
    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    /// The default provider hint propagated to subagent chat states.
    pub fn default_provider(&self) -> Option<&str> {
        self.default_provider.as_deref()
    }

    /// Resolve the model for a subagent: explicit override → role matrix →
    /// `None` (caller inherits the parent model).
    ///
    /// The role matrix (P3-6) lets users route plan/explore/verify work to
    /// dedicated models — cheap for read-only roles, strong for judging.
    /// The Evaluator role reuses the verify-role model (the same "judge seat"
    /// the loop's Reflexion critique prefers).
    pub fn role_model(&self, agent_type: &SubagentType, explicit: Option<&str>) -> Option<String> {
        if let Some(m) = explicit {
            if !m.is_empty() {
                return Some(m.to_string());
            }
        }
        match agent_type {
            SubagentType::Explore => self.explore_model.clone(),
            SubagentType::Plan => self.plan_model.clone(),
            SubagentType::Evaluator => self.verify_model.clone(),
            SubagentType::General | SubagentType::Custom(_) => None,
        }
    }

    /// The verify-role model (used by the loop's light Reflexion critique).
    pub fn verify_model(&self) -> Option<&str> {
        self.verify_model.as_deref()
    }

    /// Configure the per-role model matrix.
    pub fn with_model_matrix(
        mut self,
        plan_model: Option<String>,
        explore_model: Option<String>,
        verify_model: Option<String>,
    ) -> Self {
        self.plan_model = plan_model.filter(|m| !m.is_empty());
        self.explore_model = explore_model.filter(|m| !m.is_empty());
        self.verify_model = verify_model.filter(|m| !m.is_empty());
        self
    }

    /// Default context window size used when creating minimal parent states.
    pub fn default_context_window(&self) -> u64 {
        128_000
    }

    /// Spawn a background subagent — returns a task ID immediately.
    ///
    /// The subagent runs in a detached `tokio::spawn` task. When it completes,
    /// the result is pushed to an internal queue. The parent agent loop drains
    /// this queue at the start of each turn via `drain_background_results()`.
    /// The returned task ID is registered as the worker key so `task_stop`
    /// and `send_message` can target the background worker mid-flight.
    ///
    /// `cancel_token` is the PARENT SESSION's token (fetched from the
    /// session cancellation registry by the caller) — a user stop or session
    /// teardown must reach background workers too, not leave them burning
    /// tokens detached from the conversation that spawned them.
    ///
    /// `parent_model` is the parent's CURRENT model — the model-resolution
    /// fallback (explicit override → role matrix → parent model) must not
    /// pin background workers to a build-time default snapshot.
    pub fn spawn_background_subagent(
        &self,
        config: SubagentConfig,
        parent_model: String,
        parent_provider: Option<String>,
        session_id: String,
        cancel_token: tokio_util::sync::CancellationToken,
        app: tauri::AppHandle,
    ) -> crate::core::error::AppResult<String> {
        let task_id = crate::core::ids::generate_id();
        let mut config = config;
        config.task_id = Some(task_id.clone());
        let task_description = config.task.clone();
        let surface_completion = config.surface_completion;
        let coordinator = self.clone();
        let results = self.background_results.clone();
        let task_id_clone = task_id.clone();

        let parent_state = ChatState::with_provider(
            config.model.clone().unwrap_or(parent_model),
            self.default_context_window(),
            parent_provider.or_else(|| self.default_provider.clone()),
        );

        let worker_token = cancel_token.child_token();
        tokio::spawn(async move {
            let result = coordinator
                .spawn_subagent_with_cancel(&config, &parent_state, &app, &worker_token)
                .await;

            let bg_result = BackgroundSubagentResult {
                task_id: task_id_clone,
                task: task_description,
                result: result.unwrap_or_else(|e| crate::agent::multi_agent::SubagentResult {
                    response: String::new(),
                    modified_files: vec![],
                    usage: crate::core::types::TokenUsage::default(),
                    success: false,
                    error: Some(e.to_string()),
                }),
                surface_completion,
                session_id,
            };

            results.lock().await.push(bg_result);
        });

        Ok(task_id)
    }

    /// Drain completed background subagent results for one session.
    ///
    /// Called by the agent loop at the start of each turn. Returns results
    /// pushed by completed background tasks of this session since the last
    /// drain; results from other sessions stay queued. The drained subset
    /// is removed from the queue.
    pub async fn drain_background_results(
        &self,
        session_id: &str,
    ) -> Vec<BackgroundSubagentResult> {
        let mut results = self.background_results.lock().await;
        let mut mine: Vec<BackgroundSubagentResult> = Vec::new();
        let mut rest: Vec<BackgroundSubagentResult> = Vec::new();
        for r in results.drain(..) {
            if r.session_id == session_id {
                mine.push(r);
            } else {
                rest.push(r);
            }
        }
        *results = rest;
        mine
    }

    /// Drop all queued background results for a session that is being torn
    /// down — a deleted session never runs another agent-loop turn (the only
    /// thing that drains its queue), so its entries must be purged at
    /// teardown instead of leaking in process memory forever.
    pub async fn purge_background_results(&self, session_id: &str) {
        let mut results = self.background_results.lock().await;
        results.retain(|r| r.session_id != session_id);
    }

    // ── Per-worker lifecycle (SendMessage / TaskStop tools) ─────

    /// Register a running worker with its cancellation token.
    pub async fn register_worker(
        &self,
        worker_id: &str,
        cancel: tokio_util::sync::CancellationToken,
    ) {
        self.worker_cancels
            .lock()
            .await
            .insert(worker_id.to_string(), cancel);
    }

    /// Record the task_id → subagent_id alias of a background worker so
    /// `task_stop` can mark its state record (see [`Self::stop_worker`]).
    pub async fn register_worker_subagent_id(&self, task_id: &str, subagent_id: &str) {
        self.worker_subagent_ids
            .lock()
            .await
            .insert(task_id.to_string(), subagent_id.to_string());
    }

    /// Unregister a worker (called when it finishes or is cancelled).
    pub async fn unregister_worker(&self, worker_id: &str) {
        self.worker_cancels.lock().await.remove(worker_id);
        self.worker_messages.lock().await.remove(worker_id);
        self.worker_subagent_ids.lock().await.remove(worker_id);
    }

    /// Cancel a running worker by ID. Returns `false` if not found.
    pub async fn stop_worker(&self, worker_id: &str) -> bool {
        let token = self.worker_cancels.lock().await.remove(worker_id);
        match token {
            Some(token) => {
                token.cancel();
                self.worker_messages.lock().await.remove(worker_id);
                // Double-key: a background worker is addressable by its
                // tool-facing task_id while its state record is keyed by the
                // internal subagent_id — mark Stopped under BOTH keys so the
                // phase machine / activity panel are never left stuck.
                self.worker_state.stop_worker(worker_id).await;
                if let Some(subagent_id) = self.worker_subagent_ids.lock().await.remove(worker_id) {
                    self.worker_state.stop_worker(&subagent_id).await;
                }
                true
            }
            None => false,
        }
    }

    /// Queue a message for a running worker. Returns `false` if not found.
    pub async fn send_worker_message(&self, worker_id: &str, message: &str) -> bool {
        // Lock order is fixed as worker_cancels → worker_parent_prompts →
        // worker_messages everywhere (see unregister_worker /
        // stop_workers_for_prompt) so concurrent paths can never deadlock.
        let cancels = self.worker_cancels.lock().await;
        if !cancels.contains_key(worker_id) {
            return false;
        }
        let mut messages = self.worker_messages.lock().await;
        messages
            .entry(worker_id.to_string())
            .or_default()
            .push_back(message.to_string());
        drop(messages);
        drop(cancels);
        true
    }

    /// Drain messages queued for a worker (polled between turns).
    pub async fn drain_worker_messages(&self, worker_id: &str) -> Vec<String> {
        let mut messages = self.worker_messages.lock().await;
        messages
            .remove(worker_id)
            .map(|q| q.into_iter().collect())
            .unwrap_or_default()
    }
}
