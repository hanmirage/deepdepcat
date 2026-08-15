//! Task manager — tracks depwork tasks (create/list). Task execution is
//! handled by the agent loop itself; this manager is a lightweight store.
//!
//! Persistence: when a database is attached (`with_db`), every mutation
//! writes through to the `tasks` table and the manager hydrates from it on
//! startup — the sidebar task list survives app restarts instead of being
//! wiped with the process (#84 audit: the `tasks` table existed in the
//! schema but nothing ever wrote to it).

use crate::core::types::{CoworkTask, TaskStatus, TaskType};
use crate::storage::database::Database;
use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// A tracked task.
struct RunningTask {
    task: CoworkTask,
}

/// The task manager — creates and lists depwork tasks.
pub struct TaskManager {
    tasks: Arc<RwLock<HashMap<String, RunningTask>>>,
    db: Option<Arc<Database>>,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            db: None,
        }
    }

    /// Attach the database for persistence. Hydrates the in-memory map from
    /// the `tasks` table so previously created tasks re-appear.
    ///
    /// Called once at construction time (app startup / test setup), before
    /// any concurrent access exists — so it replaces the whole lock with a
    /// pre-populated map instead of taking it. No `blocking_write` (panics
    /// inside a tokio runtime) and no `try_write`-spin loop (deadlock risk
    /// when a lock holder waits on this thread's scheduler) — the previous
    /// hydration loop could spin forever (#88 fix).
    pub fn with_db(mut self, db: Arc<Database>) -> Self {
        self.db = Some(db.clone());
        let mut hydrated: HashMap<String, RunningTask> = HashMap::new();
        for task in crate::storage::database::list_tasks(&db) {
            hydrated.insert(task.id.clone(), RunningTask { task });
        }
        self.tasks = Arc::new(RwLock::new(hydrated));
        info!("TaskManager hydrated from database");
        self
    }

    /// Create a new task. `session_id` links the task to the agent session
    /// that created it (the `task_manage` tool passes its session; the
    /// frontend sidebar passes `None`) — the per-session list and
    /// one-runner demotion stay scoped to that session.
    pub async fn create_task(
        &self,
        description: impl Into<String>,
        task_type: TaskType,
        context_paths: Vec<String>,
        session_id: Option<String>,
    ) -> String {
        let task_id = crate::core::ids::task_id(match task_type {
            TaskType::LocalBash => "bash",
            TaskType::LocalAgent => "agent",
            TaskType::RemoteAgent => "remote",
            TaskType::LocalWorkflow => "workflow",
            TaskType::MonitorMcp => "monitor",
            TaskType::Dream => "dream",
        });

        let task = CoworkTask {
            id: task_id.clone(),
            description: description.into(),
            status: TaskStatus::Pending,
            context_paths,
            created_at: Utc::now(),
            completed_at: None,
            session_id,
        };

        self.tasks
            .write()
            .await
            .insert(task_id.clone(), RunningTask { task: task.clone() });
        if let Some(ref db) = self.db {
            crate::storage::database::insert_task(db, &task);
        }

        info!(task_id = %task_id, "Task created");
        task_id
    }

    /// Get a task by ID.
    pub async fn get_task(&self, task_id: &str) -> Option<CoworkTask> {
        self.tasks.read().await.get(task_id).map(|r| r.task.clone())
    }

    /// List all tasks (frontend sidebar view — every session).
    pub async fn list_tasks(&self) -> Vec<CoworkTask> {
        self.tasks
            .read()
            .await
            .values()
            .map(|r| r.task.clone())
            .collect()
    }

    /// List the tasks belonging to ONE agent session. The `task_manage`
    /// tool uses this so one session's task list (and the one-active-task
    /// demotion) never touches another session's tasks.
    pub async fn list_tasks_for_session(&self, session_id: &str) -> Vec<CoworkTask> {
        self.tasks
            .read()
            .await
            .values()
            .filter(|r| r.task.session_id.as_deref() == Some(session_id))
            .map(|r| r.task.clone())
            .collect()
    }

    /// Update a task's status. Returns `false` when the task doesn't exist.
    pub async fn update_task_status(&self, task_id: &str, status: TaskStatus) -> bool {
        let mut tasks = self.tasks.write().await;
        let Some(entry) = tasks.get_mut(task_id) else {
            return false;
        };
        entry.task.status = status;
        if status.is_terminal() {
            entry.task.completed_at = Some(Utc::now());
        }
        if let Some(ref db) = self.db {
            crate::storage::database::update_task(db, task_id, status);
        }
        true
    }

    /// Delete a task. Returns `false` when the task doesn't exist.
    pub async fn delete_task(&self, task_id: &str) -> bool {
        let removed = self.tasks.write().await.remove(task_id).is_some();
        if removed {
            if let Some(ref db) = self.db {
                crate::storage::database::delete_task(db, task_id);
            }
        }
        removed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_update_delete_roundtrip() {
        let manager = TaskManager::new();
        let id = manager
            .create_task(
                "write docs".to_string(),
                TaskType::LocalWorkflow,
                vec![],
                None,
            )
            .await;

        let task = manager.get_task(&id).await.unwrap();
        assert_eq!(task.status, TaskStatus::Pending);

        assert!(manager.update_task_status(&id, TaskStatus::Running).await);
        assert_eq!(
            manager.get_task(&id).await.unwrap().status,
            TaskStatus::Running
        );

        assert!(manager.update_task_status(&id, TaskStatus::Completed).await);
        assert!(manager.get_task(&id).await.unwrap().completed_at.is_some());

        assert!(
            !manager
                .update_task_status("nope", TaskStatus::Running)
                .await
        );
        assert!(manager.delete_task(&id).await);
        assert!(!manager.delete_task(&id).await);
        assert!(manager.list_tasks().await.is_empty());
    }

    #[tokio::test]
    async fn only_one_running_via_demotion() {
        let manager = TaskManager::new();
        let a = manager
            .create_task("a", TaskType::LocalWorkflow, vec![], None)
            .await;
        let b = manager
            .create_task("b", TaskType::LocalWorkflow, vec![], None)
            .await;
        manager.update_task_status(&a, TaskStatus::Running).await;
        manager.update_task_status(&b, TaskStatus::Running).await;
        // The tool demotes other runners before starting a new one; the
        // manager itself allows both, but the tool layer enforces one-runner.
        let running = manager
            .list_tasks()
            .await
            .into_iter()
            .filter(|t| t.status == TaskStatus::Running)
            .count();
        assert_eq!(running, 2);
        // Tool-layer behavior (matches TaskManageTool.update).
        manager.update_task_status(&a, TaskStatus::Pending).await;
        let running = manager
            .list_tasks()
            .await
            .into_iter()
            .filter(|t| t.status == TaskStatus::Running)
            .count();
        assert_eq!(running, 1);
    }

    #[tokio::test]
    async fn tasks_persist_and_hydrate_across_manager_recreation() {
        // The tasks table must survive a manager rebuild (app restart):
        // create → recreate from the same DB → the task is still there,
        // with its status/context paths intact.
        let dir = tempfile_dir();
        let db =
            Arc::new(crate::storage::database::Database::open(&dir.join("t.db"), true).unwrap());
        db.run_migrations().unwrap();

        // `tasks.session_id` has a FK to `sessions` — seed the parent row
        // (same pattern as checkpoint.rs) so a session-linked task persists.
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .unwrap()
            .execute(
                "INSERT INTO sessions
                     (id, title, model, provider, status, created_at, updated_at,
                      system_prompt, turn_count, prompt_tokens, completion_tokens)
                 VALUES ('sess-1', 'test', 'deepseek', 'deepseek', 'active', ?1, ?2, '', 0, 0, 0)",
                rusqlite::params![now, now],
            )
            .unwrap();

        let manager = TaskManager::new().with_db(db.clone());
        let id = manager
            .create_task(
                "修复目录下的bug".to_string(),
                TaskType::LocalAgent,
                vec!["src/main.rs".to_string()],
                Some("sess-1".to_string()),
            )
            .await;
        manager.update_task_status(&id, TaskStatus::Running).await;

        // Simulate restart: fresh manager over the same database.
        let restarted = TaskManager::new().with_db(db.clone());
        let tasks = restarted.list_tasks().await;
        assert_eq!(tasks.len(), 1, "task must survive manager recreation");
        let t = &tasks[0];
        assert_eq!(t.id, id);
        assert_eq!(t.description, "修复目录下的bug");
        assert_eq!(t.status, TaskStatus::Running);
        assert_eq!(t.context_paths, vec!["src/main.rs".to_string()]);

        // Deleting removes the row — a second restart sees it gone.
        restarted.delete_task(&id).await;
        let again = TaskManager::new().with_db(db.clone());
        assert!(again.list_tasks().await.is_empty());
    }

    #[tokio::test]
    async fn list_tasks_for_session_is_isolated() {
        let manager = TaskManager::new();
        let _a = manager
            .create_task("a", TaskType::LocalWorkflow, vec![], Some("s1".into()))
            .await;
        let _b = manager
            .create_task("b", TaskType::LocalWorkflow, vec![], Some("s1".into()))
            .await;
        let _c = manager
            .create_task("c", TaskType::LocalWorkflow, vec![], Some("s2".into()))
            .await;
        let _d = manager
            .create_task("d", TaskType::LocalWorkflow, vec![], None)
            .await;

        assert_eq!(manager.list_tasks_for_session("s1").await.len(), 2);
        assert_eq!(manager.list_tasks_for_session("s2").await.len(), 1);
        assert_eq!(manager.list_tasks_for_session("nope").await.len(), 0);
        // The sidebar (all tasks) still sees everything.
        assert_eq!(manager.list_tasks().await.len(), 4);
    }

    fn tempfile_dir() -> std::path::PathBuf {
        // Unique per call: pid-based dirs collide across parallel tests in
        // the same binary and across reused pids between test runs, which
        // made the hydration test fail on a stale `sessions.id` row.
        let dir = std::env::temp_dir().join(format!(
            "ddc-task-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
