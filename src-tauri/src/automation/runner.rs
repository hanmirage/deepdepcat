//! Automation runner — polls due scheduled tasks and executes them as
//! background agent sessions.
//!
//! Execution model: each run creates a REAL session, runs the standard
//! agent loop, persists the transcript, and records a run row. Runs are
//! unattended:
//! - permission `Ask` verdicts become denials (the dispatcher short-circuits
//!   via [`crate::bootstrap::AppState::is_unattended`]) — no 30s stall;
//! - `ask_user` is unavailable;
//! - a cancellation token is registered per run session so the user can
//!   stop a runaway scheduled run from the UI.
//!
//! Worktree runs keep their changes in the isolated worktree for review —
//! nothing is auto-merged into the user's working tree.

use super::store::AutomationStore;
use super::{summarize_conversation, RunStatus, ScheduledRun, ScheduledTask};
use crate::agent::agent_builder::AgentBuilder;
use crate::bootstrap::AppState;
use crate::workspace::checkpoint::FileStateTracker;
use chrono::Utc;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;

/// Polling interval for due-task detection.
const DEFAULT_POLL_SECS: u64 = 30;
/// Summary used for cancelled runs.
const CANCELLED_SUMMARY: &str = "已取消";

/// The scheduled-agent-task runner.
#[derive(Clone)]
pub struct AutomationRunner {
    store: AutomationStore,
    /// task_id → currently running. Std mutex so a RAII guard can release
    /// the slot even on panic paths.
    running: Arc<Mutex<HashSet<String>>>,
    state: AppState,
    poll_secs: u64,
}

/// RAII guard that frees the per-task running slot on drop.
struct RunningSlot {
    task_id: String,
    running: Arc<Mutex<HashSet<String>>>,
}

impl Drop for RunningSlot {
    fn drop(&mut self) {
        if let Ok(mut set) = self.running.lock() {
            set.remove(&self.task_id);
        }
    }
}

impl AutomationRunner {
    pub fn new(
        store: AutomationStore,
        state: AppState,
        running: Arc<Mutex<HashSet<String>>>,
    ) -> Self {
        Self {
            store,
            running,
            state,
            poll_secs: DEFAULT_POLL_SECS,
        }
    }

    /// Start the polling loop as a detached task.
    pub fn spawn(self, app: AppHandle) -> tauri::async_runtime::JoinHandle<()> {
        tauri::async_runtime::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                self.poll_secs.max(5),
            ));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                self.pump(&app).await;
            }
        })
    }

    /// Manually trigger a run (the Scheduled page's "run now").
    pub async fn run_task_now(&self, app: &AppHandle, task_id: &str) -> Result<String, String> {
        let task = self
            .store
            .get_task(task_id)
            .ok_or_else(|| "定时任务不存在".to_string())?;
        if self
            .running
            .lock()
            .map_err(|e| e.to_string())?
            .contains(task_id)
        {
            return Err("该任务正在运行中".to_string());
        }
        let run = self.new_run(&task);
        self.store.insert_run(&run)?;
        self.emit_run(app, &run);
        let this = self.clone();
        let app = app.clone();
        let run_id = run.id.clone();
        tauri::async_runtime::spawn(async move {
            this.execute(app, task, run_id).await;
        });
        Ok(run.id)
    }

    /// Cancel a run. A running run cancels its agent session; a pending run
    /// is marked cancelled and the execute guard skips it.
    pub async fn cancel_run(&self, run_id: &str) -> Result<(), String> {
        let run = self
            .store
            .get_run(run_id)
            .ok_or_else(|| "运行记录不存在".to_string())?;
        match run.status {
            RunStatus::Running => {
                if let Some(session_id) = run.session_id.as_deref() {
                    self.state.cancel_session(session_id).await;
                    Ok(())
                } else {
                    self.store
                        .update_run(run_id, RunStatus::Cancelled, None, CANCELLED_SUMMARY, "", "")?;
                    Ok(())
                }
            }
            RunStatus::Pending => {
                self.store
                    .update_run(run_id, RunStatus::Cancelled, None, CANCELLED_SUMMARY, "", "")?;
                Ok(())
            }
            _ => Err("该运行已结束".to_string()),
        }
    }

    /// Remove a scheduled run's leftover worktree. Refuses when the worktree
    /// has uncommitted changes — the user must review/merge them first.
    pub async fn cleanup_worktree(&self, run_id: &str) -> Result<String, String> {
        let run = self
            .store
            .get_run(run_id)
            .ok_or_else(|| "运行记录不存在".to_string())?;
        let Some(session_id) = run.session_id.as_deref() else {
            return Ok("该运行没有关联会话，无需清理".to_string());
        };
        match self
            .state
            .worktree_isolation
            .cleanup_worktree(session_id)
            .await
            .map_err(|e| e.to_string())?
        {
            crate::workspace::isolation::WorktreeCleanupOutcome::Removed => {
                Ok("worktree 已清理".to_string())
            }
            crate::workspace::isolation::WorktreeCleanupOutcome::NotRegistered => {
                Ok("没有已注册的 worktree".to_string())
            }
            crate::workspace::isolation::WorktreeCleanupOutcome::Dirty => {
                Err("worktree 有未提交的改动，请先审阅/合并后再清理".to_string())
            }
        }
    }

    /// Poll: fire every due, active, not-already-running task.
    async fn pump(&self, app: &AppHandle) {
        let now_ms = now_ms();
        for task in self.store.list_tasks() {
            if !task.active {
                continue;
            }
            let running = match self.running.lock() {
                Ok(set) => set.contains(&task.id),
                Err(_) => true,
            };
            if running {
                continue;
            }
            let Some(due) = task.schedule.next_due_ms(task.last_run_at_ms, now_ms) else {
                continue;
            };
            if due > now_ms {
                continue;
            }
            let run = self.new_run(&task);
            if let Err(e) = self.store.insert_run(&run) {
                tracing::warn!(task_id = %task.id, error = %e, "Failed to insert scheduled run");
                continue;
            }
            self.emit_run(app, &run);
            let this = self.clone();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                this.execute(app, task, run.id).await;
            });
        }
    }

    fn new_run(&self, task: &ScheduledTask) -> ScheduledRun {
        ScheduledRun {
            id: crate::core::ids::generate_id(),
            task_id: task.id.clone(),
            session_id: None,
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            summary: String::new(),
            error: String::new(),
            worktree_path: String::new(),
        }
    }

    /// Execute one run. Never panics out of the task — every failure lands
    /// in the run row's `error` field.
    async fn execute(&self, app: AppHandle, task: ScheduledTask, run_id: String) {
        // Reserve the per-task slot first (release via RAII on every exit).
        let slot = {
            let mut set = match self.running.lock() {
                Ok(s) => s,
                Err(_) => {
                    let _ = self.store.update_run(
                        &run_id,
                        RunStatus::Skipped,
                        None,
                        "",
                        "调度锁不可用",
                        "",
                    );
                    return;
                }
            };
            if set.insert(task.id.clone()) {
                Some(RunningSlot {
                    task_id: task.id.clone(),
                    running: self.running.clone(),
                })
            } else {
                None
            }
        };
        let Some(_slot) = slot else {
            let _ = self.store.update_run(
                &run_id,
                RunStatus::Skipped,
                None,
                "",
                "该任务已在运行",
                "",
            );
            return;
        };

        // A cancel that landed while the run was still pending (before its
        // session existed) marks the row cancelled — skip execution.
        let run = self.store.get_run(&run_id);
        if run
            .as_ref()
            .is_some_and(|r| r.status != RunStatus::Running)
        {
            return;
        }

        let result = self.run_agent(&app, &task, &run_id).await;
        let (session_id, summary, error) = match result {
            Ok((sid, summary)) => (Some(sid), summary, String::new()),
            Err(e) => (None, String::new(), e),
        };

        let status = if !error.is_empty() {
            if error == CANCELLED_SUMMARY {
                RunStatus::Cancelled
            } else {
                RunStatus::Failed
            }
        } else {
            RunStatus::Completed
        };

        // Notification hook — the Scheduled inbox surface (observe-only).
        if let Some(sid) = session_id.as_deref() {
            let ctx = crate::hooks::types::HookContext::new(
                crate::hooks::types::HookEvent::Notification,
                sid,
            )
            .with_data("kind", serde_json::json!("scheduled_run"))
            .with_data("status", serde_json::json!(status.as_str()));
            self.state.hook_executor.execute_observe(&ctx).await;
        }

        let _ = self.store.update_run(
            &run_id,
            status,
            session_id.as_deref(),
            &summary,
            &error,
            "",
        );
        // A fired run advances the schedule regardless of outcome — an
        // immediate retry storm on a failing task would be worse.
        self.store.mark_task_run(&task.id, now_ms());
        if let Some(updated_task) = self.store.get_task(&task.id) {
            let _ = app.emit("scheduled-task-updated", &updated_task);
        }
        if let Some(updated_run) = self.store.get_run(&run_id) {
            let _ = app.emit("scheduled-run-updated", &updated_run);
        }

        // ── Autonomous report ───────────────────────────────────────────
        // A completed/failed scheduled agent run surfaces a summary to the
        // frontend bell + toast — the user learns what the background agent
        // did (a persistent agent's report, or any scheduled run's result)
        // without opening the session. Cancelled/skipped runs stay silent.
        if matches!(status, RunStatus::Completed | RunStatus::Failed) {
            let report_summary = if summary.is_empty() { &error } else { &summary };
            let _ = app.emit(
                "agent-completed",
                &serde_json::json!({
                    "agent_id": task.id,
                    "name": task.name,
                    "session_id": session_id,
                    "status": status.as_str(),
                    "summary": report_summary,
                }),
            );
        }
    }

    /// The actual background session run. Returns `(session_id, summary)`.
    async fn run_agent(
        &self,
        app: &AppHandle,
        task: &ScheduledTask,
        run_id: &str,
    ) -> Result<(String, String), String> {
        let state = &self.state;

        // ── Resolve the working directory ──────────────────────────────
        let project = if !task.project_path.trim().is_empty() {
            Some(PathBuf::from(task.project_path.trim()))
        } else {
            state.workspace.read().ok().and_then(|w| w.clone())
        };
        let Some(project) = project else {
            return Err("未配置项目目录：请选择项目或填写 project_path".to_string());
        };
        if !project.is_dir() {
            return Err(format!("项目目录不存在: {}", project.display()));
        }

        // ── Config snapshot + session creation ─────────────────────────
        let (default_model, default_provider) = {
            let config = state.config().map_err(|e| e.to_string())?;
            (
                config.app.default_model.clone(),
                config.app.default_provider.clone(),
            )
        };
        let work_mode = crate::toolkit::WorkMode::parse(Some(&task.work_mode));
        let model = if task.model.trim().is_empty() {
            default_model
        } else {
            task.model.trim().to_string()
        };
        // Unattended posture: accept file edits, deny everything that would
        // prompt. Depwork's document tools self-approve NEW files and the
        // session's OWN drafts via their write gate; a pre-existing USER
        // file becomes an Ask → unattended deny instead of a silent
        // overwrite. A bare `full_access` here would bypass that gate and let
        // scheduled runs clobber user documents with zero prompts.
        let permission_mode = "accept_edits";

        // ── Persistent session reuse ────────────────────────────────────
        // A persistent agent reuses its own session across fires so its
        // context/goal accumulates (`take_chat_state` below loads the full
        // history — the agent "lives"). First fire creates the session and
        // writes the id back to the task row; later fires reuse it unless the
        // session row is gone (deleted/restart edge) — then we create fresh
        // and re-write.
        let persistent_reuse: Option<String> = if task.persistent {
            match task.persistent_session_id.as_deref() {
                Some(sid) => {
                    let mut sessions = state.sessions.lock().await;
                    if sessions.get_session(sid).is_ok() {
                        Some(sid.to_string())
                    } else {
                        None
                    }
                }
                None => None,
            }
        } else {
            None
        };
        let (session_id, provider) = if let Some(sid) = persistent_reuse {
            let mut sessions = state.sessions.lock().await;
            let s = sessions.get_session(&sid).map_err(|e| e.to_string())?;
            (s.id.clone(), s.provider.clone())
        } else {
            let mut sessions = state.sessions.lock().await;
            let s = sessions
                .create_session(
                    model.clone(),
                    default_provider.clone(),
                    None,
                    Some(project.to_string_lossy().to_string()),
                    Some(task.work_mode.clone()),
                    None,
                    Some(permission_mode.to_string()),
                )
                .map_err(|e| e.to_string())?;
            (s.id.clone(), s.provider.clone())
        };
        // Persistent first fire / stale-id fallback: persist the session id
        // so the NEXT fire reuses it.
        if task.persistent && task.persistent_session_id.as_deref() != Some(&session_id) {
            let mut updated = task.clone();
            updated.persistent_session_id = Some(session_id.clone());
            let _ = self.store.upsert_task(&updated);
        }
        let _ = self.store.update_run(
            run_id,
            RunStatus::Running,
            Some(&session_id),
            "",
            "",
            "",
        );
        if let Some(run) = self.store.get_run(run_id) {
            let _ = app.emit("scheduled-run-updated", &run);
        }

        // ── Optional worktree isolation ────────────────────────────────
        let mut run_workspace = project;
        // Persistent agents never use a worktree — their files accumulate in
        // the project across fires (a per-run forked worktree would not carry
        // prior edits). Belt-and-braces against an old row with both flags.
        if task.use_worktree && !task.persistent {
            if !is_git_repo(&run_workspace) {
                let mut sessions = state.sessions.lock().await;
                let _ = sessions.delete_session(&session_id);
                return Err(
                    "项目不是 git 仓库，无法使用 worktree；请关闭该选项或改用 git 项目".to_string(),
                );
            }
            run_workspace = state
                .worktree_isolation
                .create_isolated_worktree(
                    &run_workspace,
                    &session_id,
                    Some(crate::workspace::isolation::IsolationMode::Linked),
                )
                .await
                .map_err(|e| e.to_string())?;
            let _ = self.store.update_run(
                run_id,
                RunStatus::Running,
                None,
                "",
                "",
                &run_workspace.to_string_lossy(),
            );
        }

        let usage_tracker = state.usage_tracker(&session_id).await;
        let run_workspace_for_tracker = run_workspace.clone();
        let built = AgentBuilder::from_state(state, Some(run_workspace))?
            .with_mode(crate::agent::agent_loop::AgentLoopMode::Standard)
            .with_work_mode(work_mode)
            .with_usage_tracker(usage_tracker)
            .with_debug_mode(state.debug_mode())
            .with_provider(Some(provider.clone()))
            .build();

        // Bounded by the same global session-concurrency semaphore as
        // interactive runs — a scheduled burst can never starve the user.
        let permit = state
            .session_concurrency
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| format!("并发闸门不可用: {e}"))?;

        let mut chat_state = state
            .sessions
            .lock()
            .await
            .take_chat_state(&session_id)
            .map_err(|e| e.to_string())?;
        let trace_id = crate::core::ids::trace_id();
        chat_state.trace_id = Some(trace_id.clone());
        tracing::info!(session_id = %session_id, trace_id = %trace_id, "Scheduled run trace started");

        // ── Unattended posture + loop registration ─────────────────────
        // Registered AFTER every fallible step above: any early `?` return
        // before this point runs without unattended/cancel registrations, so
        // it cannot leak a permanently-unattended session (Ask → silent deny
        // on later interactive turns). From here to `finalize_run` below
        // there is no fallible step.
        state.mark_unattended(&session_id).await;
        let cancel_token = CancellationToken::new();
        state.register_cancellation(&session_id, cancel_token.clone()).await;
        let _pause_rx = state.register_pause(&session_id).await;

        let result = built
            .loop_
            .run(
                app,
                &session_id,
                &mut chat_state,
                &task.prompt,
                &cancel_token,
                state.debug_mode(),
                Some(FileStateTracker::new(Some(run_workspace_for_tracker))),
                Some(&state.skill_engine),
            )
            .await;
        let summary =
            summarize_conversation(&chat_state.conversation, chat_state.prompt_index as u64);

        // Persist + restore, mirroring the interactive send path.
        {
            let mut sessions = state.sessions.lock().await;
            let _ = sessions.put_chat_state(&session_id, chat_state);
            let _ = sessions.persist_session(&session_id);
            let _ = sessions.persist_messages(&session_id);
        }
        state.unmark_unattended(&session_id).await;
        let status = match &result {
            Ok(_) => "completed",
            Err(e) if e.is_cancelled() => "cancelled",
            Err(_) => "error",
        };
        state.finalize_run(app, &session_id, status).await;
        drop(permit);

        match result {
            Ok(_) => Ok((session_id, summary)),
            Err(e) if e.is_cancelled() => Err(CANCELLED_SUMMARY.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn emit_run(&self, app: &AppHandle, run: &ScheduledRun) {
        let _ = app.emit("scheduled-run-updated", run);
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Whether `dir` is inside a git working tree.
fn is_git_repo(dir: &Path) -> bool {
    let mut cmd = std::process::Command::new("git");
    crate::core::proc::no_window(&mut cmd);
    cmd.current_dir(dir)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_detection_returns_false_for_missing_dir() {
        assert!(!is_git_repo(Path::new(
            "C:\\definitely-not-a-deepdepcat-repo-982734"
        )));
    }

    #[test]
    fn running_slot_releases_on_drop() {
        let running = Arc::new(Mutex::new(HashSet::new()));
        {
            let slot = RunningSlot {
                task_id: "t1".into(),
                running: running.clone(),
            };
            running.lock().unwrap().insert("t1".to_string());
            assert!(running.lock().unwrap().contains("t1"));
            drop(slot);
        }
        assert!(!running.lock().unwrap().contains("t1"));
    }
}
