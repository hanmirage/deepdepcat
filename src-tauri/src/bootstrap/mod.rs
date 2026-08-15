//! Global application state — shared across all Tauri commands.
//!
//! Uses `Arc<RwLock>` for thread-safe concurrent access. This is the central
//! hub that every subsystem reads from and writes to.
//!
//! The struct and its field definitions live here; the `impl AppState`
//! blocks are split across the submodules by concern:
//! - `init` — construction and subsystem wiring
//! - `lifecycle` — skills reload, async post-init, feature flags
//! - `session` — per-session runtime registries (usage, cancel, pause, …)
//! - `plan` — plan-approval flow, session grants, pending interactions
//! - `mode` — per-session permission-mode overrides, run-end restore, cleanup

use crate::agent::multi_agent::MultiAgentCoordinator;
use crate::agent::session::SessionManager;
use crate::codebase::dependency::DependencyGraph;
use crate::codebase::symbols::SymbolIndex;
use crate::core::config::AppConfig;
use crate::core::error::AppResult;
use crate::core::types::AgentStatus;
use crate::hooks::HookRegistry;
use crate::llm::circuit_breaker::CircuitBreaker;
use crate::mcp::manager::McpManager;
use crate::memory::embedding::EmbeddingProvider;
use crate::memory::injection::MemoryInjector;
use crate::memory::search::MemorySearcher;
use crate::memory::store::MemoryStore;
use crate::observability::diagnostics::DiagnosticsReporter;
use crate::observability::usage::SessionUsageTracker;
use crate::permissions::PermissionChecker;
use crate::storage::database::Database;
use crate::task::TaskManager;
use crate::tools::registry::ToolRegistry;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;

/// Per-session file-content fingerprints — session_id → (workspace, path) → FNV-1a hash.
type FileSeenHashMap = HashMap<String, HashMap<(PathBuf, PathBuf), u64>>;

mod init;
mod lifecycle;
mod mode;
mod plan;
mod session;
/// Startup wiring — the composition root (one call from `lib.rs` setup).
pub mod startup;

/// Maximum distinct sessions that may run an agent loop concurrently. A burst
/// of parallel sessions each hammering the LLM API would otherwise risk 429s
/// and a runaway token budget.
pub(crate) const MAX_CONCURRENT_SESSIONS: usize = 3;

/// Per-session, per-image transcription cache — (session_id, image bytes,
/// prompt, region) → the vision model's description and when it was produced.
/// The same image asked the same question at the same zoom level is described
/// once per session; later calls short-circuit. A different prompt (e.g. a
/// `visual_describe` tool question) or a different crop region gets its own
/// fresh answer.
pub type VisualDescribeCache =
    HashMap<(String, Vec<u8>, String, String), (String, std::time::Instant)>;

/// The global application state, managed by Tauri's `.manage()`.
///
/// All fields are behind `Arc` so they can be cheaply cloned into
/// background tasks spawned by commands.
#[derive(Clone)]
pub struct AppState {
    // ── Core infra ──────────────────────────────────────────────────────
    /// Application configuration (loaded at startup).
    pub config: Arc<RwLock<AppConfig>>,
    /// SQLite database connection.
    pub db: Arc<Database>,
    /// Session manager (chat sessions).
    pub sessions: Arc<Mutex<SessionManager>>,
    /// Session-level concurrency cap — bounds how many distinct sessions may
    /// run an agent loop at the same time. Prevents an N-session burst from
    /// hammering the LLM API with N parallel loops (429s, runaway token
    /// budget). Acquired in `send_message` after `take_chat_state` succeeds,
    /// released at the single exit; same-session queued replays share the
    /// permit.
    pub session_concurrency: Arc<tokio::sync::Semaphore>,
    /// Tool registry (available tools).
    pub tools: Arc<ToolRegistry>,
    /// Permission checker.
    pub permissions: Arc<PermissionChecker>,
    /// Hook registry.
    pub hooks: Arc<RwLock<HookRegistry>>,
    /// Hook executor — fires observe/blocking hook events from the runtime
    /// (permission events, notifications, batch boundaries).
    pub hook_executor: crate::hooks::HookExecutor,
    /// Hash-based hook trust store (untrusted hooks never execute).
    pub hook_trust: Arc<crate::hooks::trust::HookTrustStore>,
    // ── Memory (store / embedding / searcher / injection) ────────────────
    /// Memory store (low-level SQLite CRUD).
    pub memory: Arc<MemoryStore>,
    /// Embedding provider (generates vector embeddings).
    pub embedding_provider: Arc<EmbeddingProvider>,
    /// Memory searcher (hybrid BM25 + cosine similarity).
    pub memory_searcher: Arc<MemorySearcher>,
    /// Memory injector (auto-injects memories into system prompt).
    pub memory_injector: Arc<MemoryInjector>,
    /// Per-session timestamp of the last learning-extraction pass — gates
    /// the background self-evolution hook (10-minute minimum interval).
    pub learning_last_run: Arc<tokio::sync::Mutex<HashMap<String, std::time::Instant>>>,
    /// Per-session timestamp of the last procedure-capture pass — gates the
    /// background workflow-learning hook (10-minute minimum interval).
    pub procedure_last_run: Arc<tokio::sync::Mutex<HashMap<String, std::time::Instant>>>,
    /// Workspaces that already attempted project-cognition generation — a
    /// missing `.deepdepcat/project-cognition.md` triggers ONE background
    /// generation; this set stops it re-triggering on every turn.
    pub cognition_tried: Arc<tokio::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>>,
    // ── Observability / codebase / workspace ─────────────────────────────
    /// Session usage trackers (session_id → tracker).
    pub usage_trackers: Arc<Mutex<HashMap<String, SessionUsageTracker>>>,
    /// Codebase symbol index.
    pub symbol_index: Arc<RwLock<SymbolIndex>>,
    /// Cached dependency graph (built on first query or index_codebase call).
    pub dependency_graph: Arc<RwLock<Option<DependencyGraph>>>,
    /// Background task manager.
    pub task_manager: Arc<TaskManager>,
    /// Image transcription cache: per-session, per-image — a picture is
    /// described by the vision model once per session; later calls return
    /// the cached description instead of re-invoking the vision API. This is
    /// the structural dedup that prompt-level guidance cannot guarantee.
    pub visual_describe_cache: Arc<Mutex<VisualDescribeCache>>,
    /// Application data directory.
    pub app_data_dir: PathBuf,
    /// Agent status (lock-free atomic).
    agent_status: Arc<AtomicU8>,
    /// Debug tracing toggle (lock-free atomic).
    debug_mode: Arc<AtomicBool>,
    /// Workspace path (optional).
    pub workspace: Arc<RwLock<Option<PathBuf>>>,
    // ── Run lifecycle (cancel / pause / pending interactions) ────────────
    /// Active session ID → cancellation token.
    pub cancellation_tokens: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    /// Active session ID → pause watch sender (true = paused). Unlike the
    /// cancellation token (one-way latch), the watch flips freely between
    /// paused/resumed, letting a running agent loop suspend at its checkpoints.
    pub paused_sessions: Arc<Mutex<HashMap<String, tokio::sync::watch::Sender<bool>>>>,
    /// Pending permission requests: request_id → sender + tool metadata
    /// (tool name and args are needed to record "always allow" grants).
    pub pending_permissions:
        Arc<Mutex<HashMap<String, crate::permissions::grant_store::PendingPermission>>>,
    /// Pending MCP elicitation requests: elicitation_id → oneshot sender.
    pub pending_elicitations: Arc<
        Mutex<HashMap<String, tokio::sync::oneshot::Sender<crate::mcp::types::ElicitationResult>>>,
    >,
    /// Pending user input requests: request_id → oneshot sender.
    pub pending_user_inputs: Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>>,
    // ── Services (MCP / SSE / LLM / coordinators / LSP / stores) ─────────
    /// MCP server manager.
    pub mcp_manager: Arc<McpManager>,
    /// Real SSE transport hub — raw `chat-stream` events fanned out to
    /// EventSource subscribers.
    pub sse_hub: crate::sse::SseHub,
    /// Bound port of the loopback SSE server (set once at startup).
    pub sse_port: Arc<tokio::sync::Mutex<Option<u16>>>,
    /// Root LLM client — the providers store is shared by every clone (the
    /// multi-agent coordinator, session agents, compaction), so a config
    /// change can be hot-applied via [`Self::refresh_llm_providers`].
    pub llm_client: crate::llm::client::LlmClient,
    /// Multi-agent coordinator (subagent spawning).
    pub coordinator: Arc<MultiAgentCoordinator>,
    /// File state trackers per session (session_id → tracker).
    pub file_state_trackers:
        Arc<Mutex<HashMap<String, crate::workspace::checkpoint::FileStateTracker>>>,
    /// Per-session fingerprints of file contents the agent last read or
    /// wrote (session_id → (workspace, path) → FNV-1a hash) — the
    /// stale-edit guard compares disk content against these before every
    /// file write so the agent can never overwrite changes it has not seen.
    /// The workspace component keeps sub-agents and parallel workers in
    /// different workspaces isolated within the same session.
    pub file_seen_hashes: Arc<Mutex<FileSeenHashMap>>,
    /// Skill activation engine — manages conditional skill activation.
    pub skill_engine: Arc<crate::skills::activation::SkillActivationEngine>,
    /// MCP connection pool — tracks connection health and reconnects.
    pub mcp_connection_pool: Arc<crate::mcp::connection_pool::McpConnectionPool>,
    /// Shared LLM circuit breaker — per-provider failure tracking.
    pub circuit_breaker: Arc<CircuitBreaker>,
    /// Feature flag manager — remote-controlled feature toggles.
    pub feature_flags: Arc<crate::core::feature_flag::FeatureFlagManager>,
    /// Backend server connection settings (env-overridable).
    pub server_config: Arc<std::sync::RwLock<crate::core::managed::ServerConfig>>,
    /// LSP manager — per-workspace language server clients.
    pub lsp_manager: Arc<crate::tools::builtin::lsp::LspManager>,
    /// Session goal store — per-session declared goals (update_goal tool).
    pub goal_store: Arc<crate::tools::builtin::update_goal::GoalStore>,
    /// Session todo store — per-session task lists (todo_write tool),
    /// persisted so the frontend task-progress panel survives restarts.
    pub todo_store: Arc<crate::tools::builtin::todo_write::TodoStore>,
    /// Per-session prompt queues — prompts sent while the agent is busy are
    /// queued and replayed in order after the running turn completes.
    pub prompt_queues:
        Arc<tokio::sync::Mutex<HashMap<String, crate::agent::prompt_queue::PromptQueue>>>,
    /// Background task registry (bash background:true) — shared with the
    /// bash/kill_task/wait_tasks tools so the frontend task panel can
    /// query the same task list.
    pub background_tasks: Arc<crate::tools::background::BackgroundTaskRegistry>,
    /// Interrupted workflow progress (cancel → resume) — the workflow tool
    /// saves progress here so a cancelled fan-out/loop can be continued.
    pub workflow_store: Arc<crate::agent::workflow::WorkflowStore>,
    /// In-flight main-agent turns (persistence visibility) — lets the
    /// frontend list "background sessions" that keep running after the user
    /// switches away, and emit completion events.
    pub running_turns: Arc<crate::agent::running::RunningTurnRegistry>,
    /// Scheduler store + runner — scheduled tasks are actually executed.
    pub scheduler_store: crate::tools::builtin::scheduler::SchedulerStore,
    // ── Automation / scheduled tasks ─────────────────────────────────────
    /// Persistent scheduled-agent-task store (定时任务).
    pub automation_store: crate::automation::AutomationStore,
    /// task_id → running guard shared by the automation runner and the
    /// "run now" / cancel commands (std mutex so RAII release is panic-safe).
    pub automation_running: Arc<std::sync::Mutex<HashSet<String>>>,
    /// Session ids currently running unattended (scheduled runs). While a
    /// session is listed here, permission `Ask` becomes a denial and
    /// `ask_user` is unavailable — the loop can never stall on a prompt.
    pub unattended_sessions: Arc<tokio::sync::Mutex<HashSet<String>>>,
    // ── Permission / plan / workspace state ──────────────────────────────
    /// Per-session Auto-Review denial accounting (3-consecutive / 10-of-50
    /// circuit breaker). Removed with the session on cleanup.
    pub auto_review_trackers: Arc<
        tokio::sync::Mutex<
            HashMap<String, crate::permissions::auto_review::AutoReviewTracker>,
        >,
    >,
    /// Worktree isolation manager — shared by subagents and scheduled runs.
    pub worktree_isolation: Arc<crate::workspace::isolation::WorktreeIsolationManager>,
    /// Shared monitor event buffer — tools, subagents, and background
    /// tasks push events here; the monitor tool drains them.
    pub monitor_events: crate::tools::builtin::monitor::EventBuffer,
    /// Anonymous diagnostics — silent tool-error aggregation + upload.
    /// Toggle-controlled (default on, opt-out in Settings → Privacy).
    pub diagnostics: Arc<DiagnosticsReporter>,
    /// Durable "always allow" permission grants (persisted to disk).
    pub grant_store: Arc<crate::permissions::grant_store::PermissionGrantStore>,
    /// Plugin policy layer — JSON policy restricting plugin installation.
    pub plugin_policy: Arc<crate::permissions::plugin_policy::PluginPolicyStore>,
    /// Pending plan-approval requests: request_id → parked approval
    /// (exit_plan_mode tool waits on the oneshot).
    pub pending_plan_approvals:
        Arc<tokio::sync::Mutex<HashMap<String, crate::permissions::plan::PendingPlanApproval>>>,
    /// Permission mode to restore after a plan is approved:
    /// session_id → mode the agent was in before `enter_plan_mode`.
    pub plan_previous_modes: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
    /// Parsed steps of the approved plan per session — consumed by the
    /// loop's checklist gate (P2-5 structured planner).
    pub active_plan_steps:
        Arc<tokio::sync::Mutex<HashMap<String, Vec<crate::permissions::plan::PlanStep>>>>,
    /// RAW plan Markdown per session (the exact `plan` string the model
    /// passed to exit_plan_mode) — kept so the run-end hook can archive it
    /// to `.deepdepcat/plans/<session>.md` after execution completes.
    pub approved_plan_text: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
    /// Latest user rejection feedback per session — captured so the run-end
    /// plan reflection can tell the next plan-writer what the user pushed
    /// back on before approving.
    pub last_reject_feedback: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
    /// Session-scoped grants ("allow for this session"): session_id → list
    /// of (tool_name, pattern). Pure memory — never persisted.
    pub session_grants: Arc<tokio::sync::Mutex<crate::permissions::grant_store::SessionGrantMap>>,
    /// Pending interactions the user must answer, per session: permission
    /// requests, plan approvals, and ask_user questions. Surfaced to the
    /// frontend as a "waiting for you" status.
    pub pending_interactions:
        Arc<tokio::sync::Mutex<HashMap<String, Vec<crate::permissions::plan::PendingInteraction>>>>,
    /// Per-session permission-mode overrides: session_id → (mode, set at).
    /// A fresh override shadows the global mode for that session only; it
    /// expires (falls back to the global mode) after [`SESSION_MODE_TTL`]
    /// so a stranded read-only lock can never hold a session forever. The
    /// global mode stays the default for sessions without an override.
    pub session_modes: Arc<
        tokio::sync::Mutex<
            HashMap<String, (crate::permissions::mode::PermissionMode, std::time::Instant)>,
        >,
    >,
    /// Browser takeover manager — a real Chromium browser the agent can
    /// drive (Depwork), with human-in-the-loop handoff for captchas/logins.
    pub browser: Arc<crate::browser::BrowserManager>,
    /// Live-frame streaming for the embedded dev browser — relays CDP
    /// screencast frames to the frontend (`browser-screencast-frame`).
    pub screencast: Arc<crate::browser::screencast::ScreencastController>,
    /// Per-session output paths the Depwork agent created (session outputs).
    /// Editing one's own outputs is auto-allowed; touching pre-existing
    /// user files still prompts. Recorded by the generate/convert tools,
    /// consulted by the edit tools' permission checks.
    pub session_outputs: Arc<std::sync::Mutex<HashMap<String, Vec<std::path::PathBuf>>>>,
    /// Paths changed by EXTERNAL processes (file watcher), most recent
    /// first. The agent loop's verification gate consumes matching entries
    /// to invalidate auto-LSP evidence recorded before the external edit —
    /// a "clean" verdict from before a change is not evidence for the
    /// current file state.
    pub external_changes: Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>,
    /// Async-hook wake queue (session_id → messages). Async `async_rewake`
    /// hooks push exit-2 messages here; the agent loop drains them at each
    /// iteration so the model can react mid-turn. Cleared when a new run
    /// starts so stale wakes never leak into a later user message.
    pub async_hook_wakes: Arc<tokio::sync::Mutex<HashMap<String, Vec<String>>>>,
}

impl AppState {
    /// Drain queued async-hook wake messages for a session.
    pub async fn drain_async_hook_wakes(&self, session_id: &str) -> Vec<String> {
        self.async_hook_wakes
            .lock()
            .await
            .remove(session_id)
            .unwrap_or_default()
    }

    /// Drop stale async-hook wakes for a session (called at run start).
    pub async fn clear_async_hook_wakes(&self, session_id: &str) {
        self.async_hook_wakes.lock().await.remove(session_id);
    }

    /// Get the current agent status.
    pub fn agent_status(&self) -> AgentStatus {
        AgentStatus::from_u8(self.agent_status.load(Ordering::Relaxed))
    }

    /// Set the agent status.
    pub fn set_agent_status(&self, status: AgentStatus) {
        self.agent_status.store(status.as_u8(), Ordering::Relaxed);
    }

    /// Check if debug tracing is enabled.
    pub fn debug_mode(&self) -> bool {
        self.debug_mode.load(Ordering::Relaxed)
    }

    /// Enable or disable debug tracing.
    pub fn set_debug_mode(&self, on: bool) {
        self.debug_mode.store(on, Ordering::Relaxed);
    }

    /// Get a read lock on the config.
    pub fn config(&self) -> AppResult<std::sync::RwLockReadGuard<'_, AppConfig>> {
        self.config.read().map_err(Into::into)
    }

    /// Get a write lock on the config.
    pub fn config_write(&self) -> AppResult<std::sync::RwLockWriteGuard<'_, AppConfig>> {
        self.config.write().map_err(Into::into)
    }

    /// Hot-apply the current config's provider list into the shared LLM
    /// client store. Call after any config change that touched providers
    /// (API keys, base URLs, protocols) so running agents and subagents pick
    /// up the new credentials without a restart.
    pub fn refresh_llm_providers(&self) {
        let providers = self
            .config()
            .map(|guard| guard.llm.providers.clone())
            .unwrap_or_default();
        self.llm_client.refresh_providers(providers);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::Semaphore;

    #[tokio::test]
    async fn session_semaphore_allows_three_then_waits() {
        // The session concurrency cap is 3: 3 permits available, a 4th
        // acquisition waits until one is released.
        let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_SESSIONS));
        let p1 = sem.clone().acquire_owned().await.unwrap();
        let p2 = sem.clone().acquire_owned().await.unwrap();
        let p3 = sem.clone().acquire_owned().await.unwrap();

        // 4th acquisition must NOT complete immediately.
        let sem4 = sem.clone();
        let waiter = tokio::spawn(async move {
            let _p4 = sem4.acquire_owned().await.unwrap();
        });
        // Give the waiter a moment — it must still be pending.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "4th session must wait for a permit");

        // Release one → the waiter proceeds.
        drop(p3);
        tokio::time::timeout(std::time::Duration::from_secs(2), waiter)
            .await
            .expect("waiter must proceed after release")
            .unwrap();
        drop(p1);
        drop(p2);
    }

    #[tokio::test]
    async fn session_semaphore_wait_is_cancellable() {
        // A waiter stuck behind a full semaphore can be cancelled cleanly —
        // this is the send_message select! branch for user interrupt.
        let sem = Arc::new(Semaphore::new(1));
        let _p1 = sem.clone().acquire_owned().await.unwrap();
        let cancel = tokio_util::sync::CancellationToken::new();

        let sem2 = sem.clone();
        let cancel2 = cancel.clone();
        let task = tokio::spawn(async move {
            let acquire = sem2.acquire_owned();
            tokio::select! {
                _permit = acquire => "permit",
                _ = cancel2.cancelled() => "cancelled",
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .expect("cancellable wait must resolve")
            .unwrap();
        assert_eq!(result, "cancelled", "wait must abort on cancellation");
    }
}
