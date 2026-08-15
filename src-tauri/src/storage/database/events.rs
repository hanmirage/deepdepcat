//! Replay-exact agent event log — append-only audit records.
//!
//! Every model call, tool run, permission decision, and file edit appends
//! one row. `seq` is monotonic per session, so `replay_turn` rebuilds the
//! exact call order of a turn after a crash (the Meta Muse Code "local
//! event log" pattern). Writes are best-effort at the call site — the log
//! must never break the agent loop.

use crate::core::error::AppResult;
use crate::storage::database::Database;
use rusqlite::params;
use serde_json::Value;

/// One persisted agent event.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentEvent {
    pub id: i64,
    pub session_id: String,
    #[serde(default)]
    pub turn_id: Option<String>,
    pub seq: i64,
    pub kind: String,
    pub payload: Value,
    pub created_at: String,
}

/// Append one event. `seq` is derived inside the same locked connection as
/// the insert, so concurrent appends cannot duplicate or reorder a session.
pub fn append_event(
    db: &Database,
    session_id: &str,
    turn_id: Option<&str>,
    kind: &str,
    payload: Value,
) -> AppResult<()> {
    let conn = db.conn()?;
    let seq: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM agent_events WHERE session_id = ?1",
        params![session_id],
        |row| row.get(0),
    )?;
    let now = chrono::Utc::now();
    conn.execute(
        "INSERT INTO agent_events
             (session_id, turn_id, seq, kind, payload, created_at, created_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            session_id,
            turn_id,
            seq,
            kind,
            payload.to_string(),
            now.to_rfc3339(),
            now.timestamp_millis()
        ],
    )?;
    Ok(())
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentEvent> {
    let payload: String = row.get(5)?;
    Ok(AgentEvent {
        id: row.get(0)?,
        session_id: row.get(1)?,
        turn_id: row.get(2)?,
        seq: row.get(3)?,
        kind: row.get(4)?,
        payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
        created_at: row.get(6)?,
    })
}

/// List the most recent events of a session, newest first.
pub fn list_events(db: &Database, session_id: &str, limit: usize) -> AppResult<Vec<AgentEvent>> {
    let conn = db.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, session_id, turn_id, seq, kind, payload, created_at
         FROM agent_events
         WHERE session_id = ?1
         ORDER BY seq DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![session_id, limit as i64], row_to_event)?;
    let mut events = Vec::new();
    for row in rows {
        events.push(row?);
    }
    Ok(events)
}

/// Replay one turn's events in exact execution order (seq ascending) —
/// the audit path rebuilds what actually happened after a crash.
pub fn list_turn_events(
    db: &Database,
    session_id: &str,
    turn_id: &str,
) -> AppResult<Vec<AgentEvent>> {
    let conn = db.conn()?;
    let mut stmt = conn.prepare(
        "SELECT id, session_id, turn_id, seq, kind, payload, created_at
         FROM agent_events
         WHERE session_id = ?1 AND turn_id = ?2
         ORDER BY seq ASC",
    )?;
    let rows = stmt.query_map(params![session_id, turn_id], row_to_event)?;
    let mut events = Vec::new();
    for row in rows {
        events.push(row?);
    }
    Ok(events)
}

/// Delete events older than `keep_days` — bounded retention, returns the
/// number of rows removed.
pub fn prune_events(db: &Database, keep_days: u32) -> AppResult<usize> {
    let cutoff_ms = chrono::Utc::now().timestamp_millis() - i64::from(keep_days) * 86_400_000;
    let conn = db.conn()?;
    let deleted = conn.execute(
        "DELETE FROM agent_events WHERE created_ms < ?1",
        params![cutoff_ms],
    )?;
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_db() -> Database {
        let dir = std::env::temp_dir().join(format!(
            "ddc-events-test-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("t.db"), false).unwrap();
        db.run_migrations().unwrap();
        db
    }

    fn seed_session(db: &Database, session_id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = db.conn().unwrap();
        conn.execute(
            "INSERT INTO sessions
                 (id, title, model, provider, status, created_at, updated_at,
                  system_prompt, turn_count, prompt_tokens, completion_tokens)
             VALUES (?1, 'test', 'deepseek', 'deepseek', 'active', ?2, ?2, '', 0, 0, 0)",
            params![session_id, now],
        )
        .unwrap();
    }

    #[test]
    fn append_assigns_monotonic_seq_per_session() {
        let db = test_db();
        seed_session(&db, "s1");
        seed_session(&db, "s2");

        append_event(&db, "s1", Some("t1"), "model_call", json!({"model": "m"})).unwrap();
        append_event(&db, "s1", Some("t1"), "tool_run", json!({"tool": "grep"})).unwrap();
        append_event(&db, "s2", Some("t2"), "model_call", json!({"model": "m"})).unwrap();

        let s1 = list_events(&db, "s1", 10).unwrap();
        let s2 = list_events(&db, "s2", 10).unwrap();
        assert_eq!(s1.len(), 2);
        assert_eq!(s1[0].seq, 2, "newest first, per-session seq");
        assert_eq!(s1[1].seq, 1);
        assert_eq!(s2[0].seq, 1, "seq is per-session, not global");
    }

    #[test]
    fn replay_turn_returns_exact_execution_order() {
        let db = test_db();
        seed_session(&db, "s1");
        append_event(&db, "s1", Some("t1"), "model_call", json!({"n": 1})).unwrap();
        append_event(&db, "s1", Some("t1"), "tool_run", json!({"n": 2})).unwrap();
        append_event(&db, "s1", Some("t1"), "edit", json!({"n": 3})).unwrap();
        append_event(&db, "s1", Some("t2"), "model_call", json!({"n": 4})).unwrap();

        let replay = list_turn_events(&db, "s1", "t1").unwrap();
        assert_eq!(replay.len(), 3);
        assert_eq!(replay[0].kind, "model_call");
        assert_eq!(replay[1].kind, "tool_run");
        assert_eq!(replay[2].kind, "edit");
        assert_eq!(replay[2].payload["n"], 3);
    }

    #[test]
    fn prune_removes_only_old_rows() {
        let db = test_db();
        seed_session(&db, "s1");
        append_event(&db, "s1", Some("t1"), "model_call", json!({})).unwrap();

        // Backdate the row beyond the retention window.
        let old_ms = chrono::Utc::now().timestamp_millis() - 10 * 86_400_000;
        db.conn()
            .unwrap()
            .execute("UPDATE agent_events SET created_ms = ?1", params![old_ms])
            .unwrap();

        assert_eq!(prune_events(&db, 7).unwrap(), 1);
        assert!(list_events(&db, "s1", 10).unwrap().is_empty());
    }
}
