//! update_goal tool — declares or updates the session's current goal.
//!
//! The goal is a short, authoritative statement of what the user wants to
//! accomplish in this session. It is stored per session, shown in the UI
//! ("当前目标" capsule) and injected into every request so long-running
//! tasks stay on target even when the conversation drifts.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tauri::Emitter;

/// Per-session goal store, shared with the frontend commands.
///
/// Persisted to the database (`sessions.goal`) so the task intent survives
/// app restarts: `get` lazily falls back to the database on a memory miss
/// (the first turn after a restart re-hydrates the goal and re-injects it
/// as `<current-goal>`), and `set` writes through immediately.
pub struct GoalStore {
    goals: Arc<RwLock<HashMap<String, String>>>,
    /// Database for persistence — `None` in tests / when unavailable
    /// (falls back to memory-only behavior).
    db: Option<Arc<crate::storage::database::Database>>,
}

impl Default for GoalStore {
    fn default() -> Self {
        Self {
            goals: Arc::new(RwLock::new(HashMap::new())),
            db: None,
        }
    }
}

impl GoalStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the database so goals persist across restarts.
    pub fn with_db(mut self, db: Arc<crate::storage::database::Database>) -> Self {
        self.db = Some(db);
        self
    }

    /// Set the goal for a session. Empty string clears it.
    pub fn set(&self, session_id: &str, goal: String) {
        {
            let mut goals = self.goals.write().unwrap_or_else(|e| e.into_inner());
            if goal.trim().is_empty() {
                goals.remove(session_id);
            } else {
                goals.insert(session_id.to_string(), goal.clone());
            }
        }
        if let Some(ref db) = self.db {
            if let Err(e) = db.set_session_goal(session_id, &goal) {
                tracing::warn!(session_id, error = %e, "Failed to persist session goal");
            }
        }
    }

    /// Get the goal for a session.
    ///
    /// Memory first; on a miss the database is consulted once and the
    /// result cached (a goal set before a restart re-hydrates here).
    pub fn get(&self, session_id: &str) -> Option<String> {
        if let Some(goal) = self
            .goals
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
        {
            return Some(goal);
        }
        let db = self.db.as_ref()?;
        match db.get_session_goal(session_id) {
            Ok(Some(goal)) => {
                let mut goals = self.goals.write().unwrap_or_else(|e| e.into_inner());
                goals.insert(session_id.to_string(), goal.clone());
                Some(goal)
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(session_id, error = %e, "Failed to load session goal");
                None
            }
        }
    }
}

/// Goal-updated event emitted to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalEvent {
    pub session_id: String,
    pub goal: Option<String>,
}

/// Tool for declaring the session goal.
pub struct UpdateGoalTool {
    store: Arc<GoalStore>,
}

impl UpdateGoalTool {
    pub fn new(store: Arc<GoalStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for UpdateGoalTool {
    fn name(&self) -> &str {
        "update_goal"
    }

    fn description(&self) -> &str {
        "Declare or update the current session goal. The goal is a short \
         authoritative statement of what the user wants to accomplish. \
         Call this when the user states a new objective, or when the task \
         scope changes. Keep it concise (one sentence if possible). \
         Pass an empty string to clear the goal."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "The session goal. Concise, actionable, and self-contained."
                },
                "reason": {
                    "type": "string",
                    "description": "Optional one-line explanation of why the goal changed."
                }
            },
            "required": ["goal"]
        })
    }

    fn is_read_only(&self) -> bool {
        false
    }

    /// Side-effecting — never run in parallel with other tools.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> AppResult<ToolResult> {
        let goal = args
            .get("goal")
            .and_then(|g| g.as_str())
            .ok_or_else(|| AppError::Parse("Missing 'goal'".into()))?
            .trim()
            .to_string();

        let reason = args
            .get("reason")
            .and_then(|r| r.as_str())
            .filter(|r| !r.is_empty())
            .map(|r| r.trim().to_string());

        let cleared = goal.is_empty();
        self.store.set(&ctx.session_id, goal.clone());

        let _ = ctx.app.emit(
            "goal-updated",
            GoalEvent {
                session_id: ctx.session_id.clone(),
                goal: if cleared { None } else { Some(goal.clone()) },
            },
        );

        if cleared {
            Ok(ToolResult::success("Session goal cleared."))
        } else {
            let mut msg = format!("Session goal set: {goal}");
            if let Some(ref r) = reason {
                msg.push_str(&format!("\nReason: {r}"));
            }
            Ok(ToolResult::success(msg))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_store_get_set_clear() {
        let store = GoalStore::new();
        assert!(store.get("s1").is_none());
        store.set("s1", "Fix the build".to_string());
        assert_eq!(store.get("s1").as_deref(), Some("Fix the build"));
        // Empty string clears.
        store.set("s1", "  ".to_string());
        assert!(store.get("s1").is_none());
        // Sessions are isolated.
        assert!(store.get("s2").is_none());
    }

    fn store_with_db() -> (GoalStore, Arc<crate::storage::database::Database>) {
        let dir =
            std::env::temp_dir().join(format!("ddc-goal-test-{}", crate::core::ids::generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db =
            Arc::new(crate::storage::database::Database::open(&dir.join("t.db"), false).unwrap());
        db.run_migrations().unwrap();
        (GoalStore::new().with_db(db.clone()), db)
    }

    #[test]
    fn goal_survives_store_recreation() {
        // Simulates an app restart: a brand-new GoalStore over the same
        // database must re-hydrate the goal from disk on the first get.
        let (store, db) = store_with_db();
        // Goals attach to sessions — the real flow always has the session
        // row (update_goal runs inside a session), so create it first.
        let session = crate::core::types::Session::new("m", "p");
        db.upsert_session(&session).unwrap();
        let sid = session.id.clone();
        store.set(&sid, "Refactor auth module".to_string());
        drop(store);

        let store = GoalStore::new().with_db(db);
        assert_eq!(store.get(&sid).as_deref(), Some("Refactor auth module"));
    }

    #[test]
    fn goal_clear_removes_from_database() {
        let (store, db) = store_with_db();
        store.set("s1", "Do the thing".to_string());
        store.set("s1", "  ".to_string());
        assert_eq!(db.get_session_goal("s1").unwrap(), None);
    }

    #[test]
    fn goal_store_without_db_stays_memory_only() {
        let store = GoalStore::new();
        store.set("s1", "memory only".to_string());
        assert_eq!(store.get("s1").as_deref(), Some("memory only"));
    }

    #[test]
    fn tool_name() {
        let tool = UpdateGoalTool::new(Arc::new(GoalStore::new()));
        assert_eq!(tool.name(), "update_goal");
    }

    #[test]
    fn tool_not_read_only() {
        let tool = UpdateGoalTool::new(Arc::new(GoalStore::new()));
        assert!(!tool.is_read_only());
    }
}
