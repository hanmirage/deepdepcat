//! Task persistence — CoworkTask rows in the `tasks` table.
//!
//! The task panel (sidebar TaskSection + taskApi.listTasks) reads from the
//! backend TaskManager, which used to be a memory-only HashMap: every app
//! restart silently wiped the user's task list. This module makes the
//! manager durable — create/update/delete write through to SQLite, and the
//! manager hydrates from the table on startup.

use crate::core::types::{CoworkTask, TaskStatus};
use crate::storage::database::Database;
use rusqlite::params;
use std::sync::Arc;

/// Read all persisted tasks, newest first.
pub fn list_tasks(db: &Arc<Database>) -> Vec<CoworkTask> {
    let Ok(conn) = db.conn() else {
        tracing::warn!("Task list read failed — in-memory list may diverge from disk");
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, description, status, context_paths, session_id, created_at, completed_at \
         FROM tasks ORDER BY created_at DESC",
    ) else {
        tracing::warn!("Task list query prepare failed — returning empty");
        return Vec::new();
    };
    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let description: String = row.get(1)?;
        let status: String = row.get(2)?;
        let context_paths: Option<String> = row.get(3)?;
        let session_id: Option<String> = row.get(4)?;
        let created_at: String = row.get(5)?;
        let completed_at: Option<String> = row.get(6)?;
        Ok(CoworkTask {
            id,
            description,
            status: parse_status(&status),
            context_paths: parse_json_list(context_paths.as_deref()),
            created_at: chrono::DateTime::parse_from_rfc3339(&created_at)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .unwrap_or_else(|_| chrono::Utc::now()),
            completed_at: completed_at
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&chrono::Utc)),
            session_id,
        })
    });
    match rows {
        Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
        Err(_) => {
            tracing::warn!("Task list query failed — returning empty");
            Vec::new()
        }
    }
}

/// Persist a newly created task.
pub fn insert_task(db: &Arc<Database>, task: &CoworkTask) {
    let Ok(conn) = db.conn() else {
        tracing::warn!("Task insert failed to lock DB — in-memory task will not persist");
        return;
    };
    let _ = conn.execute(
        "INSERT INTO tasks (id, description, status, context_paths, session_id, created_at, completed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            task.id,
            task.description,
            task.status.as_str(),
            serde_json::to_string(&task.context_paths).unwrap_or_else(|_| "[]".to_string()),
            task.session_id,
            task.created_at.to_rfc3339(),
            task.completed_at.map(|t| t.to_rfc3339()),
        ],
    )
    .map_err(|e| tracing::warn!(error = %e, "Failed to persist task"))
    .is_ok();
}

/// Update a task's status and completion time.
pub fn update_task(db: &Arc<Database>, task_id: &str, status: TaskStatus) {
    let Ok(conn) = db.conn() else {
        tracing::warn!("Task update failed to lock DB — in-memory status will not persist");
        return;
    };
    let completed_at = if status.is_terminal() {
        Some(chrono::Utc::now().to_rfc3339())
    } else {
        // Re-opening a completed task must CLEAR the stale completion time —
        // the old COALESCE kept it, so a pending/running task carried a
        // completed_at it never earned.
        None
    };
    let _ = conn
        .execute(
            "UPDATE tasks SET status = ?1, completed_at = ?2 WHERE id = ?3",
            params![status.as_str(), completed_at, task_id],
        )
        .map_err(|e| tracing::warn!(error = %e, "Failed to persist task status"));
}

/// Remove a task row.
pub fn delete_task(db: &Arc<Database>, task_id: &str) {
    let Ok(conn) = db.conn() else {
        tracing::warn!("Task delete failed to lock DB — in-memory deletion will not persist");
        return;
    };
    let _ = conn
        .execute("DELETE FROM tasks WHERE id = ?1", params![task_id])
        .map_err(|e| tracing::warn!(error = %e, "Failed to delete task row"));
}

fn parse_status(s: &str) -> TaskStatus {
    match s {
        "pending" => TaskStatus::Pending,
        "running" => TaskStatus::Running,
        "completed" => TaskStatus::Completed,
        "failed" => TaskStatus::Failed,
        "killed" => TaskStatus::Killed,
        _ => TaskStatus::Pending,
    }
}

fn parse_json_list(s: Option<&str>) -> Vec<String> {
    s.and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::CoworkTask;
    use chrono::Utc;

    fn test_db() -> Arc<Database> {
        let dir = std::env::temp_dir().join(format!(
            "ddc-tasks-test-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("test.db"), false).unwrap();
        db.run_migrations().unwrap();
        Arc::new(db)
    }

    fn task(id: &str) -> CoworkTask {
        CoworkTask {
            id: id.to_string(),
            description: "t".to_string(),
            status: TaskStatus::Pending,
            context_paths: vec![],
            created_at: Utc::now(),
            completed_at: None,
            session_id: None,
        }
    }

    #[test]
    fn reopening_completed_task_clears_completed_at() {
        // Regression: re-opening a completed task must clear the stale
        // completion timestamp — the old COALESCE kept it, so a pending/running
        // task carried a completed_at it never earned.
        let db = test_db();
        insert_task(&db, &task("t1"));
        update_task(&db, "t1", TaskStatus::Completed);
        let completed = list_tasks(&db);
        assert!(
            completed[0].completed_at.is_some(),
            "a completed task records its completion time"
        );

        // Re-open → back to Pending → completed_at must be gone.
        update_task(&db, "t1", TaskStatus::Pending);
        let reopened = list_tasks(&db);
        assert_eq!(reopened[0].status, TaskStatus::Pending);
        assert!(
            reopened[0].completed_at.is_none(),
            "a re-opened task must not retain a stale completion time"
        );
        // And re-completing sets it again.
        update_task(&db, "t1", TaskStatus::Completed);
        assert!(list_tasks(&db)[0].completed_at.is_some());
        drop(db);
        // Temp dir cleanup is best-effort; leave the OS to reap it.
    }

    #[test]
    fn persistence_roundtrip() {
        let db = test_db();
        insert_task(&db, &task("a"));
        insert_task(&db, &task("b"));
        assert_eq!(list_tasks(&db).len(), 2);
        delete_task(&db, "a");
        let remaining = list_tasks(&db);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "b");
        drop(db);
    }
}
