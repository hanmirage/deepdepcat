//! Agent builder — fluent construction API for assembling an `AgentLoop`
//! with all dependencies wired up.
//!
//! Extracts the inline construction logic that was previously duplicated
//! in the `send_chat_message` command. The builder:
//!
//! 1. Discovers the repository type (Rust, Node, Python, etc.) for context
//! 2. Builds the LLM client with retry and prompt caching
//! 3. Creates the tool dispatcher with permission gating
//! 4. Creates the compactor with the configured summarizer model
//! 5. Builds the context builder with memory injection and context chips
//! 6. Initializes system reminder state for the session
//! 7. Instruments startup phases with timing guards
//! 8. Assembles the `AgentLoop` with all components

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::agent::agent_loop::{AgentLoop, AgentLoopConfig, AgentLoopMode};
use crate::agent::compaction::Compactor;
use crate::agent::context::ContextBuilder;
use crate::agent::discovery::discover;
use crate::core::config::{AgentSection, LlmSection, PermissionsSection, ToolsSection};
use crate::core::types::{ContextChip, ProjectType};
use crate::hooks::HookExecutor;
use crate::llm::circuit_breaker::CircuitBreaker;
use crate::llm::client::LlmClient;
use crate::llm::retry::RetryConfig;
use crate::memory::injection::MemoryInjector;
use crate::observability::usage::SessionUsageTracker;
use crate::permissions::PermissionChecker;
use crate::tools::dispatch::ToolDispatcher;
use crate::tools::registry::ToolRegistry;
use crate::workspace::checkpoint::FileStateTracker;

/// The fully assembled agent — ready to run.
pub struct BuiltAgent {
    /// The configured agent loop.
    pub loop_: AgentLoop,
    /// Custom-agent body to overlay on the mode's system prompt (main
    /// session persona). `None` = standard persona.
    pub system_prompt: Option<String>,
}

/// Fluent builder for constructing an `AgentLoop` with all dependencies.
///
/// Created from an `AppState` snapshot, then customized with optional
/// parameters (mode, context chips), and finally built via `build()`.
pub struct AgentBuilder {
    // Config sections (cloned from AppState at creation time)
    llm_config: LlmSection,
    agent_config: AgentSection,
    tools_config: ToolsSection,
    permissions_config: PermissionsSection,

    // Workspace
    workspace: Option<PathBuf>,

    // Shared dependencies (Arc-cloned from AppState)
    tool_registry: Arc<ToolRegistry>,
    permissions: Arc<PermissionChecker>,
    hooks: Arc<RwLock<crate::hooks::HookRegistry>>,
    memory_injector: Arc<MemoryInjector>,
    pending_permissions: Arc<
        tokio::sync::Mutex<HashMap<String, crate::permissions::grant_store::PendingPermission>>,
    >,
    grant_store: Arc<crate::permissions::grant_store::PermissionGrantStore>,
    circuit_breaker: Arc<CircuitBreaker>,

    // Optional parameters
    context_chips: Vec<ContextChip>,
    mode: AgentLoopMode,
    work_mode: crate::toolkit::WorkMode,
    usage_tracker: Option<SessionUsageTracker>,
    debug_mode: bool,

    // File state tracker for checkpoint/rewind
    file_state_tracker: Option<FileStateTracker>,
    // Reasoning effort for DeepSeek thinking mode
    reasoning_effort: Option<String>,
    /// Session provider hint — threaded into tool contexts so meta-tools
    /// (`agent` decompose) route internal LLM calls to the same provider.
    provider: Option<String>,
    // Skill activation engine (for skill guidance reminders)
    skill_engine: Option<Arc<crate::skills::activation::SkillActivationEngine>>,
    // LSP manager (for diagnostic reminders)
    lsp_manager: Option<Arc<crate::tools::builtin::lsp::LspManager>>,
    // Shared interjection registry (per-run transient guidance)
    interjections:
        Option<Arc<tokio::sync::Mutex<crate::agent::interjection::InterjectionRegistry>>>,
    /// Optional custom agent persona for the MAIN session (Code/Depwork).
    /// When set, the body overlays the mode prompt and the definition's
    /// permissions + tool allowlist are enforced (M9 semantics).
    custom_agent: Option<crate::agent::definition::ResolvedCustomAgent>,
}

impl AgentBuilder {
    /// Create a builder from config sections and shared dependencies.
    ///
    /// In production, these are cloned from `AppState`. In tests, they
    /// can be constructed directly.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        llm_config: LlmSection,
        agent_config: AgentSection,
        tools_config: ToolsSection,
        permissions_config: PermissionsSection,
        workspace: Option<PathBuf>,
        tool_registry: Arc<ToolRegistry>,
        permissions: Arc<PermissionChecker>,
        hooks: Arc<RwLock<crate::hooks::HookRegistry>>,
        memory_injector: Arc<MemoryInjector>,
        pending_permissions: Arc<
            tokio::sync::Mutex<HashMap<String, crate::permissions::grant_store::PendingPermission>>,
        >,
        circuit_breaker: Arc<CircuitBreaker>,
    ) -> Self {
        Self {
            llm_config,
            agent_config,
            tools_config,
            permissions_config,
            workspace,
            tool_registry,
            permissions,
            hooks,
            memory_injector,
            pending_permissions,
            circuit_breaker,
            grant_store: Arc::new(crate::permissions::grant_store::PermissionGrantStore::default()),
            context_chips: vec![],
            mode: AgentLoopMode::Standard,
            work_mode: crate::toolkit::WorkMode::Code,
            custom_agent: None,
            usage_tracker: None,
            debug_mode: false,
            file_state_tracker: None,
            reasoning_effort: None,
            provider: None,
            skill_engine: None,
            lsp_manager: None,
            interjections: None,
        }
    }

    /// Build from an `AppState` snapshot: reads the four config sections,
    /// clones the shared dependencies, and applies the overrides common to
    /// every entry point (skill engine, LSP manager, grant store). This
    /// collapses the repeated 11-arg `new()` + clone chain in chat /
    /// automation / A2A / ACP. Entry-specific overrides (mode, work mode,
    /// usage tracker, provider, debug mode, chips, custom agent) are applied
    /// by the caller afterward.
    pub fn from_state(
        state: &crate::bootstrap::AppState,
        workspace: Option<PathBuf>,
    ) -> Result<Self, String> {
        let config = state.config().map_err(|e| e.to_string())?;
        let sections = (
            config.llm.clone(),
            config.agent.clone(),
            config.tools.clone(),
            config.permissions.clone(),
        );
        drop(config);
        Ok(Self::new(
            sections.0,
            sections.1,
            sections.2,
            sections.3,
            workspace,
            state.tools.clone(),
            state.permissions.clone(),
            state.hooks.clone(),
            state.memory_injector.clone(),
            state.pending_permissions.clone(),
            state.circuit_breaker.clone(),
        )
        .with_skill_engine(state.skill_engine.clone())
        .with_lsp_manager(state.lsp_manager.clone())
        .with_grant_store(state.grant_store.clone()))
    }

    /// Attach the durable "always allow" grant store.
    pub fn with_grant_store(
        mut self,
        store: Arc<crate::permissions::grant_store::PermissionGrantStore>,
    ) -> Self {
        self.grant_store = store;
        self
    }

    /// Set the agent loop mode (Standard, PlanExecute, Reflexion, etc.).
    pub fn with_mode(mut self, mode: AgentLoopMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set the product work mode (Code / Depwork).
    ///
    /// Filters the tool registry to the mode's available tool scope and
    /// selects the matching default system prompt.
    pub fn with_work_mode(mut self, work_mode: crate::toolkit::WorkMode) -> Self {
        self.work_mode = work_mode;
        self
    }

    /// Set a custom agent persona for the MAIN session. Its body overlays
    /// the mode's system prompt; its permissions and tool allowlist are
    /// enforced by the dispatcher (M9 semantics).
    pub fn with_custom_agent(
        mut self,
        agent: Option<crate::agent::definition::ResolvedCustomAgent>,
    ) -> Self {
        self.custom_agent = agent;
        self
    }

    /// Set context chips (files/folders/URLs attached by the user).
    pub fn with_context_chips(mut self, chips: Vec<ContextChip>) -> Self {
        self.context_chips = chips;
        self
    }

    /// Set the usage tracker for recording token usage.
    pub fn with_usage_tracker(mut self, tracker: SessionUsageTracker) -> Self {
        self.usage_tracker = Some(tracker);
        self
    }

    /// Enable or disable debug tracing.
    pub fn with_debug_mode(mut self, enabled: bool) -> Self {
        self.debug_mode = enabled;
        self
    }

    /// Set the file state tracker for checkpoint/rewind functionality.
    pub fn with_file_state_tracker(mut self, tracker: FileStateTracker) -> Self {
        self.file_state_tracker = Some(tracker);
        self
    }

    /// Set the reasoning effort for DeepSeek thinking mode.
    /// "high" → reasoning_effort = "high"
    /// "max" → reasoning_effort = "max"
    /// "auto" or None → None (resolved per-intent inside the agent loop)
    pub fn with_reasoning_effort(mut self, effort: Option<String>) -> Self {
        self.reasoning_effort = effort;
        self
    }

    /// Set the session provider hint (deepseek / provider-<ts> / …).
    pub fn with_provider(mut self, provider: Option<String>) -> Self {
        self.provider = provider;
        self
    }

    /// Set the skill activation engine for skill guidance reminders.
    pub fn with_skill_engine(
        mut self,
        engine: Arc<crate::skills::activation::SkillActivationEngine>,
    ) -> Self {
        self.skill_engine = Some(engine);
        self
    }

    /// Set the LSP manager for automatic diagnostic reminders.
    pub fn with_lsp_manager(
        mut self,
        manager: Arc<crate::tools::builtin::lsp::LspManager>,
    ) -> Self {
        self.lsp_manager = Some(manager);
        self
    }

    /// Set the shared interjection registry (per-run transient guidance).
    pub fn with_interjections(
        mut self,
        registry: Arc<tokio::sync::Mutex<crate::agent::interjection::InterjectionRegistry>>,
    ) -> Self {
        self.interjections = Some(registry);
        self
    }

    /// Build the agent — assembles all dependencies and returns a `BuiltAgent`.
    pub fn build(self) -> BuiltAgent {
        // Phase 1: Discover repository type
        let project_type = self
            .workspace
            .as_deref()
            .map(|ws| discover(ws).project_type)
            .unwrap_or(ProjectType::Unknown);

        // Phase 2: Build LLM client
        let retry_config = RetryConfig::from_llm_config(&self.llm_config);
        let llm_client = LlmClient::new(
            self.llm_config.providers.clone(),
            retry_config,
            self.llm_config.prompt_caching_enabled,
            self.circuit_breaker.clone(),
        );

        // Phase 3: Build tool dispatcher
        let tool_semaphore = Arc::new(tokio::sync::Semaphore::new(
            self.agent_config.max_concurrent_tools.max(1),
        ));
        // Filter the shared registry by work mode: Depwork drops Code-only
        // tools (bash, code editing, LSP, code intelligence) at build time.
        let mode_registry = Arc::new(self.tool_registry.for_mode(self.work_mode));
        // A custom agent's `allowed_tools` narrows the registry further —
        // the main persona only sees the tools it declares (when any).
        let mode_registry = match &self.custom_agent {
            Some(ca) if !ca.allowed_tools.is_empty() => {
                let names: Vec<&str> = ca.allowed_tools.iter().map(|s| s.as_str()).collect();
                Arc::new(mode_registry.allowlist_clone(&names))
            }
            _ => mode_registry,
        };
        let agent_rules = self.custom_agent.as_ref().map(|ca| {
            std::sync::Arc::new(crate::permissions::rules::AgentPermissionRules::from_lists(
                &ca.permissions.allow,
                &ca.permissions.deny,
                &ca.permissions.ask,
            ))
        });
        // The main custom agent's DENIES must also propagate down to its
        // subagents (M9: a parent's hard veto can never be dropped by a
        // child). Allows/asks stay main-loop-only — children define their
        // own; only restrictions inherit.
        let inherited_denies = self
            .custom_agent
            .as_ref()
            .map(|ca| ca.permissions.deny.clone())
            .unwrap_or_default();
        if let Some(ca) = &self.custom_agent {
            tracing::info!(agent = %ca.name, "Main session using custom agent persona");
        }
        let tool_dispatcher = ToolDispatcher::new(
            mode_registry,
            self.permissions,
            self.tools_config.max_output_chars,
            self.workspace.clone(),
            self.pending_permissions,
            self.file_state_tracker,
        )
        .with_provider(self.provider)
        .with_usage_tracker(self.usage_tracker.clone())
        .with_grant_store(self.grant_store)
        .with_concurrency(tool_semaphore)
        .with_work_mode(self.work_mode)
        .with_agent_rules(agent_rules)
        .with_agent_deny_rules(inherited_denies.clone())
        .with_behavior_version(crate::toolkit::ToolBehaviorVersion::parse(
            &self.tools_config.behavior_version,
        ))
        .with_reminder(std::sync::Arc::new(
            crate::tools::reminders::EmptyOutputReminder,
        ))
        .with_reminder(std::sync::Arc::new(
            crate::tools::reminders::CompletionSignalReminder,
        ));

        // Phase 3b: Register the skill guidance reminder when a skill engine
        // is available — active skills then surface in tool outputs.
        // The work mode filters skills declared for the other mode.
        let tool_dispatcher = if let Some(engine) = self.skill_engine.clone() {
            tool_dispatcher.with_reminder(std::sync::Arc::new(
                crate::tools::reminders::SkillGuidanceReminder::new(engine, self.work_mode),
            ))
        } else {
            tool_dispatcher
        };

        // Phase 3c: Register the LSP diagnostics reminder. It only queries
        // already-running clients, so it never cold-starts a server.
        let tool_dispatcher = if let Some(manager) = self.lsp_manager {
            tool_dispatcher.with_reminder(std::sync::Arc::new(
                crate::tools::reminders::DiagnosticsReminder::new(manager),
            ))
        } else {
            tool_dispatcher
        };

        // Phase 4: Build compactor with two-pass prefire enabled.
        let compactor = Compactor::new(
            llm_client.clone(),
            self.agent_config.compaction_model.clone(),
        )
        .with_two_pass(70)
        // Background prefire summaries record their billed tokens into the
        // session usage tracker (the stats surface; `total_usage` budget
        // seeding remains out of reach for detached tasks).
        .with_usage_tracker(self.usage_tracker.clone());

        // Phase 5: Build context builder
        let mut context_builder = ContextBuilder::new(self.workspace);
        context_builder.set_work_mode(self.work_mode);
        if !self.context_chips.is_empty() {
            context_builder.set_context_chips(self.context_chips);
        }
        context_builder.set_memory_injector(self.memory_injector);
        context_builder.set_project_type(project_type.clone());
        if let Some(engine) = self.skill_engine.clone() {
            context_builder.set_skill_engine(engine);
        }
        // Phase 6: Build hook executor
        let hook_executor = HookExecutor::new(self.hooks);

        // Phase 7: Build loop config
        // deepseek-native: reasoning effort from the user's input-bar
        // selection (low/high/max). Unknown values fall back to None — the
        // request builder then defaults to "high".
        let reasoning_effort = self
            .reasoning_effort
            .as_deref()
            .and_then(resolve_explicit_effort);
        let loop_config = AgentLoopConfig {
            max_turns: self.agent_config.max_turns,
            auto_compact_threshold_percent: self.agent_config.auto_compact_threshold_percent,
            temperature: None,
            mode: self.mode,
            max_consecutive_denials: self.permissions_config.max_consecutive_denials,
            reasoning_effort,
            session_token_limit: self.agent_config.session_token_limit,
            session_cost_limit: self.agent_config.session_cost_limit,
            run_timeout_secs: self.agent_config.run_timeout_secs,
            turn_output_token_limit: self.agent_config.turn_output_token_limit.filter(|&l| l > 0),
            agent_deny_rules: inherited_denies.clone(),
        };

        // Phase 8: Assemble agent loop
        let mut agent_loop = AgentLoop::new(
            llm_client,
            tool_dispatcher,
            compactor,
            context_builder,
            loop_config,
            hook_executor,
        );

        if let Some(tracker) = self.usage_tracker {
            agent_loop = agent_loop.with_usage_tracker(tracker);
        }

        if let Some(registry) = self.interjections {
            agent_loop = agent_loop.with_interjections(registry);
        }

        BuiltAgent {
            loop_: agent_loop,
            system_prompt: self
                .custom_agent
                .as_ref()
                .map(|ca| ca.body.clone())
                .filter(|body| !body.trim().is_empty()),
        }
    }
}

/// Resolve the user's explicit reasoning-tier string to a DeepSeek effort.
/// Unknown values return `None` — the request builder then defaults to
/// "high" (deep thought).
fn resolve_explicit_effort(mode: &str) -> Option<String> {
    match mode {
        "low" => Some("low".to_string()),
        "high" => Some("high".to_string()),
        "max" => Some("max".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{AgentSection, LlmSection, PermissionsSection, ToolsSection};
    use crate::hooks::HookRegistry;
    use crate::memory::embedding::EmbeddingProvider;
    use crate::memory::injection::MemoryInjector;
    use crate::memory::search::MemorySearcher;
    use crate::memory::store::MemoryStore;
    use crate::permissions::PermissionChecker;
    use crate::tools::registry::ToolRegistry;
    use tokio::sync::Mutex;

    fn build_test_agent_builder() -> AgentBuilder {
        let llm_config = LlmSection::default();
        let agent_config = AgentSection::default();
        let tools_config = ToolsSection::default();
        let permissions_config = PermissionsSection::default();

        let tool_registry = Arc::new(ToolRegistry::new());
        let permissions = Arc::new(PermissionChecker::new(permissions_config.clone()));
        let hooks = Arc::new(RwLock::new(HookRegistry::new()));

        // Minimal memory stack using a temp database
        let tmp = tempfile::tempdir().expect("failed to create temp dir");
        let db = Arc::new(
            crate::storage::database::Database::open(&tmp.path().join("test.db"), false)
                .expect("failed to open test DB"),
        );
        let _ = db.run_migrations();
        let memory_store = Arc::new(MemoryStore::new(db));
        let embedding_provider = Arc::new(EmbeddingProvider::local());
        let memory_searcher = MemorySearcher::new(memory_store, embedding_provider, 0.0, 10);
        let memory_injector = Arc::new(MemoryInjector::new(memory_searcher, false));

        let pending_permissions = Arc::new(Mutex::new(HashMap::new()));
        let circuit_breaker = Arc::new(crate::llm::circuit_breaker::CircuitBreaker::new(
            crate::llm::circuit_breaker::CircuitBreakerConfig::default(),
        ));

        AgentBuilder::new(
            llm_config,
            agent_config,
            tools_config,
            permissions_config,
            None,
            tool_registry,
            permissions,
            hooks,
            memory_injector,
            pending_permissions,
            circuit_breaker,
        )
    }

    #[test]
    fn build_produces_agent_with_defaults() {
        build_test_agent_builder().build();
    }

    #[test]
    fn explicit_reasoning_tiers_resolve_directly() {
        assert_eq!(resolve_explicit_effort("low"), Some("low".to_string()));
        assert_eq!(resolve_explicit_effort("high"), Some("high".to_string()));
        assert_eq!(resolve_explicit_effort("max"), Some("max".to_string()));
    }

    #[test]
    fn auto_reasoning_tier_stays_unresolved() {
        // Auto is resolved per-intent inside the agent loop — the builder
        // must leave it None so the loop gets to choose the tier.
        assert_eq!(resolve_explicit_effort("auto"), None);
        assert_eq!(resolve_explicit_effort(""), None);
        assert_eq!(resolve_explicit_effort("bogus"), None);
    }

    #[test]
    fn build_with_mode_coordinator() {
        // The built agent should have coordinator mode set in the loop config
        // (verified indirectly — the build completed without panic)
        build_test_agent_builder()
            .with_mode(AgentLoopMode::Coordinator)
            .build();
    }

    #[test]
    fn build_with_context_chips() {
        let chips = vec![ContextChip::File {
            name: "test.rs".into(),
            path: "C:\\tmp\\test.rs".into(),
            data_url: None,
        }];
        build_test_agent_builder().with_context_chips(chips).build();
    }

}
