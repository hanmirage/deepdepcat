//! AppState lifecycle methods — skills reload, async post-init, and the
//! background tasks started after construction.

use super::AppState;
use crate::core::error::AppResult;

impl AppState {
    /// Reload skills from all sources (own + ecosystem). Called at startup,
    /// when the workspace changes, and after a compat toggle so project-level
    /// skills take effect.
    pub async fn reload_skills(&self) {
        let workspace = self.workspace.read().map(|w| w.clone()).unwrap_or_default();
        // Ecosystem compat gate comes from the `[skills]` config section.
        let claude_enabled = match self.config() {
            Ok(c) => c.skills.claude_enabled,
            Err(_) => true,
        };
        let loader = crate::skills::loader::SkillLoader::new(&self.app_data_dir)
            .with_workspace(workspace)
            .with_compat(claude_enabled);
        if let Ok(skills) = loader.load_all() {
            self.skill_engine.load_skills(skills).await;
        }
    }

    /// Initialize subsystems that require async setup after construction.
    ///
    /// Loads skills into the activation engine, starts the MCP health checker,
    /// and fetches managed config + feature flags from the server (best-effort).
    pub async fn initialize_async(&self) -> AppResult<()> {
        // Load skills into the activation engine (own + ecosystem sources).
        self.reload_skills().await;

        // Start the file watcher (workspace "index invalidator"): external
        // file changes mark the symbol index + dependency graph stale and
        // invalidate pre-change auto-LSP evidence. Runs on a blocking thread
        // so disk walks never stall the async runtime.
        {
            let workspace = self.workspace.read().map(|w| w.clone()).unwrap_or_default();
            if let Some(w) = workspace {
                if w.is_dir() {
                    crate::memory::watcher::FileWatcher::new(w).run(self.clone());
                }
            }
        }

        // Start the MCP connection health checker.
        self.mcp_connection_pool.start_health_checker();

        // Start the scheduler runner — scheduled tasks now actually fire.
        // The permission checker gates every unattended execution (no user
        // to approve each run), and commands are timeout-bounded.
        crate::tools::builtin::scheduler::SchedulerRunner::new(
            self.scheduler_store.clone(),
            5,
            (*self.permissions).clone(),
        )
        .spawn();

        // Start the anonymous diagnostics flush loop (tool-error counts).
        // The server URL is read live from server_config so the loop follows
        // the user's configured backend.
        {
            let server_config = self.server_config.clone();
            crate::observability::diagnostics::spawn_flush_loop(
                self.diagnostics.clone(),
                std::time::Duration::from_secs(60),
                move || {
                    server_config
                        .read()
                        .map(|g| g.base_url.clone())
                        .unwrap_or_default()
                },
            );
        }

        // Start the device heartbeat loop (anonymous online presence for
        // the admin "devices" view). Same live server_url resolution as the
        // diagnostics loop; disabled when the privacy toggle is off.
        {
            let app_data_dir = self.app_data_dir.clone();
            let server_config = self.server_config.clone();
            crate::observability::heartbeat::spawn_heartbeat_loop(
                app_data_dir,
                crate::observability::heartbeat::HEARTBEAT_INTERVAL,
                move || {
                    server_config
                        .read()
                        .map(|g| g.base_url.clone())
                        .unwrap_or_default()
                },
            );
        }

        // Fetch feature flags (offline-safe). Runs in the background so the
        // network request never blocks app startup (the sync block_on in
        // setup would otherwise hold webview load → white screen on first
        // launch). Flags arrive when the server responds; the app is fully
        // functional without them.
        {
            let this = self.clone();
            tauri::async_runtime::spawn(async move {
                this.refresh_feature_flags().await;
            });
        }

        Ok(())
    }

    /// Fetch feature flags from the server.
    ///
    /// Best-effort: any failure keeps the previous values and is logged,
    /// never propagated — the app must work fully offline.
    pub async fn refresh_feature_flags(&self) {
        self.feature_flags.fetch_flags().await;
    }
}
