//! DeepDepCat - Unified AI Desktop Workbench
//!
//! Full-featured Rust backend with:
//! - Multi-provider LLM client (DeepSeek, OpenAI, Anthropic, Grok, Ollama)
//! - Agent loop with streaming, compaction, and multi-agent support
//! - 14 built-in tools (file I/O, bash, grep, glob, web, memory, agent)
//! - Multi-layered permission system (rules, filesystem, bash security)
//! - Hook system (20+ events, 4 execution types)
//! - MCP protocol integration (stdio/SSE/HTTP)
//! - Memory system with FTS5 full-text search
//! - Skills system (bundled + file-based)
//! - SQLite persistence with migrations
//!
//! ## Frontend-Backend Interaction Pattern
//!
//! ### 1. Frontend -> Backend (Command invocation)
//! Frontend calls Rust functions via `invoke()`:
//! ```typescript
//! import { invoke } from "@tauri-apps/api/core";
//! const info = await invoke<SystemInfo>("get_system_info");
//! ```
//!
//! ### 2. Backend -> Frontend (Event emission)
//! Backend pushes updates to frontend via events:
//! ```rust,ignore
//! app.emit("chat-stream", &stream_event)?;
//! ```
//! Frontend listens with:
//! ```typescript
//! import { listen } from "@tauri-apps/api/event";
//! const unlisten = await listen("chat-stream", (e) => { ... });
//! ```

// ── Module declarations ───────────────────────────────────────────────────────
mod a2a;
mod acp;
mod agent;
mod automation;
mod bootstrap;
mod browser;
mod codebase;
mod commands;
mod core;
mod hooks;
#[cfg(test)]
mod layering_guard;
mod llm;
mod mcp;
mod memory;
mod observability;
mod permissions;
mod sse;
// The Windows Job Object path (core::proc::JobObject via BashTool) is live;
// the Unix wrapper generators were unwired dead code and removed.
mod sandbox;
mod skills;
mod storage;
mod task;
mod toolkit;
mod tools;
mod workspace;

use commands::{
    agent::*, auth_cmd::*, automation_cmd::*, browser_cmd::*, chat::*, cloud_cmd::*,
    compaction_cmd::*, config_cmd::*, connector::*, crash_cmd::*, feedback_cmd::*, hook_cmd::*,
    mcp_cmd::*, memory_cmd::*, model_cmd::*, observability_cmd::*, pdf_cmd::*, permission_cmd::*,
    permission_governance_cmd::*, plan_cmd::*, preview::*, rewind::*, session::*, sync_cmd::*,
    system::*, task_cmd::*, tools::*, update::*,
};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Install global panic hook for crash report generation
    crate::core::crash::ensure_client_id();
    crate::core::crash::install_panic_hook();
    // Windows native crash filter — catches access violations and other
    // native exceptions that never reach the Rust panic hook.
    #[cfg(windows)]
    crate::core::crash::install_native_crash_filter();

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,deepdepcat=debug".into()),
        )
        .init();

    tauri::Builder::default()
        // ── Plugins ──────────────────────────────────────────────
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        // ── Single instance ──────────────────────────────────────
        // Only one app process at a time: a second launch (desktop icon,
        // update auto-launch, double-click) focuses the existing window
        // instead of spawning a duplicate.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = app.get_webview_window("main").map(|w| {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            });
        }))
        // ── State (initialized async at startup) ─────────────────
        .setup(|app| {
            // Get the workspace from the current directory
            let workspace = std::env::current_dir().ok();

            // Initialize app state
            let state =
                tauri::async_runtime::block_on(bootstrap::AppState::initialize(workspace))?;

            // Initialize async subsystems (skills, MCP health checker).
            tauri::async_runtime::block_on(state.initialize_async())?;

            // AppState must be managed BEFORE any subsystem that calls
            // `app.state::<AppState>()` (MCP connect → elicitation handler,
            // tool registration) — otherwise those calls panic.
            app.manage(state);

            // Remove a staged silent update once we run at/beyond its version
            // (the exit-install succeeded, or a newer release replaced it).
            crate::commands::update::cleanup_stale_pending(app.handle());

            // All background subsystems (MCP sync, update polling, idle
            // reaper, SSE transport, ACP/A2A servers, automation runner)
            // assemble here — one readable call, see bootstrap/startup.rs.
            bootstrap::startup::start_subsystems(app)?;

            Ok(())
        })
        // ── Command handlers ─────────────────────────────────────
        .invoke_handler(tauri::generate_handler![
            // System commands
            get_system_info,
            get_agent_status,
            set_agent_status,
            cancel_operation,
            pause_operation,
            resume_operation,
            list_running_sessions,
            set_debug_mode,
            get_debug_mode,
            get_sse_port,
            mcp_app_log,
            refresh_skills,
            set_workspace,
            open_workspace_file,
            // Circuit breaker commands
            get_circuit_breaker_states,
            reset_circuit_breaker,
            // MCP elicitation commands
            respond_elicitation,
            // Crash report commands
            list_crash_reports,
            read_crash_report,
            delete_crash_report,
            // Feature flag commands
            get_feature_flags,
            set_feature_flag,
            // Chat commands
            send_chat_message,
            get_turn_snapshot,
            // Session commands
            create_session,
            list_sessions,
            get_session,
            delete_session,
            get_session_messages,
            update_session_title,
            set_session_pinned,
            update_session_model,
            delete_message,
            get_session_goal,
            set_session_goal,
            get_session_todos,
            // Cloud sync commands
            sync_now,
            // Agent commands
            get_permission_mode,
            set_permission_mode,
            respond_to_user_input,
            list_agent_definitions,
            // Permission commands
            respond_permission,
            get_auto_review_enabled,
            set_auto_review_enabled,
            override_auto_review_denial,
            clear_permission_grants,
            clear_session_grants,
            list_permission_grants,
            remove_permission_grant,
            get_permission_rules,
            set_permission_rules,
            list_plugin_policy,
            set_plugin_policy,
            // Plan-approval commands (pause & plan loop)
            respond_plan_approval,
            list_skills,
            save_skill,
            delete_skill,
            // Hook commands
            list_hooks,
            save_hook,
            delete_hook,
            trust_hook,
            untrust_hook,
            list_hook_events,
            preview_hook,
            list_project_hooks,
            get_project_hooks_enabled,
            set_project_hooks_enabled,
            // Auth commands (direct password login + session persistence)
            login_with_password,
            verify_token,
            revoke_token,
            auth_store_token,
            auth_load_token,
            auth_delete_token,
            get_default_server_url,
            update_user_profile,
            upload_avatar,
            // Registration (send-code → verify-email)
            register_send_code,
            register_verify_email,
            // Cloud content commands (public website endpoints)
            submit_feedback,
            fetch_changelog,
            fetch_site_config,
            // Rewind commands
            rewind_to,
            get_rewind_points,
            // Compaction commands
            force_compact,
            // Tool commands
            list_active_workers,
            list_background_tasks,
            read_task_output,
            kill_background_task,
            // PDF commands (depwork preview panel)
            extract_pdf_text,
            // Config commands
            get_config,
            update_config,
            // Model discovery commands (native HTTP — provider APIs don't send CORS)
            fetch_provider_models,
            // Memory commands
            store_memory,
            search_memories,
            list_memories,
            delete_memory,
            get_memory_count,
            trigger_dream,
            get_memory_files,
            get_procedure_files,
            // MCP commands
            list_mcp_servers,
            add_mcp_server,
            remove_mcp_server,
            connect_mcp_server,
            disconnect_mcp_server,
            get_mcp_tools,
            list_connected_mcp_servers,
            list_mcp_prompts,
            call_mcp_prompt,
            mcp_app_proxy,
            save_mcp_credential,
            delete_mcp_credential,
            list_mcp_credentials,
            // Connector commands (UI placeholder stubs)
            list_connectors,
            connect_connector,
            disconnect_connector,
            list_plugins,
            install_plugin,
            uninstall_plugin,
            toggle_plugin,
            // Task (depwork) commands
            list_tasks,
            create_task,
            // Automation (scheduled agent tasks) commands
            list_scheduled_tasks,
            create_scheduled_task,
            update_scheduled_task,
            delete_scheduled_task,
            list_scheduled_runs,
            delete_scheduled_run,
            run_scheduled_task_now,
            cancel_scheduled_run,
            cleanup_scheduled_worktree,
            // Browser takeover commands
            browser_takeover_start,
            browser_takeover_stop,
            browser_takeover_status,
            browser_takeover_navigate,
            browser_takeover_screenshot,
            browser_takeover_logs,
            browser_takeover_resume,
            browser_takeover_default_timeout,
            browser_screencast_start,
            browser_screencast_stop,
            browser_takeover_input,
            // Browser tab strip (frontend)
            browser_tabs,
            browser_tab_new,
            browser_tab_switch,
            browser_tab_close,
            // Preview pane (read a local HTML target for the sandboxed frame)
            read_preview_target,
            open_preview_external,
            // Codebase commands
            // Observability commands
            get_session_usage,
            get_global_usage,
            get_session_events,
            // Sandbox commands
            // Update commands
            check_for_update,
            download_and_install_update,
            download_silent_update,
            has_pending_silent_update,
            clear_pending_silent_update,
            relaunch_app,
            // Crash report commands (anonymous upload pipeline)
            get_pending_crash,
            dismiss_pending_crash,
            export_session_conversation,
            submit_crash_report,
            // Diagnostics toggle (anonymous error telemetry, Settings → Privacy)
            get_diagnostics_enabled,
            set_diagnostics_enabled,
            // Client-side error telemetry — POSTs natively (reqwest, no CORS)
            submit_client_error,
            // NOTE: `send_stream_message`/`cancel_stream` (sampling) and
            // `tool_list`/`tool_execute`/`tool_has` (tool_cmd) are NOT
            // registered — their Tauri states (SamplingState/ToolState) are
            // never managed, so invoking them would panic at runtime. The
            // legacy `sampling`/`tool` modules were removed in Round 16
            // (dead code, self-contained references only).
        ])
        .build(tauri::generate_context!())
        .expect("error while building DeepDepCat application")
        .run(|app_handle, event| {
            // ── Silent update install on exit ─────────────────────
            // A staged silent update (downloaded in the background for a
            // backend-only release) installs itself when the user closes the
            // app: spawn a detached helper that waits for this process to
            // fully exit, then runs `msiexec /quiet`. Next launch is the new
            // version — zero user interaction.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                install_staged_update_on_exit(app_handle);
            }
        });
}

/// If a silent update is staged, spawn a detached installer that waits for
/// this process to exit and then installs it quietly, relaunching the app.
fn install_staged_update_on_exit(app: &tauri::AppHandle) {
    let Some(staged) = crate::commands::update::pending_staged_version(app) else {
        return;
    };
    tracing::info!(version = %staged.0, "Installing staged update on exit");

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // NSIS per-user installer: `-S` = silent, `/currentuser` = per-user
        // (no UAC). Wait ~5s for the app process to release its files, then
        // install and relaunch the app so the update is seamless.
        let script = format!(
            "@echo off\r\nping -n 6 127.0.0.1 >nul\r\n\"{}\" -S /currentuser\r\nstart \"\" \"%LOCALAPPDATA%\\DeepDepCat\\DeepDepCat.exe\"\r\n",
            cmd_escape_path(&staged.1.to_string_lossy())
        );
        let _ = std::process::Command::new("cmd")
            .args(["/c", &script])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = staged;
    }
}

/// Escape a path for embedding in a `cmd` script: `& | < > ^` are command
/// metacharacters even inside double quotes, so each gets a caret escape
/// (`^&`). Without this, an installer path containing `&` (or a caret)
/// would be split into separate commands or mangled.
fn cmd_escape_path(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '&' | '|' | '<' | '>' | '^' => {
                escaped.push('^');
                escaped.push(c);
            }
            _ => escaped.push(c),
        }
    }
    escaped
}
