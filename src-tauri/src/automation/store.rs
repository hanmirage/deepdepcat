//! Durable storage for scheduled tasks and their runs.
//!
//! All methods are synchronous SQLite round-trips (the database wrapper is
//! a `Mutex<Connection>`); commands and the runner call them directly — a
//! local desktop DB makes this cheap and keeps the code simple.

use super::{RunStatus, ScheduleSpec, ScheduledRun, ScheduledTask};
use crate::storage::database::Database;
use chrono::{DateTime, Utc};
use rusqlite::params;
use std::sync::Arc;

/// Cloneable handle over the scheduled-task tables.
#[derive(Clone)]
pub struct AutomationStore {
    db: Arc<Database>,
}

impl AutomationStore {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    // ── Tasks ─────────────────────────────────────────────────────────

    /// Persist a task (insert or update by id).
    pub fn upsert_task(&self, task: &ScheduledTask) -> Result<(), String> {
        let conn = self.db.conn().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO scheduled_tasks
                 (id, name, prompt, schedule_kind, every_secs, daily_time,
                  project_path, use_worktree, work_mode, model, active,
                  last_run_at_ms, run_count, created_at, updated_at,
                  persistent, persistent_session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 prompt = excluded.prompt,
                 schedule_kind = excluded.schedule_kind,
                 every_secs = excluded.every_secs,
                 daily_time = excluded.daily_time,
                 project_path = excluded.project_path,
                 use_worktree = excluded.use_worktree,
                 work_mode = excluded.work_mode,
                 model = excluded.model,
                 active = excluded.active,
                 last_run_at_ms = excluded.last_run_at_ms,
                 run_count = excluded.run_count,
                 updated_at = excluded.updated_at,
                 persistent = excluded.persistent,
                 -- The runner writes persistent_session_id back on the first
                 -- fire; a frontend update that omits it must NOT clobber it.
                 persistent_session_id = COALESCE(excluded.persistent_session_id, scheduled_tasks.persistent_session_id)",
            params![
                task.id,
                task.name,
                task.prompt,
                schedule_kind_str(&task.schedule),
                schedule_every_secs(&task.schedule),
                schedule_daily_time(&task.schedule),
                task.project_path,
                bool_to_int(task.use_worktree),
                task.work_mode,
                task.model,
                bool_to_int(task.active),
                task.last_run_at_ms,
                task.run_count,
                task.created_at.to_rfc3339(),
                task.updated_at.to_rfc3339(),
                bool_to_int(task.persistent),
                task.persistent_session_id,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Load all tasks, newest first.
    pub fn list_tasks(&self) -> Vec<ScheduledTask> {
        let Ok(conn) = self.db.conn() else {
            return Vec::new();
        };
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, name, prompt, schedule_kind, every_secs, daily_time,
                    project_path, use_worktree, work_mode, model, active,
                    last_run_at_ms, run_count, created_at, updated_at,
                    persistent, persistent_session_id
             FROM scheduled_tasks ORDER BY created_at DESC",
        ) else {
            return Vec::new();
        };
        stmt.query_map([], row_to_task)
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Load one task.
    pub fn get_task(&self, task_id: &str) -> Option<ScheduledTask> {
        let conn = self.db.conn().ok()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, name, prompt, schedule_kind, every_secs, daily_time,
                        project_path, use_worktree, work_mode, model, active,
                        last_run_at_ms, run_count, created_at, updated_at,
                        persistent, persistent_session_id
                 FROM scheduled_tasks WHERE id = ?1",
            )
            .ok()?;
        stmt.query_row(params![task_id], row_to_task).ok()
    }

    /// Delete a task (runs cascade via the foreign key).
    pub fn delete_task(&self, task_id: &str) -> Result<(), String> {
        let conn = self.db.conn().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM scheduled_tasks WHERE id = ?1",
            params![task_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Record a completed fire (sets last run time + bumps the counter).
    pub fn mark_task_run(&self, task_id: &str, now_ms: i64) {
        let Ok(conn) = self.db.conn() else {
            return;
        };
        let _ = conn.execute(
            "UPDATE scheduled_tasks
             SET last_run_at_ms = ?1, run_count = run_count + 1, updated_at = ?2
             WHERE id = ?3",
            params![
                now_ms,
                Utc::now().to_rfc3339(),
                task_id
            ],
        );
    }

    // ── Runs ──────────────────────────────────────────────────────────

    /// Insert a run row.
    pub fn insert_run(&self, run: &ScheduledRun) -> Result<(), String> {
        let conn = self.db.conn().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO scheduled_runs
                 (id, task_id, session_id, status, started_at, finished_at,
                  summary, error, worktree_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                run.id,
                run.task_id,
                run.session_id,
                run.status.as_str(),
                run.started_at.to_rfc3339(),
                run.finished_at.map(|t| t.to_rfc3339()),
                run.summary,
                run.error,
                run.worktree_path,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Update a run's lifecycle fields.
    pub fn update_run(
        &self,
        run_id: &str,
        status: RunStatus,
        session_id: Option<&str>,
        summary: &str,
        error: &str,
        worktree_path: &str,
    ) -> Result<(), String> {
        let conn = self.db.conn().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE scheduled_runs
             SET status = ?1,
                 session_id = COALESCE(?2, session_id),
                 finished_at = CASE WHEN ?3 THEN ?4 ELSE finished_at END,
                 summary = ?5,
                 error = ?6,
                 worktree_path = ?7
             WHERE id = ?8",
            params![
                status.as_str(),
                session_id,
                status_terminal_bool(status),
                Utc::now().to_rfc3339(),
                summary,
                error,
                worktree_path,
                run_id,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Load runs for a task (newest first) or all runs when `task_id` is
    /// `None`.
    pub fn list_runs(&self, task_id: Option<&str>, limit: i64) -> Vec<ScheduledRun> {
        let Ok(conn) = self.db.conn() else {
            return Vec::new();
        };
        let limit = limit.clamp(1, 500);
        let sql = match task_id {
            Some(_) => {
                "SELECT id, task_id, session_id, status, started_at, finished_at,
                        summary, error, worktree_path
                 FROM scheduled_runs WHERE task_id = ?1
                 ORDER BY started_at DESC LIMIT ?2"
            }
            None => {
                "SELECT id, task_id, session_id, status, started_at, finished_at,
                        summary, error, worktree_path
                 FROM scheduled_runs ORDER BY started_at DESC LIMIT ?1"
            }
        };
        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = match task_id {
            Some(id) => stmt.query_map(params![id, limit], row_to_run),
            None => stmt.query_map(params![limit], row_to_run),
        };
        rows.map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// Load one run.
    pub fn get_run(&self, run_id: &str) -> Option<ScheduledRun> {
        let conn = self.db.conn().ok()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, task_id, session_id, status, started_at, finished_at,
                        summary, error, worktree_path
                 FROM scheduled_runs WHERE id = ?1",
            )
            .ok()?;
        stmt.query_row(params![run_id], row_to_run).ok()
    }

    /// Delete a run row (the session transcript stays; worktree cleanup is
    /// a separate explicit command).
    pub fn delete_run(&self, run_id: &str) -> Result<(), String> {
        let conn = self.db.conn().map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM scheduled_runs WHERE id = ?1",
            params![run_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledTask> {
    let kind: String = row.get(3)?;
    let every_secs: i64 = row.get(4)?;
    let daily_time: String = row.get(5)?;
    Ok(ScheduledTask {
        id: row.get(0)?,
        name: row.get(1)?,
        prompt: row.get(2)?,
        schedule: match kind.as_str() {
            "daily" => ScheduleSpec::Daily { time: daily_time },
            _ => ScheduleSpec::Interval { every_secs },
        },
        project_path: row.get(6)?,
        use_worktree: row.get::<_, i64>(7)? != 0,
        work_mode: row.get(8)?,
        model: row.get(9)?,
        persistent: row.get::<_, i64>(15)? != 0,
        persistent_session_id: row.get(16)?,
        active: row.get::<_, i64>(10)? != 0,
        last_run_at_ms: row.get(11)?,
        run_count: row.get(12)?,
        created_at: parse_dt(row.get(13)?),
        updated_at: parse_dt(row.get(14)?),
    })
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduledRun> {
    Ok(ScheduledRun {
        id: row.get(0)?,
        task_id: row.get(1)?,
        session_id: row.get(2)?,
        status: RunStatus::from_str(&row.get::<_, String>(3)?),
        started_at: parse_dt(row.get(4)?),
        finished_at: row
            .get::<_, Option<String>>(5)?
            .map(parse_dt),
        summary: row.get(6)?,
        error: row.get(7)?,
        worktree_path: row.get(8)?,
    })
}

fn parse_dt(s: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn schedule_kind_str(spec: &ScheduleSpec) -> &'static str {
    match spec {
        ScheduleSpec::Interval { .. } => "interval",
        ScheduleSpec::Daily { .. } => "daily",
    }
}

fn schedule_every_secs(spec: &ScheduleSpec) -> i64 {
    match spec {
        ScheduleSpec::Interval { every_secs } => *every_secs,
        ScheduleSpec::Daily { .. } => 0,
    }
}

fn schedule_daily_time(spec: &ScheduleSpec) -> String {
    match spec {
        ScheduleSpec::Interval { .. } => String::new(),
        ScheduleSpec::Daily { time } => time.clone(),
    }
}

fn bool_to_int(b: bool) -> i64 {
    if b {
        1
    } else {
        0
    }
}

fn status_terminal_bool(status: RunStatus) -> bool {
    matches!(
        status,
        RunStatus::Completed
            | RunStatus::Failed
            | RunStatus::Skipped
            | RunStatus::Cancelled
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::ids;

    fn store() -> AutomationStore {
        let dir = std::env::temp_dir().join(format!(
            "ddc-automation-store-test-{}",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(Database::open(&dir.join("t.db"), false).unwrap());
        db.run_migrations().unwrap();
        AutomationStore::new(db)
    }

    fn task() -> ScheduledTask {
        ScheduledTask {
            id: ids::generate_id(),
            name: "每日巡检".into(),
            prompt: "检查测试并修复".into(),
            schedule: ScheduleSpec::Daily {
                time: "09:00".into(),
            },
            project_path: String::new(),
            use_worktree: false,
            work_mode: "code".into(),
            model: String::new(),
            active: true,
            persistent: false,
            persistent_session_id: None,
            last_run_at_ms: None,
            run_count: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn task_round_trip_preserves_schedule_and_flags() {
        let store = store();
        let t = task();
        store.upsert_task(&t).unwrap();
        let loaded = store.get_task(&t.id).unwrap();
        assert_eq!(loaded.name, "每日巡检");
        assert_eq!(
            loaded.schedule,
            ScheduleSpec::Daily {
                time: "09:00".into()
            }
        );
        assert_eq!(loaded.work_mode, "code");
        assert!(loaded.active);

        store.mark_task_run(&t.id, 1_234_567);
        let after = store.get_task(&t.id).unwrap();
        assert_eq!(after.run_count, 1);
        assert_eq!(after.last_run_at_ms, Some(1_234_567));

        store.delete_task(&t.id).unwrap();
        assert!(store.get_task(&t.id).is_none());
    }

    #[test]
    fn persistent_fields_round_trip_and_never_clobbered() {
        let store = store();
        let mut t = task();
        t.persistent = true;
        t.persistent_session_id = Some("sess-persistent".into());
        store.upsert_task(&t).unwrap();
        let loaded = store.get_task(&t.id).unwrap();
        assert!(loaded.persistent);
        assert_eq!(loaded.persistent_session_id.as_deref(), Some("sess-persistent"));

        // A frontend update that omits the session id must NOT clear it — the
        // runner writes it back on the first fire; a concurrent edit must not
        // clobber the persistent binding (COALESCE in ON CONFLICT).
        let mut edited = t.clone();
        edited.persistent_session_id = None;
        edited.updated_at = Utc::now();
        store.upsert_task(&edited).unwrap();
        let after = store.get_task(&t.id).unwrap();
        assert_eq!(
            after.persistent_session_id.as_deref(),
            Some("sess-persistent"),
            "None upsert must not clear the runner's write-back"
        );

        // An explicit new session id DOES update the binding (next fire).
        let mut moved = t.clone();
        moved.persistent_session_id = Some("sess-moved".into());
        store.upsert_task(&moved).unwrap();
        assert_eq!(
            store.get_task(&t.id).unwrap().persistent_session_id.as_deref(),
            Some("sess-moved")
        );
    }

    #[test]
    fn run_crud_and_cascade_delete() {
        let store = store();
        let t = task();
        store.upsert_task(&t).unwrap();

        let run = ScheduledRun {
            id: ids::generate_id(),
            task_id: t.id.clone(),
            session_id: Some("sess-1".into()),
            status: RunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            summary: String::new(),
            error: String::new(),
            worktree_path: String::new(),
        };
        store.insert_run(&run).unwrap();
        store
            .update_run(&run.id, RunStatus::Completed, None, "完成", "", "")
            .unwrap();
        let loaded = store.get_run(&run.id).unwrap();
        assert_eq!(loaded.status, RunStatus::Completed);
        assert_eq!(loaded.summary, "完成");
        assert_eq!(loaded.session_id.as_deref(), Some("sess-1"));
        assert!(loaded.finished_at.is_some());

        let all = store.list_runs(None, 10);
        assert_eq!(all.len(), 1);
        let per_task = store.list_runs(Some(&t.id), 10);
        assert_eq!(per_task.len(), 1);

        // Deleting the task cascades to its runs.
        store.delete_task(&t.id).unwrap();
        assert!(store.get_run(&run.id).is_none());
    }

    #[test]
    fn list_runs_respects_limit() {
        let store = store();
        let t = task();
        store.upsert_task(&t).unwrap();
        for _ in 0..3 {
            store
                .insert_run(&ScheduledRun {
                    id: ids::generate_id(),
                    task_id: t.id.clone(),
                    session_id: None,
                    status: RunStatus::Completed,
                    started_at: Utc::now(),
                    finished_at: None,
                    summary: String::new(),
                    error: String::new(),
                    worktree_path: String::new(),
                })
                .unwrap();
        }
        assert_eq!(store.list_runs(Some(&t.id), 2).len(), 2);
        assert_eq!(store.list_runs(None, 500).len(), 3);
    }
}
