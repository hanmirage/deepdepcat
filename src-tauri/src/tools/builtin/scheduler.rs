//! Scheduler tool — create, list, and delete scheduled (recurring) tasks.
//!
//! Allows the agent to schedule periodic operations like running tests,
//! checking build status, or monitoring file changes at fixed intervals.

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};
use tauri::Emitter;

/// A scheduled task definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTask {
    /// Unique ID for this task.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// The command to execute.
    pub command: String,
    /// Interval in seconds between executions.
    pub interval_secs: u64,
    /// Whether the task is currently active.
    pub active: bool,
    /// Number of times this task has fired.
    pub run_count: u64,
    /// Unix epoch ms of the last execution (None = never ran).
    #[serde(default)]
    pub last_run_at_ms: Option<u64>,
}

/// Shared scheduler store — holds all scheduled tasks.
#[derive(Clone)]
pub struct SchedulerStore {
    tasks: Arc<RwLock<Vec<ScheduledTask>>>,
}

impl SchedulerStore {
    /// Create a new empty scheduler store.
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Add a task to the store.
    pub fn add(&self, task: ScheduledTask) {
        self.tasks
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .push(task);
    }

    /// Remove a task by ID.
    pub fn remove(&self, id: &str) -> bool {
        let mut tasks = self.tasks.write().unwrap_or_else(|e| e.into_inner());
        if let Some(pos) = tasks.iter().position(|t| t.id == id) {
            tasks.remove(pos);
            return true;
        }
        false
    }

    /// List all tasks.
    pub fn list(&self) -> Vec<ScheduledTask> {
        self.tasks.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Get a task by ID.
    pub fn get(&self, id: &str) -> Option<ScheduledTask> {
        self.tasks
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|t| t.id == id)
            .cloned()
    }

    /// Tasks whose interval has elapsed since their last run (or that have
    /// never run).
    pub fn due_tasks(&self, now_ms: u64) -> Vec<ScheduledTask> {
        self.tasks
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter(|t| {
                t.active
                    && t.interval_secs > 0
                    && t.last_run_at_ms
                        .map(|last| {
                            now_ms.saturating_sub(last) >= t.interval_secs.saturating_mul(1000)
                        })
                        .unwrap_or(true)
            })
            .cloned()
            .collect()
    }

    /// Mark a task as run at `now_ms` and bump its counter.
    pub fn mark_run(&self, id: &str, now_ms: u64) {
        if let Some(task) = self
            .tasks
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
            .find(|t| t.id == id)
        {
            task.last_run_at_ms = Some(now_ms);
            task.run_count += 1;
        }
    }
}

impl Default for SchedulerStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Background runner that executes due scheduled tasks.
///
/// Polls the store every `poll_interval_secs`; every task whose interval
/// has elapsed fires its command through the platform shell. Output is
/// logged; failures never crash the loop.
///
/// Security (#88 audit H8): unlike the interactive `bash` tool, scheduled
/// commands run UNATTENDED — there is no user to approve each execution.
/// Two guards close that gap:
/// 1. Every command passes the full bash permission pipeline (rules +
///    bash-security layers) before it runs; a denied command is skipped
///    with a warning, never executed silently.
/// 2. Execution is bounded by [`SCHEDULER_COMMAND_TIMEOUT_SECS`] — a hung
///    command (interactive prompt, `ping -t`, ...) can no longer wedge the
///    whole scheduler loop forever.
pub struct SchedulerRunner {
    store: SchedulerStore,
    poll_interval_secs: u64,
    permissions: crate::permissions::checker::PermissionChecker,
}

/// Wall-clock cap for one scheduled command execution.
const SCHEDULER_COMMAND_TIMEOUT_SECS: u64 = 30;

impl SchedulerRunner {
    pub fn new(
        store: SchedulerStore,
        poll_interval_secs: u64,
        permissions: crate::permissions::checker::PermissionChecker,
    ) -> Self {
        Self {
            store,
            poll_interval_secs,
            permissions,
        }
    }

    /// Spawn the polling loop as a detached tokio task.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                self.poll_interval_secs.max(1),
            ));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                self.run_due().await;
            }
        })
    }

    async fn run_due(&self) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let due = self.store.due_tasks(now_ms);
        for task in due {
            // Re-check under the store lock: the task may have been
            // removed/deactivated between due_tasks and now.
            let still_due = self
                .store
                .get(&task.id)
                .map(|t| {
                    t.active
                        && t.last_run_at_ms
                            .map(|last| {
                                now_ms.saturating_sub(last) >= t.interval_secs.saturating_mul(1000)
                            })
                            .unwrap_or(true)
                })
                .unwrap_or(false);
            if !still_due {
                continue;
            }

            // ── Permission gate (unattended execution) ─────────────────
            // The command goes through the FULL bash pipeline as if the
            // agent typed it into the bash tool: project rules, settings
            // rules, filesystem validation, and the bash-security layers
            // (destructive commands, rm -rf, etc.). Deny → skip + warn;
            // Ask → skip + warn (there is nobody to prompt); Allow → run.
            // A fixed key keeps the unattended scheduler's denials out of
            // every interactive session's budget (and vice versa).
            let verdict = self.permissions.check(
                "bash",
                &serde_json::json!({ "command": task.command }),
                false,
                "scheduler",
            );
            use crate::permissions::checker::PermissionResult;
            match verdict {
                PermissionResult::Deny(reason) => {
                    tracing::warn!(
                        task_id = %task.id,
                        name = %task.name,
                        reason = %reason,
                        "Scheduled task denied by permission pipeline — skipping execution"
                    );
                    continue;
                }
                PermissionResult::Ask => {
                    tracing::warn!(
                        task_id = %task.id,
                        name = %task.name,
                        "Scheduled task requires approval — unattended execution skipped"
                    );
                    continue;
                }
                PermissionResult::Allow => {}
            }

            self.store.mark_run(&task.id, now_ms);
            let output = execute_command(&task.command).await;
            match output {
                Ok(out) => {
                    tracing::info!(
                        task_id = %task.id,
                        name = %task.name,
                        run_count = task.run_count,
                        output_len = out.len(),
                        "Scheduled task executed"
                    );
                }
                Err(e) => {
                    tracing::warn!(task_id = %task.id, name = %task.name, error = %e, "Scheduled task failed");
                }
            }
        }
    }
}

/// Execute a shell command (platform default shell) and capture output.
/// Bounded by [`SCHEDULER_COMMAND_TIMEOUT_SECS`] so a hung command cannot
/// wedge the scheduler loop.
async fn execute_command(command: &str) -> Result<String, String> {
    #[cfg(windows)]
    let (shell, flag) = ("cmd", "/C");
    #[cfg(not(windows))]
    let (shell, flag) = ("sh", "-c");

    let mut cmd = tokio::process::Command::new(shell);
    crate::core::proc::no_window_tokio(&mut cmd);
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(SCHEDULER_COMMAND_TIMEOUT_SECS),
        cmd.arg(flag).arg(command).output(),
    )
    .await
    .map_err(|_| {
        format!(
            "Command timed out after {SCHEDULER_COMMAND_TIMEOUT_SECS}s — it was killed to protect the scheduler"
        )
    })?
    .map_err(|e| e.to_string())?;
    let stdout = crate::core::encoding::decode_native_output(&output.stdout);
    let stderr = crate::core::encoding::decode_native_output(&output.stderr);
    if output.status.success() {
        Ok(if stderr.is_empty() {
            stdout
        } else {
            format!("{stdout}\n{stderr}")
        })
    } else {
        Err(if stderr.is_empty() { stdout } else { stderr })
    }
}

/// Tool for creating a scheduled task.
pub struct SchedulerCreateTool {
    store: SchedulerStore,
}

impl SchedulerCreateTool {
    /// Create a new scheduler create tool with the given store.
    pub fn new(store: SchedulerStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for SchedulerCreateTool {
    fn name(&self) -> &str {
        "scheduler_create"
    }

    fn description(&self) -> &str {
        "Create a scheduled task that runs a command at fixed intervals. \
        Useful for periodic monitoring, running tests, or checking build status."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Human-readable name for this scheduled task"
                },
                "command": {
                    "type": "string",
                    "description": "The shell command to execute on each interval"
                },
                "interval_secs": {
                    "type": "integer",
                    "description": "Interval between executions in seconds (minimum 5)",
                    "minimum": 5
                }
            },
            "required": ["name", "command", "interval_secs"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Side-effecting — never run in parallel with other tools.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn scope(&self) -> crate::toolkit::ToolScope {
        // Scheduled tasks execute SHELL commands on an interval — Depwork
        // has no shell and must not schedule command execution.
        crate::toolkit::ToolScope::Code
    }

    fn check_permissions(&self, _args: &Value, _ctx: &ToolContext) -> PermissionDecision {
        PermissionDecision::Ask
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> AppResult<ToolResult> {
        let name = args
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::core::error::AppError::ToolNotFound("missing 'name'".into()))?;

        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::core::error::AppError::ToolNotFound("missing 'command'".into())
            })?;

        let interval_secs = args
            .get("interval_secs")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| {
                crate::core::error::AppError::ToolNotFound("missing 'interval_secs'".into())
            })?;

        if interval_secs < 5 {
            return Ok(ToolResult::error("interval_secs must be at least 5"));
        }
        // Cap at 30 days — the model supplies this value; a pathological
        // number must not break the runner's timing math.
        let interval_secs = interval_secs.min(86_400 * 30);

        let task = ScheduledTask {
            id: crate::core::ids::generate_id(),
            name: name.to_string(),
            command: command.to_string(),
            interval_secs,
            active: true,
            run_count: 0,
            last_run_at_ms: None,
        };

        let _ = ctx.app.emit("scheduler-task-created", &task);
        self.store.add(task.clone());

        Ok(ToolResult::success(format!(
            "Scheduled task '{}' created with ID {} (every {}s)",
            task.name, task.id, task.interval_secs
        )))
    }
}

/// Tool for listing all scheduled tasks.
pub struct SchedulerListTool {
    store: SchedulerStore,
}

impl SchedulerListTool {
    /// Create a new scheduler list tool with the given store.
    pub fn new(store: SchedulerStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for SchedulerListTool {
    fn name(&self) -> &str {
        "scheduler_list"
    }

    fn description(&self) -> &str {
        "List all scheduled tasks with their IDs, names, intervals, and run counts."
    }

    fn parameters(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn scope(&self) -> crate::toolkit::ToolScope {
        // Scheduled tasks execute SHELL commands on an interval — Depwork
        // has no shell and must not schedule command execution.
        crate::toolkit::ToolScope::Code
    }

    async fn execute(&self, _args: Value, _ctx: &ToolContext) -> AppResult<ToolResult> {
        let tasks = self.store.list();
        if tasks.is_empty() {
            return Ok(ToolResult::success("No scheduled tasks."));
        }

        let lines: Vec<String> = tasks
            .iter()
            .map(|t| {
                format!(
                    "- [{}] {} (every {}s, runs: {}, active: {})",
                    t.id, t.name, t.interval_secs, t.run_count, t.active
                )
            })
            .collect();

        Ok(ToolResult::success(lines.join("\n")))
    }
}

/// Tool for deleting a scheduled task.
pub struct SchedulerDeleteTool {
    store: SchedulerStore,
}

impl SchedulerDeleteTool {
    /// Create a new scheduler delete tool with the given store.
    pub fn new(store: SchedulerStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for SchedulerDeleteTool {
    fn name(&self) -> &str {
        "scheduler_delete"
    }

    fn description(&self) -> &str {
        "Delete a scheduled task by its ID. The task is stopped and removed."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": {
                    "type": "string",
                    "description": "The ID of the scheduled task to delete"
                }
            },
            "required": ["task_id"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Side-effecting — never run in parallel with other tools.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn scope(&self) -> crate::toolkit::ToolScope {
        // Scheduled tasks execute SHELL commands on an interval — Depwork
        // has no shell and must not schedule command execution.
        crate::toolkit::ToolScope::Code
    }

    fn check_permissions(&self, _args: &Value, _ctx: &ToolContext) -> PermissionDecision {
        PermissionDecision::Ask
    }

    async fn execute(&self, args: Value, _ctx: &ToolContext) -> AppResult<ToolResult> {
        let task_id = args
            .get("task_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::core::error::AppError::ToolNotFound("missing 'task_id'".into())
            })?;

        if self.store.remove(task_id) {
            Ok(ToolResult::success(format!(
                "Scheduled task {task_id} deleted."
            )))
        } else {
            Ok(ToolResult::error(format!("Task {task_id} not found.")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_add_remove_list() {
        let store = SchedulerStore::new();
        let task = ScheduledTask {
            id: "t1".into(),
            name: "test".into(),
            command: "echo hi".into(),
            interval_secs: 10,
            active: true,
            run_count: 0,
            last_run_at_ms: None,
        };
        store.add(task);
        assert_eq!(store.list().len(), 1);
        assert!(store.get("t1").is_some());
        assert!(store.remove("t1"));
        assert!(store.list().is_empty());
    }

    #[test]
    fn due_tasks_never_ran_and_interval_elapsed() {
        let store = SchedulerStore::new();
        store.add(ScheduledTask {
            id: "never".into(),
            name: "a".into(),
            command: "echo".into(),
            interval_secs: 10,
            active: true,
            run_count: 0,
            last_run_at_ms: None,
        });
        store.add(ScheduledTask {
            id: "elapsed".into(),
            name: "b".into(),
            command: "echo".into(),
            interval_secs: 10,
            active: true,
            run_count: 1,
            last_run_at_ms: Some(1_000), // 10s ago at now=11_000
        });
        store.add(ScheduledTask {
            id: "not-yet".into(),
            name: "c".into(),
            command: "echo".into(),
            interval_secs: 60,
            active: true,
            run_count: 1,
            last_run_at_ms: Some(10_000), // only 1s ago at now=11_000
        });
        store.add(ScheduledTask {
            id: "disabled".into(),
            name: "d".into(),
            command: "echo".into(),
            interval_secs: 5,
            active: false,
            run_count: 0,
            last_run_at_ms: None,
        });

        let due = store.due_tasks(11_000);
        let ids: Vec<&str> = due.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"never"));
        assert!(ids.contains(&"elapsed"));
        assert!(!ids.contains(&"not-yet"));
        assert!(!ids.contains(&"disabled"));
    }

    #[test]
    fn mark_run_sets_last_run_and_bumps_count() {
        let store = SchedulerStore::new();
        store.add(ScheduledTask {
            id: "t1".into(),
            name: "a".into(),
            command: "echo".into(),
            interval_secs: 5,
            active: true,
            run_count: 0,
            last_run_at_ms: None,
        });
        store.mark_run("t1", 5_000);
        let task = store.get("t1").unwrap();
        assert_eq!(task.last_run_at_ms, Some(5_000));
        assert_eq!(task.run_count, 1);
        // After mark_run the task is not due again until the interval passes.
        assert!(store.due_tasks(5_100).is_empty());
        assert!(store.due_tasks(10_100).iter().any(|t| t.id == "t1"));
    }
}
