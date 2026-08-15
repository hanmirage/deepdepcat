//! A2A task persistence — tasks survive app restarts.
//!
//! The full `Task` JSON is stored per row; `tasks/get` stays correct after
//! a restart. Sessions are intentionally NOT foreign-keyed: closing an ACP
//! session must not cascade-delete an orchestration record.

use crate::core::error::AppResult;
use crate::storage::database::Database;
use chrono::Utc;
use rusqlite::params;
use std::sync::Arc;

/// A persisted A2A task row.
#[derive(Debug, Clone)]
pub struct A2aTaskRow {
    pub session_id: Option<String>,
    pub task_json: String,
}

/// Upsert a task row (INSERT OR REPLACE by id).
pub fn upsert_task(
    db: &Arc<Database>,
    id: &str,
    session_id: Option<&str>,
    task_json: &str,
) -> AppResult<()> {
    let conn = db.conn()?;
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO a2a_tasks (id, session_id, task_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET
             session_id = excluded.session_id,
             task_json = excluded.task_json,
             updated_at = excluded.updated_at",
        params![id, session_id, task_json, now, now],
    )?;
    Ok(())
}

/// Load every persisted task (newest updated first).
pub fn load_tasks(db: &Arc<Database>) -> Vec<A2aTaskRow> {
    let Ok(conn) = db.conn() else {
        return Vec::new();
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT session_id, task_json
         FROM a2a_tasks ORDER BY updated_at DESC",
    ) else {
        return Vec::new();
    };
    stmt.query_map([], |row| {
        Ok(A2aTaskRow {
            session_id: row.get(0)?,
            task_json: row.get(1)?,
        })
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

/// Remove a task row (client-initiated cleanup; sessions untouched).
pub fn delete_task(db: &Arc<Database>, id: &str) -> AppResult<()> {
    let conn = db.conn()?;
    conn.execute("DELETE FROM a2a_tasks WHERE id = ?1", params![id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Arc<Database> {
        let dir = std::env::temp_dir().join(format!(
            "ddc-a2a-store-test-{}",
            uuid::Uuid::new_v4()
        ));
        let db = Arc::new(Database::open(&dir.join("t.db"), false).unwrap());
        db.run_migrations().unwrap();
        db
    }

    #[test]
    fn task_roundtrip_survives_reload() {
        let db = db();
        upsert_task(&db, "a2a-1", Some("sess-1"), r#"{"id":"a2a-1","status":{"state":"completed"}}"#)
            .unwrap();
        upsert_task(&db, "a2a-2", None, r#"{"id":"a2a-2","status":{"state":"working"}}"#).unwrap();

        let rows = load_tasks(&db);
        assert_eq!(rows.len(), 2);
        let r1 = rows.iter().find(|r| r.task_json.contains("a2a-1")).unwrap();
        assert_eq!(r1.session_id.as_deref(), Some("sess-1"));
        assert!(r1.task_json.contains("completed"));

        // Update an existing row — replace, not duplicate.
        upsert_task(&db, "a2a-1", Some("sess-1"), r#"{"id":"a2a-1","status":{"state":"failed"}}"#)
            .unwrap();
        assert_eq!(load_tasks(&db).len(), 2);
        assert!(load_tasks(&db)
            .iter()
            .find(|r| r.task_json.contains("a2a-1"))
            .unwrap()
            .task_json
            .contains("failed"));

        delete_task(&db, "a2a-2").unwrap();
        assert_eq!(load_tasks(&db).len(), 1);
    }
}
