//! Built-in tools — the default toolset available to the agent.
//!
//! Tools registered:
//! - read_file, write_file, edit_file, list_dir — filesystem operations
//! - apply_patch, search_replace — advanced editing
//! - bash — shell command execution
//! - grep, glob — code search
//! - web_fetch — web page content retrieval
//! - web_search — web search
//! - task_manage, todo_write — task management
//! - kill_task — background task termination
//! - ask_user — user interaction
//! - enter_plan_mode, exit_plan_mode — plan mode
//! - memory_search, memory_store — memory system
//! - procedure_save, procedure_search — procedural memory (learned workflows)
//! - agent_tool — subagent spawning
//! - scheduler_create, scheduler_list, scheduler_delete — scheduled tasks
//! - lsp — language server protocol integration
//! - monitor — event monitoring
//! - use_tool — meta-tool dispatch
//! - file_operation_lock — file locking

pub mod agent_tool;
pub mod apply_patch;
pub mod ask_user;
pub mod bash;
pub mod code_search;
pub mod coordinator_tools;
pub mod depwork;
pub mod dev_browser_open;
pub mod diff_preview;
pub mod edit_file;
pub mod file_operation_lock;
pub mod glob;
pub mod grep;
pub mod kill_task;
pub mod list_dir;
pub mod lsp;
pub mod memory_ops;
pub mod monitor;
pub mod procedure_ops;
pub mod plan_mode;
pub mod read_file;
pub mod read_file_document;
pub mod read_file_image;
pub mod read_file_pdf;
pub mod scheduler;
pub mod search_replace;
pub mod task_manage;
pub mod todo_write;
pub mod update_goal;
pub mod use_tool;
pub mod user_profile;
pub mod visual_describe;
pub mod wait_tasks;
pub mod web_fetch;
pub mod web_search;
pub mod workflow_tool;
pub mod write_file;

use crate::core::config::AppConfig;
use crate::memory::search::MemorySearcher;
use crate::memory::store::MemoryStore;
use crate::tools::registry::ToolRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Resolve a path relative to the workspace.
///
/// - Absolute paths are used as-is.
/// - Relative paths (including `.` and `""`) are joined to the workspace root.
/// - If no workspace is set, relative paths resolve against the process cwd.
///
/// All filesystem tools must use this function instead of `PathBuf::from(path)`
/// to ensure the agent operates inside the user-selected workspace, not the
/// process's own directory.
pub fn resolve_path(workspace: Option<&Path>, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    if path.trim().is_empty() || path.trim() == "." {
        return workspace
            .map(|w| w.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
    }
    workspace
        .map(|w| w.join(path))
        .unwrap_or_else(|| PathBuf::from(path))
}

/// Register all built-in tools.
#[allow(clippy::too_many_arguments)]
pub fn register_all(
    registry: &ToolRegistry,
    config: &AppConfig,
    memory_store: Arc<MemoryStore>,
    memory_searcher: Arc<MemorySearcher>,
    embedding_provider: Arc<crate::memory::embedding::EmbeddingProvider>,
    lsp_manager: Arc<lsp::LspManager>,
    goal_store: Arc<update_goal::GoalStore>,
    todo_store: Arc<todo_write::TodoStore>,
    background_tasks: Arc<crate::tools::background::BackgroundTaskRegistry>,
    scheduler_store: scheduler::SchedulerStore,
    monitor_events: monitor::EventBuffer,
    task_manager: Arc<crate::task::TaskManager>,
    sandbox: Option<Arc<std::sync::RwLock<crate::sandbox::executor::SandboxExecutor>>>,
) {
    // Filesystem tools (always available)
    registry.register(Arc::new(read_file::ReadFileTool::new(
        config.tools.max_output_chars,
    )));
    registry.register(Arc::new(write_file::WriteFileTool::new()));
    registry.register(Arc::new(edit_file::EditFileTool::new()));
    registry.register(Arc::new(list_dir::ListDirTool::new()));

    // Search tools (always available)
    registry.register(Arc::new(grep::GrepTool::new(config.tools.max_output_chars)));
    registry.register(Arc::new(glob::GlobTool::new()));

    // Codebase intelligence tools — symbol & dependency lookup over the
    // indexed codebase (lazily indexed on first use).
    registry.register(Arc::new(code_search::SearchSymbolsTool::new()));
    registry.register(Arc::new(code_search::FileDependenciesTool::new()));

    // Bash tool (configurable)
    if config.tools.bash_enabled {
        registry.register(Arc::new(bash::BashTool::with_background_registry(
            config.tools.bash_timeout_secs,
            config.tools.max_output_chars,
            background_tasks.clone(),
            sandbox.clone(),
        )));
        registry.register(Arc::new(kill_task::KillTaskTool::new(
            background_tasks.clone(),
        )));
        registry.register(Arc::new(wait_tasks::WaitTasksTool::new(background_tasks)));
    }

    // Web tools (configurable)
    if config.tools.web_fetch_enabled {
        registry.register(Arc::new(web_fetch::WebFetchTool::new(
            config.tools.max_output_chars,
        )));
    }
    if config.tools.web_search_enabled {
        registry.register(Arc::new(web_search::WebSearchTool::new()));
    }

    // Advanced editing tools
    registry.register(Arc::new(apply_patch::ApplyPatchTool::new()));
    registry.register(Arc::new(search_replace::SearchReplaceTool::new()));

    // Dev-browser preview (always available — opens the in-app window).
    registry.register(Arc::new(dev_browser_open::DevBrowserOpenTool::new()));

    // Task management tools (todo_write is the structured, frontend-synced
    // implementation; task_manage tracks shared tasks in the TaskManager —
    // the same store the frontend task list reads from).
    registry.register(Arc::new(todo_write::TodoWriteTool::new(todo_store)));
    registry.register(Arc::new(task_manage::TaskManageTool::new(task_manager)));

    // Session goal tool
    registry.register(Arc::new(update_goal::UpdateGoalTool::new(
        goal_store.clone(),
    )));

    // Plan mode tools
    registry.register(Arc::new(plan_mode::EnterPlanModeTool::new()));
    registry.register(Arc::new(plan_mode::ExitPlanModeTool::new()));

    // Interaction tools
    registry.register(Arc::new(ask_user::AskUserTool::new()));

    // Memory tools
    if config.memory.enabled {
        registry.register(Arc::new(memory_ops::MemorySearchTool::new(
            memory_searcher.clone(),
        )));
        registry.register(Arc::new(memory_ops::MemoryStoreTool::new(
            memory_store.clone(),
            embedding_provider.clone(),
        )));
        registry.register(Arc::new(memory_ops::MemoryLearnTool::new()));
        registry.register(Arc::new(memory_ops::MemoryWriteTool::new()));
        registry.register(Arc::new(procedure_ops::ProcedureSaveTool::new()));
        registry.register(Arc::new(procedure_ops::ProcedureSearchTool::new()));
    }

    // User profile (managed-section file in ~/.deepdepcat/USER.md)
    registry.register(Arc::new(user_profile::UserProfileTool::new()));

    // Visual describe — on-demand vision-model lookup for text-only main
    // models (DeepSeek). Attached images are transcribed automatically on
    // send; this tool lets the model re-ask the vision model with a targeted
    // question for fine details. Shares the transcription cache.
    registry.register(Arc::new(visual_describe::VisualDescribeTool::new()));

    // Agent tool (multi-agent)
    if config.agent.multi_agent_enabled {
        registry.register(Arc::new(agent_tool::AgentTool::new()));
        registry.register(Arc::new(workflow_tool::WorkflowTool::new()));
        registry.register(Arc::new(coordinator_tools::SendMessageTool::new()));
        registry.register(Arc::new(coordinator_tools::TaskStopTool::new()));
    }

    // Scheduler tools
    registry.register(Arc::new(scheduler::SchedulerCreateTool::new(
        scheduler_store.clone(),
    )));
    registry.register(Arc::new(scheduler::SchedulerListTool::new(
        scheduler_store.clone(),
    )));
    registry.register(Arc::new(scheduler::SchedulerDeleteTool::new(
        scheduler_store,
    )));

    // LSP tool — real stdio client (definition, references, diagnostics,
    // format). The server is started lazily on first use, so registering
    // it has no startup cost. Toggleable via `tools.lsp_enabled`.
    if config.tools.lsp_enabled {
        registry.register(Arc::new(lsp::LspTool::new((*lsp_manager).clone())));
    }

    // Monitor tool — drains the SHARED event buffer (tools, subagents,
    // background tasks push into it).
    registry.register(Arc::new(monitor::MonitorTool::new(monitor_events)));

    // Meta-tool dispatch (snapshot clone — register last to include all tools)
    registry.register(Arc::new(use_tool::UseTool::new(registry.clone())));

    // File operation lock
    registry.register(Arc::new(file_operation_lock::FileOperationLockTool::new(
        file_operation_lock::FileLockManager::new(),
    )));

    // Depwork-only tools (document automation — only reachable in Depwork
    // mode via `ToolRegistry::for_mode`).
    depwork::register_depwork_tools(registry);

    tracing::info!("Registered {} built-in tools", registry.len());
}
