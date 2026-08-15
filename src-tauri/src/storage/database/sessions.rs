use rusqlite::{params, OptionalExtension};
use std::sync::Arc;

use crate::core::error::AppResult;
use crate::core::types;

use super::helpers::{parse_dt, parse_session_status};
use super::Database;

impl Database {
    /// Insert or update a session record.
    ///
    /// UPSERT (`ON CONFLICT(id) DO UPDATE`) instead of `INSERT OR REPLACE`:
    /// REPLACE is a DELETE+INSERT under the hood, and `messages` /
    /// `rewind_points` / `memory` carry `ON DELETE CASCADE` — every session
    /// upsert (title change, model switch, idle eviction, turn persist)
    /// silently wiped the session's history (#85 audit H3, verified with
    /// SQLite). DO UPDATE touches only the session row.
    pub fn upsert_session(&self, session: &types::Session) -> AppResult<()> {
        let conn = self.conn.lock()?;
        conn.execute(
            r#"INSERT INTO sessions
               (id, title, model, provider, status, created_at, updated_at,
               workspace_path, system_prompt, turn_count,
               prompt_tokens, completion_tokens, cached_read_tokens, reasoning_tokens,
               prompt_cache_hit_tokens, prompt_cache_miss_tokens, total_cost,
               work_mode, context_window, permission_mode, pinned, last_message)
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
               ON CONFLICT(id) DO UPDATE SET
                 title = excluded.title,
                 model = excluded.model,
                 provider = excluded.provider,
                 context_window = excluded.context_window,
                 status = excluded.status,
                 updated_at = excluded.updated_at,
                 workspace_path = excluded.workspace_path,
                 system_prompt = excluded.system_prompt,
                 turn_count = excluded.turn_count,
                 prompt_tokens = excluded.prompt_tokens,
                 completion_tokens = excluded.completion_tokens,
                 cached_read_tokens = excluded.cached_read_tokens,
                 reasoning_tokens = excluded.reasoning_tokens,
                 prompt_cache_hit_tokens = excluded.prompt_cache_hit_tokens,
                 prompt_cache_miss_tokens = excluded.prompt_cache_miss_tokens,
                 total_cost = excluded.total_cost,
                 work_mode = excluded.work_mode,
                 permission_mode = excluded.permission_mode,
                 pinned = excluded.pinned,
                 last_message = excluded.last_message"#,
            params![
                session.id,
                session.title,
                session.model,
                session.provider,
                format!("{:?}", session.status).to_lowercase(),
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
                session.workspace_path,
                session.system_prompt,
                session.turn_count as i64,
                session.total_usage.prompt_tokens as i64,
                session.total_usage.completion_tokens as i64,
                session.total_usage.cached_read_tokens.map(|v| v as i64),
                session.total_usage.reasoning_tokens.map(|v| v as i64),
                // deepseek-native: KV cache tokens
                session.total_usage.prompt_cache_hit_tokens.map(|v| v as i64),
                session.total_usage.prompt_cache_miss_tokens.map(|v| v as i64),
                // Estimated session cost from the cumulative token usage.
                // Pricing is the model-class heuristic from agent::budget
                // (a cost guard, not a billing ledger — real rates vary by
                // provider plan).
                crate::core::types::TokenPricing::for_model(&session.model)
                    .cost(&session.total_usage),
                session.work_mode,
                session.context_window as i64,
                session.permission_mode,
                session.pinned as i64,
                session.last_message,
            ],
        )?;
        Ok(())
    }

    /// Load a session by ID.
    pub fn get_session(&self, id: &str) -> AppResult<Option<types::Session>> {
        let conn = self.conn.lock()?;
        let row = conn
            .query_row("SELECT * FROM sessions WHERE id = ?1", params![id], |row| {
                Ok(types::Session {
                    id: row.get("id")?,
                    title: row.get("title")?,
                    model: row.get("model")?,
                    provider: row.get("provider")?,
                    context_window: row.get::<_, i64>("context_window")? as u64,
                    status: parse_session_status(row.get::<_, String>("status")?),
                    created_at: parse_dt(row.get("created_at")?),
                    updated_at: parse_dt(row.get("updated_at")?),
                    workspace_path: row.get("workspace_path")?,
                    total_usage: types::TokenUsage {
                        prompt_tokens: row.get::<_, i64>("prompt_tokens")? as u64,
                        completion_tokens: row.get::<_, i64>("completion_tokens")? as u64,
                        cached_read_tokens: row
                            .get::<_, Option<i64>>("cached_read_tokens")?
                            .map(|v| v as u64),
                        reasoning_tokens: row
                            .get::<_, Option<i64>>("reasoning_tokens")?
                            .map(|v| v as u64),
                        prompt_cache_hit_tokens: row
                            .get::<_, Option<i64>>("prompt_cache_hit_tokens")?
                            .map(|v| v as u64),
                        prompt_cache_miss_tokens: row
                            .get::<_, Option<i64>>("prompt_cache_miss_tokens")?
                            .map(|v| v as u64),
                    },
                    turn_count: row.get::<_, i64>("turn_count")? as u64,
                    system_prompt: row.get("system_prompt")?,
                    work_mode: row.get("work_mode")?,
                    permission_mode: row
                        .get::<_, Option<String>>("permission_mode")?
                        .unwrap_or_default(),
                    pinned: row.get::<_, i64>("pinned")? != 0,
                    last_message: row.get::<_, String>("last_message")?,
                    is_streaming: false,
                })
            })
            .optional()?;
        Ok(row)
    }

    /// List all sessions, ordered by most recently updated.
    pub fn list_sessions(&self, limit: u32) -> AppResult<Vec<types::Session>> {
        let conn = self.conn.lock()?;
        let mut stmt = conn.prepare("SELECT * FROM sessions ORDER BY pinned DESC, updated_at DESC LIMIT ?1")?;
        let sessions = stmt
            .query_map(params![limit as i64], |row| {
                Ok(types::Session {
                    id: row.get("id")?,
                    title: row.get("title")?,
                    model: row.get("model")?,
                    provider: row.get("provider")?,
                    context_window: row.get::<_, i64>("context_window")? as u64,
                    status: parse_session_status(row.get::<_, String>("status")?),
                    created_at: parse_dt(row.get("created_at")?),
                    updated_at: parse_dt(row.get("updated_at")?),
                    workspace_path: row.get("workspace_path")?,
                    total_usage: types::TokenUsage {
                        prompt_tokens: row.get::<_, i64>("prompt_tokens")? as u64,
                        completion_tokens: row.get::<_, i64>("completion_tokens")? as u64,
                        cached_read_tokens: row
                            .get::<_, Option<i64>>("cached_read_tokens")?
                            .map(|v| v as u64),
                        reasoning_tokens: row
                            .get::<_, Option<i64>>("reasoning_tokens")?
                            .map(|v| v as u64),
                        prompt_cache_hit_tokens: row
                            .get::<_, Option<i64>>("prompt_cache_hit_tokens")?
                            .map(|v| v as u64),
                        prompt_cache_miss_tokens: row
                            .get::<_, Option<i64>>("prompt_cache_miss_tokens")?
                            .map(|v| v as u64),
                    },
                    turn_count: row.get::<_, i64>("turn_count")? as u64,
                    system_prompt: row.get("system_prompt")?,
                    work_mode: row.get("work_mode")?,
                    permission_mode: row
                        .get::<_, Option<String>>("permission_mode")?
                        .unwrap_or_default(),
                    pinned: row.get::<_, i64>("pinned")? != 0,
                    last_message: row.get::<_, String>("last_message")?,
                    is_streaming: false,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sessions)
    }

    /// Async wrapper around [`Self::list_sessions`] — a large-history scan
    /// (sync push of 1000 rows) must not block a tokio worker.
    pub async fn list_sessions_async(
        self: &Arc<Self>,
        limit: u32,
    ) -> AppResult<Vec<types::Session>> {
        let db = self.clone();
        tokio::task::spawn_blocking(move || db.list_sessions(limit))
            .await
            .map_err(|e| {
                crate::core::error::AppError::Internal(format!("list_sessions task failed: {e}"))
            })?
    }

    /// Delete a session and all its messages.
    pub fn delete_session(&self, id: &str) -> AppResult<()> {
        let conn = self.conn.lock()?;
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])?;
        // Messages are cascade-deleted via FK
        Ok(())
    }

    /// Persist the session's goal (task intent). Empty string clears it.
    /// The goal survives restarts — re-injected as `<current-goal>`.
    pub fn set_session_goal(&self, id: &str, goal: &str) -> AppResult<()> {
        let conn = self.conn.lock()?;
        let goal = if goal.trim().is_empty() {
            None
        } else {
            Some(goal.to_string())
        };
        let updated = conn.execute(
            "UPDATE sessions SET goal = ?1 WHERE id = ?2",
            params![goal, id],
        )?;
        if updated == 0 {
            // A goal always attaches to an existing session (update_goal
            // runs inside a session); a miss means the session was deleted.
            tracing::warn!(session_id = id, "set_session_goal: session not found");
        }
        Ok(())
    }

    /// Load the persisted goal for a session.
    pub fn get_session_goal(&self, id: &str) -> AppResult<Option<String>> {
        let conn = self.conn.lock()?;
        let goal = conn
            .query_row(
                "SELECT goal FROM sessions WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(goal)
    }

    /// Persist the session's todo list (JSON array of `TodoItem`).
    /// `None`/empty clears it. The list survives restarts so the frontend
    /// task-progress panel can be restored.
    pub fn set_session_todos(&self, id: &str, todos: Option<&str>) -> AppResult<()> {
        let conn = self.conn.lock()?;
        let todos = todos
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "[]")
            .map(str::to_string);
        let updated = conn.execute(
            "UPDATE sessions SET todos = ?1 WHERE id = ?2",
            params![todos, id],
        )?;
        if updated == 0 {
            tracing::warn!(session_id = id, "set_session_todos: session not found");
        }
        Ok(())
    }

    /// Load the persisted todo list JSON for a session.
    pub fn get_session_todos(&self, id: &str) -> AppResult<Option<String>> {
        let conn = self.conn.lock()?;
        let todos = conn
            .query_row(
                "SELECT todos FROM sessions WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(todos)
    }

    /// Persist the session's approved plan steps (JSON array of `PlanStep`).
    /// `None`/empty clears it. The plan gate re-reads this after a restart,
    /// so a session resumed mid-plan still checks the approved checklist.
    pub fn set_session_plan_steps(&self, id: &str, steps: Option<&str>) -> AppResult<()> {
        let conn = self.conn.lock()?;
        let steps = steps
            .map(str::trim)
            .filter(|s| !s.is_empty() && *s != "[]")
            .map(str::to_string);
        let updated = conn.execute(
            "UPDATE sessions SET plan_steps = ?1 WHERE id = ?2",
            params![steps, id],
        )?;
        if updated == 0 {
            tracing::warn!(session_id = id, "set_session_plan_steps: session not found");
        }
        Ok(())
    }

    /// Load the persisted plan steps JSON for a session.
    pub fn get_session_plan_steps(&self, id: &str) -> AppResult<Option<String>> {
        let conn = self.conn.lock()?;
        let steps = conn
            .query_row(
                "SELECT plan_steps FROM sessions WHERE id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        Ok(steps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{Session, SessionStatus};

    fn test_db() -> std::sync::Arc<Database> {
        let dir = std::env::temp_dir().join(format!(
            "ddc-sessions-test-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db = Database::open(&dir.join("test.db"), true).unwrap();
        db.run_migrations().unwrap();
        std::sync::Arc::new(db)
    }

    fn session_with(id: &str, work_mode: &str) -> Session {
        let mut s = Session::new("model", "provider");
        s.id = id.to_string();
        s.work_mode = work_mode.to_string();
        s
    }

    #[test]
    fn upsert_and_load_roundtrips_work_mode() {
        let db = test_db();
        db.upsert_session(&session_with("s-depwork", "depwork"))
            .unwrap();
        db.upsert_session(&session_with("s-code", "code")).unwrap();

        let loaded = db.get_session("s-depwork").unwrap().expect("exists");
        assert_eq!(loaded.work_mode, "depwork");
        let loaded = db.get_session("s-code").unwrap().expect("exists");
        assert_eq!(loaded.work_mode, "code");
        assert_eq!(loaded.status, SessionStatus::Active);

        let all = db.list_sessions(10).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all
            .iter()
            .any(|s| s.id == "s-depwork" && s.work_mode == "depwork"));
    }

    #[test]
    fn upsert_and_load_roundtrips_pinned_and_last_message() {
        let db = test_db();
        let mut s = session_with("s-pin", "code");
        s.pinned = true;
        s.last_message = "最后一条消息".to_string();
        db.upsert_session(&s).unwrap();

        let loaded = db.get_session("s-pin").unwrap().expect("exists");
        assert!(loaded.pinned);
        assert_eq!(loaded.last_message, "最后一条消息");
        // New sessions start unpinned with an empty preview.
        assert!(!session_with("s-new", "code").pinned);
        assert!(session_with("s-new", "code").last_message.is_empty());
    }

    #[test]
    fn list_sessions_orders_pinned_first() {
        let db = test_db();
        let mut pinned = session_with("s-pin", "code");
        pinned.pinned = true;
        pinned.updated_at = chrono::Utc::now() - chrono::Duration::days(2);
        let fresh = session_with("s-fresh", "code"); // unpinned, newer
        db.upsert_session(&pinned).unwrap();
        db.upsert_session(&fresh).unwrap();

        let all = db.list_sessions(10).unwrap();
        let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids[0], "s-pin", "pinned session must lead the list");
        assert_eq!(ids[1], "s-fresh");
    }

    #[test]
    fn old_schema_upgrade_adds_work_mode_default() {
        // Simulate a version-9 database: sessions table WITHOUT work_mode,
        // one pre-existing row. run_migrations must add the column with the
        // 'code' default so reads succeed for old data.
        let dir = std::env::temp_dir().join(format!(
            "ddc-sessions-mig-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                // Migration-1 schema exactly: no KV-cache columns, no
                // total_cost, no work_mode — everything later migrations
                // add must be applied on upgrade.
                r#"
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL DEFAULT 'New Session',
                    model TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'active',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    workspace_path TEXT,
                    system_prompt TEXT NOT NULL DEFAULT '',
                    turn_count INTEGER NOT NULL DEFAULT 0,
                    prompt_tokens INTEGER NOT NULL DEFAULT 0,
                    completion_tokens INTEGER NOT NULL DEFAULT 0,
                    cached_read_tokens INTEGER,
                    reasoning_tokens INTEGER
                );
                INSERT INTO sessions (id, title, model, provider, status, created_at, updated_at)
                VALUES ('old-1', 'Old Session', 'm', 'p', 'active', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
                "#,
            )
            .unwrap();
        }

        let db = Database::open(&path, true).unwrap();
        db.run_migrations().unwrap();

        let loaded = db.get_session("old-1").unwrap().expect("exists");
        assert_eq!(loaded.work_mode, "code", "old sessions default to code");
        assert_eq!(loaded.title, "Old Session");
    }

    #[test]
    fn goal_persists_across_database_reopen() {
        let dir = std::env::temp_dir().join(format!(
            "ddc-sessions-goal-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("goal.db");

        {
            let db = Database::open(&path, true).unwrap();
            db.run_migrations().unwrap();
            db.upsert_session(&session_with("s1", "code")).unwrap();
            db.set_session_goal("s1", "refactor auth module").unwrap();
        }

        // Reopen — the goal must survive the process boundary.
        let db = Database::open(&path, true).unwrap();
        db.run_migrations().unwrap();
        assert_eq!(
            db.get_session_goal("s1").unwrap(),
            Some("refactor auth module".to_string())
        );

        // Empty string clears; unknown session → None.
        db.set_session_goal("s1", "").unwrap();
        assert_eq!(db.get_session_goal("s1").unwrap(), None);
        assert_eq!(db.get_session_goal("ghost").unwrap(), None);
    }

    #[test]
    fn old_schema_upgrade_adds_goal_column() {
        // A pre-goal database (migration-1 schema shape) must still upgrade
        // — run_migrations applies every pending migration including the
        // goal column.
        let dir = std::env::temp_dir().join(format!(
            "ddc-sessions-goal-mig-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old-goal.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL DEFAULT 'New Session',
                    model TEXT NOT NULL,
                    provider TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'active',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL,
                    workspace_path TEXT,
                    system_prompt TEXT NOT NULL DEFAULT '',
                    turn_count INTEGER NOT NULL DEFAULT 0,
                    prompt_tokens INTEGER NOT NULL DEFAULT 0,
                    completion_tokens INTEGER NOT NULL DEFAULT 0,
                    cached_read_tokens INTEGER,
                    reasoning_tokens INTEGER
                );
                INSERT INTO sessions (id, title, model, provider, status, created_at, updated_at)
                VALUES ('old-1', 'Old', 'm', 'p', 'active', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
                "#,
            )
            .unwrap();
        }

        let db = Database::open(&path, true).unwrap();
        db.run_migrations().unwrap();
        assert_eq!(db.get_session_goal("old-1").unwrap(), None);
        db.set_session_goal("old-1", "migrated goal").unwrap();
        assert_eq!(
            db.get_session_goal("old-1").unwrap(),
            Some("migrated goal".to_string())
        );
    }

    #[test]
    fn plan_steps_persist_across_database_reopen() {
        // The approved-plan checklist must survive a restart so the plan
        // gate can still nudge a resumed session through its steps.
        let dir = std::env::temp_dir().join(format!(
            "ddc-sessions-plan-{}",
            crate::core::ids::generate_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plan.db");

        {
            let db = Database::open(&path, true).unwrap();
            db.run_migrations().unwrap();
            db.upsert_session(&session_with("s1", "code")).unwrap();
            db.set_session_plan_steps(
                "s1",
                Some(r#"[{"id":"step-1","text":"read the module","done":false}]"#),
            )
            .unwrap();
        }

        // Reopen — the plan steps must survive the process boundary.
        let db = Database::open(&path, true).unwrap();
        db.run_migrations().unwrap();
        assert_eq!(
            db.get_session_plan_steps("s1").unwrap(),
            Some(r#"[{"id":"step-1","text":"read the module","done":false}]"#.to_string())
        );

        // Clearing with None removes; empty JSON also clears; unknown
        // session → None.
        db.set_session_plan_steps("s1", None).unwrap();
        assert_eq!(db.get_session_plan_steps("s1").unwrap(), None);
        db.set_session_plan_steps("s1", Some("[]")).unwrap();
        assert_eq!(db.get_session_plan_steps("s1").unwrap(), None);
        assert_eq!(db.get_session_plan_steps("ghost").unwrap(), None);
    }

    #[test]
    fn upsert_does_not_cascade_delete_messages() {
        // Regression for #85 H3: `INSERT OR REPLACE` deletes the session row
        // first, and messages/rewind_points carry `ON DELETE CASCADE` — an
        // ordinary session upsert (title change, model switch, eviction)
        // wiped the whole conversation history. The UPSERT rewrite must
        // leave child rows untouched.
        let db = test_db();
        db.upsert_session(&session_with("s1", "code")).unwrap();

        // Raw inserts need the connection guard — scope it so the guard is
        // released before the upsert below re-acquires the same mutex
        // (std Mutex is not reentrant: holding the guard across the upsert
        // call deadlocks).
        {
            let conn = db.conn().unwrap();
            conn.execute(
                "INSERT INTO messages (session_id, role, content, created_at) VALUES ('s1', 'user', 'hello', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO rewind_points (session_id, turn_index, created_at) \
                 VALUES ('s1', 0, '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        }

        // A second upsert of the SAME session — previously a REPLACE.
        let mut updated = session_with("s1", "code");
        updated.title = "Renamed".to_string();
        updated.turn_count = 3;
        db.upsert_session(&updated).unwrap();

        let message_count: i64 = db
            .conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let rewind_count: i64 = db
            .conn()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM rewind_points WHERE session_id = 's1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(message_count, 1, "messages must survive session upsert");
        assert_eq!(rewind_count, 1, "rewind points must survive session upsert");

        // And the upsert itself still applied the new fields.
        let loaded = db.get_session("s1").unwrap().expect("exists");
        assert_eq!(loaded.title, "Renamed");
        assert_eq!(loaded.turn_count, 3);
    }
}
