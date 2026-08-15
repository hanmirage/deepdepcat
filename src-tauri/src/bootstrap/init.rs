//! AppState construction — `initialize` wires every subsystem together.
//!
//! Split out of `core/state.rs` so the state module stays under the file
//! size budget; the `pub` API of `AppState` is unchanged.

use super::AppState;
use crate::core::config::AppConfig;
use crate::core::error::AppResult;
use crate::mcp::manager::McpManager;
use crate::permissions::PermissionChecker;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8};
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;

impl AppState {
    /// Initialize the entire application state.
    ///
    /// This is called once at startup from `lib.rs`.
    pub async fn initialize(workspace: Option<PathBuf>) -> AppResult<Self> {
        let mut workspace = workspace;
        let app_data_dir = crate::core::config::get_app_data_dir();
        std::fs::create_dir_all(&app_data_dir)?;

        // Load configuration
        let config = AppConfig::load(&app_data_dir, workspace.as_deref())?;

        // Initialize database
        let db_path = app_data_dir.join(&config.storage.database_path);
        let db = crate::storage::database::Database::open(&db_path, config.storage.wal_mode)?;
        db.run_migrations()?;
        // Bounded retention for the replay-exact agent event log — 30 days.
        // Best-effort: a prune failure must never block startup.
        if let Err(e) = crate::storage::database::prune_events(&db, 30) {
            tracing::warn!(error = %e, "Failed to prune agent event log");
        }

        // ── Restore persisted runtime settings (settings KV table) ──────
        // 1. Diagnostics (anonymous error telemetry) toggle — survives
        //    restarts instead of resetting to the config default.
        // 2. Last workspace — the persisted project wins over the raw
        //    process CWD: when the packaged app is launched from the shell
        //    or Explorer, `current_dir` is the install directory (never a
        //    real project), which used to shadow `last_workspace` and skip
        //    the restore entirely. The frontend re-sends the true workspace
        //    right after startup anyway, but the subsystems built below
        //    (file watcher, context builder, sandbox root) must not start
        //    armed at a meaningless directory.
        {
            if let Some(value) = db.get_setting("diagnostics_enabled")? {
                if let Ok(enabled) = value.parse::<bool>() {
                    crate::observability::diagnostics::set_enabled(enabled);
                }
            }
            if let Some(saved) = db.get_setting("last_workspace")? {
                let candidate = PathBuf::from(&saved);
                if !saved.is_empty() && candidate.is_dir() {
                    workspace = Some(candidate);
                }
            }
        }

        // Initialize subsystems
        let db_arc = Arc::new(db);
        // Best-effort background backup at startup (WAL-safe online copy,
        // keeps 14 daily snapshots in app_data_dir/backups). Never blocks
        // startup and never breaks the app on failure.
        {
            let db = db_arc.clone();
            let backups = app_data_dir.join("backups");
            tauri::async_runtime::spawn_blocking(move || {
                if let Err(e) = db.backup_to(&backups, 14) {
                    tracing::warn!(error = %e, "Database backup failed");
                }
            });
        }
        let sessions = Arc::new(Mutex::new(crate::agent::session::SessionManager::new(
            db_arc.clone(),
        )));
        let tools = crate::tools::registry::ToolRegistry::new();
        let permissions = Arc::new(PermissionChecker::new(config.permissions.clone()));
        // Restore the user's persisted permission mode (chosen at runtime,
        // survives restarts). Falls back to the config default when absent.
        if let Some(mode) = crate::permissions::mode::load_persisted_mode(&app_data_dir) {
            permissions.set_mode(mode);
        }
        let hooks = Arc::new(RwLock::new(crate::hooks::HookRegistry::new()));
        let hook_trust =
            Arc::new(crate::hooks::trust::HookTrustStore::load(&app_data_dir));
        let memory = Arc::new(crate::memory::store::MemoryStore::new(db_arc.clone()));

        // ── Load hooks from disk into the RUNTIME registry ──────────────
        // User-level `hooks.toml` + project-level `.deepdepcat/hooks.toml`
        // are discovered here so the executor (which reads this registry)
        // actually fires configured hooks. Save/delete commands reload the
        // same registry at runtime (see hook_cmd.rs).
        {
            let mut guard = hooks.write().unwrap_or_else(|e| e.into_inner());
            match crate::hooks::discovery::discover_and_register(
                &mut guard,
                &app_data_dir,
                workspace.as_deref(),
                config.hooks.enable_project_hooks,
            ) {
                Ok(count) => {
                    tracing::info!(count, "Hooks loaded into runtime registry");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Hook discovery failed — continuing without hooks");
                }
            }
        }

        // Memory subsystem: embedding provider → searcher → injector
        let embedding_provider = Arc::new(crate::memory::embedding::EmbeddingProvider::local());
        let search_weights = crate::memory::search::SearchWeights {
            bm25: config.memory.search_weight_bm25,
            cosine: config.memory.search_weight_cosine,
            recency: config.memory.search_weight_recency,
        };
        let memory_searcher = Arc::new(
            crate::memory::search::MemorySearcher::new(
                memory.clone(),
                embedding_provider.clone(),
                config.memory.search_min_score,
                config.memory.search_max_results,
            )
            .with_weights(search_weights.clone())
            .with_recency_half_life(config.memory.search_recency_half_life_hours as f64)
            .with_recency_temperature(config.memory.search_recency_temperature),
        );
        let memory_injector = Arc::new(crate::memory::injection::MemoryInjector::new(
            crate::memory::search::MemorySearcher::new(
                memory.clone(),
                embedding_provider.clone(),
                config.memory.search_min_score,
                config.memory.search_max_results,
            )
            .with_weights(search_weights)
            .with_recency_half_life(config.memory.search_recency_half_life_hours as f64)
            .with_recency_temperature(config.memory.search_recency_temperature),
            config.memory.auto_injection_enabled,
        ));

        // Sandbox executor — profile selectable via config.toml [tools]
        // sandbox_profile ("workspace" default / "strict" / "read_only" /
        // "off"). Strict/ReadOnly activate the restricted-token security
        // filter in the bash tool's Job Object (admin-SID stripping).
        let sandbox = Arc::new(RwLock::new(crate::sandbox::executor::SandboxExecutor::new(
            crate::core::config::parse_sandbox_profile(&config.tools.sandbox_profile),
            workspace.clone(),
            Some(app_data_dir.clone()),
        )));

        // Usage trackers (created per session on demand)
        let usage_trackers = Arc::new(Mutex::new(HashMap::new()));

        // Codebase symbol index (empty until indexed)
        let symbol_index = Arc::new(RwLock::new(crate::codebase::symbols::SymbolIndex::new()));

        // Dependency graph (built lazily or on index_codebase)
        let dependency_graph = Arc::new(RwLock::new(None));

        // Task manager — persisted through the tasks table so the sidebar
        // task list survives app restarts (was a memory-only HashMap: every
        // restart silently wiped the panel).
        let task_manager = Arc::new(crate::task::TaskManager::new().with_db(db_arc.clone()));
        let automation_store = crate::automation::AutomationStore::new(db_arc.clone());
        let automation_running = Arc::new(std::sync::Mutex::new(
            std::collections::HashSet::new(),
        ));
        let unattended_sessions =
            Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new()));
        let auto_review_trackers = Arc::new(tokio::sync::Mutex::new(HashMap::new()));

        // Register built-in tools (before wrapping in Arc)
        let lsp_manager = Arc::new(crate::tools::builtin::lsp::LspManager::new());
        // The goal store persists to the database — a session goal (task
        // intent) survives app restarts and re-hydrates on the first get
        // (cross-restart task continuation).
        let goal_store =
            Arc::new(crate::tools::builtin::update_goal::GoalStore::new().with_db(db_arc.clone()));
        // Todo lists persist to the database too — the frontend task-progress
        // panel re-hydrates from disk when a session opens (restart-safe).
        let todo_store =
            Arc::new(crate::tools::builtin::todo_write::TodoStore::new().with_db(db_arc.clone()));
        let background_tasks = Arc::new(crate::tools::background::BackgroundTaskRegistry::new());
        let workflow_store = Arc::new(crate::agent::workflow::WorkflowStore::new());
        let scheduler_store = crate::tools::builtin::scheduler::SchedulerStore::new();
        let monitor_events = crate::tools::builtin::monitor::EventBuffer::new(Default::default());
        // Anonymous diagnostics — aggregates tool errors, flushes periodically.
        let diagnostics = crate::observability::diagnostics::DiagnosticsReporter::new();
        crate::tools::builtin::register_all(
            &tools,
            &config,
            memory.clone(),
            memory_searcher.clone(),
            embedding_provider.clone(),
            lsp_manager.clone(),
            goal_store.clone(),
            todo_store.clone(),
            background_tasks.clone(),
            scheduler_store.clone(),
            monitor_events.clone(),
            task_manager.clone(),
            Some(sandbox.clone()),
        );
        let tools = Arc::new(tools);

        // Pre-create shared state maps so they can be passed to the coordinator
        let pending_permissions = Arc::new(Mutex::new(HashMap::<
            String,
            crate::permissions::grant_store::PendingPermission,
        >::new()));

        let pending_elicitations = Arc::new(Mutex::new(HashMap::<
            String,
            tokio::sync::oneshot::Sender<crate::mcp::types::ElicitationResult>,
        >::new()));

        // Build shared dependencies for the multi-agent coordinator
        let llm_config = config.llm.clone();
        let agent_config = config.agent.clone();
        let tools_config = config.tools.clone();

        // Server config (URL from env or default) + feature flags fetched
        // from the server.
        let mut server_config = crate::core::managed::ServerConfig::default();
        crate::core::managed::apply_env_overrides(&mut server_config);
        let server_config = Arc::new(std::sync::RwLock::new(server_config));
        let feature_flags = Arc::new(crate::core::feature_flag::FeatureFlagManager::new(
            crate::core::feature_flag::FeatureFlagConfig {
                server_url: format!("{}/api/v1/config/flags", {
                    let srv = server_config.read().unwrap_or_else(|e| e.into_inner());
                    srv.base_url.clone()
                }),
                cache_ttl_secs: 3600,
            },
        ));

        let retry_config = crate::llm::retry::RetryConfig::from_llm_config(&llm_config);
        let circuit_breaker = Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
            crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
        ));
        let llm_client = crate::llm::client::LlmClient::new(
            llm_config.providers.clone(),
            retry_config,
            llm_config.prompt_caching_enabled,
            circuit_breaker.clone(),
        )
        .with_vcr(crate::llm::vcr::LlmVcr::from_env(&app_data_dir));
        // The AppState keeps the root client so provider changes (API keys)
        // can be hot-applied to every clone — see refresh_llm_providers.
        let llm_client_root = llm_client.clone();
        let skill_engine = Arc::new(crate::skills::activation::SkillActivationEngine::new());
        let mut context_builder = crate::agent::context::ContextBuilder::new(workspace.clone());
        context_builder.set_memory_injector(memory_injector.clone());
        context_builder.set_skill_engine(skill_engine.clone());
        context_builder.set_project_index(dependency_graph.clone(), symbol_index.clone());
        let hook_executor = crate::hooks::HookExecutor::new(hooks.clone());

        // Wire real LLM-backed prompt evaluator so prompt-type hooks actually
        // gate operations instead of failing open. Agent-type hooks get the
        // same LLM verdict protocol (ALLOW / DENY:<reason>).
        let hook_executor = {
            let llm = llm_client.clone();
            let prompt_eval = crate::hooks::eval::LlmPromptEvaluator::with_usage_trackers(
                llm.clone(),
                agent_config.compaction_model.clone(),
                Some(usage_trackers.clone()),
            );
            hook_executor
                .with_prompt_evaluator(std::sync::Arc::new(prompt_eval))
                .with_agent_evaluator(std::sync::Arc::new(
                    crate::hooks::eval::LlmAgentEvaluator::with_usage_trackers(
                        llm,
                        agent_config.compaction_model.clone(),
                        Some(usage_trackers.clone()),
                    ),
                ))
        };
        let hook_executor = hook_executor.with_trust_store(hook_trust.clone());
        // Async-hook wake buffer: `async_rewake` hooks push exit-2 messages
        // here; the agent loop drains them mid-turn so the model can react.
        let async_hook_wakes: Arc<
            tokio::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
        > = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
        let hook_executor = hook_executor.with_wake_sink(async_hook_wakes.clone());
        let app_hook_executor = hook_executor.clone();

        // Worktree isolation manager for subagents — worktrees are created
        // as siblings of the workspace root (git worktree convention).
        let worktree_isolation =
            Arc::new(crate::workspace::isolation::WorktreeIsolationManager::new(
                workspace
                    .clone()
                    .and_then(|w| w.parent().map(|p| p.to_path_buf()))
                    .unwrap_or_else(std::env::temp_dir),
                crate::workspace::isolation::IsolationMode::None,
            ));

        // Durable "always allow" permission grants (loaded from disk).
        let grant_store = Arc::new(crate::permissions::grant_store::PermissionGrantStore::load(
            &app_data_dir,
        ));
        let plugin_policy = Arc::new(crate::permissions::plugin_policy::PluginPolicyStore::load(
            &app_data_dir,
        ));

        let coordinator = Arc::new(
            crate::agent::multi_agent::MultiAgentCoordinator::new(
                agent_config.max_subagent_depth,
                agent_config.multi_agent_enabled,
                llm_client,
                tools.clone(),
                permissions.clone(),
                context_builder,
                hook_executor,
                tools_config.max_output_chars,
                agent_config.compaction_model.clone(),
                None,
                pending_permissions.clone(),
            )
            .with_worktree_isolation(worktree_isolation.clone())
            .with_worker_concurrency(Arc::new(tokio::sync::Semaphore::new(
                agent_config.max_concurrent_tools.max(1),
            )))
            .with_model_matrix(
                agent_config.plan_model.clone(),
                agent_config.explore_model.clone(),
                agent_config.verify_model.clone(),
            )
            .with_tool_defaults(
                crate::toolkit::ToolBehaviorVersion::parse(
                    &tools_config.behavior_version,
                ),
                agent_config.max_concurrent_tools,
            )
            .with_grant_store(grant_store.clone()),
        );

        // Shared MCP connection pool — the manager registers every
        // connection here; the pool's health checker drives reconnection.
        let mcp_connection_pool = Arc::new(crate::mcp::connection_pool::McpConnectionPool::new());

        Ok(Self {
            config: Arc::new(RwLock::new(config)),
            db: db_arc,
            sessions,
            tools,
            permissions,
            hooks,
            hook_executor: app_hook_executor,
            hook_trust,
            memory,
            embedding_provider,
            memory_searcher,
            memory_injector,
            learning_last_run: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            procedure_last_run: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            cognition_tried: Arc::new(tokio::sync::Mutex::new(std::collections::HashSet::new())),
            usage_trackers,
            symbol_index,
            dependency_graph,
            task_manager,
            app_data_dir: app_data_dir.clone(),
            agent_status: Arc::new(AtomicU8::new(0)),
            debug_mode: Arc::new(AtomicBool::new(false)),
            visual_describe_cache: Arc::new(Mutex::new(HashMap::new())),
            workspace: Arc::new(RwLock::new(workspace)),
            cancellation_tokens: Arc::new(Mutex::new(HashMap::new())),
            paused_sessions: Arc::new(Mutex::new(HashMap::new())),
            pending_permissions,
            pending_elicitations,
            pending_user_inputs: Arc::new(Mutex::new(HashMap::new())),
            mcp_manager: {
                let mgr = McpManager::new()
                    .with_app_data_dir(app_data_dir.clone())
                    .with_connection_pool(mcp_connection_pool.clone());
                mgr.start_liveness_check(60, 10);
                Arc::new(mgr)
            },
            sse_hub: crate::sse::SseHub::new(),
            sse_port: Arc::new(tokio::sync::Mutex::new(None)),
            llm_client: llm_client_root,
            coordinator,
            file_state_trackers: Arc::new(Mutex::new(HashMap::new())),
            file_seen_hashes: Arc::new(Mutex::new(HashMap::new())),
            skill_engine,
            mcp_connection_pool: mcp_connection_pool.clone(),
            circuit_breaker,
            feature_flags,
            server_config,
            lsp_manager,
            goal_store,
            todo_store,
            session_concurrency: Arc::new(tokio::sync::Semaphore::new(
                super::MAX_CONCURRENT_SESSIONS,
            )),
            prompt_queues: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            background_tasks,
            workflow_store,
            running_turns: Arc::new(crate::agent::running::RunningTurnRegistry::new()),
            scheduler_store,
            automation_store,
            automation_running,
            unattended_sessions,
            auto_review_trackers,
            worktree_isolation,
            monitor_events,
            diagnostics,
            grant_store,
            plugin_policy,
            pending_plan_approvals: Arc::new(Mutex::new(HashMap::new())),
            plan_previous_modes: Arc::new(Mutex::new(HashMap::new())),
            active_plan_steps: Arc::new(Mutex::new(HashMap::new())),
            approved_plan_text: Arc::new(Mutex::new(HashMap::new())),
            last_reject_feedback: Arc::new(Mutex::new(HashMap::new())),
            session_grants: Arc::new(Mutex::new(HashMap::new())),
            pending_interactions: Arc::new(Mutex::new(HashMap::new())),
            session_modes: Arc::new(Mutex::new(HashMap::new())),
            browser: Arc::new(crate::browser::BrowserManager::new(app_data_dir)),
            screencast: Arc::new(crate::browser::screencast::ScreencastController::new()),
            session_outputs: Arc::new(std::sync::Mutex::new(HashMap::new())),
            external_changes: Arc::new(std::sync::Mutex::new(Vec::new())),
            async_hook_wakes,
        })
    }
}
