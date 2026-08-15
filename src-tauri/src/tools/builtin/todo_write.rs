//! Todo write tool — structured TODO list management.
//!
//! Allows the agent to write, update, and track structured TODO items.
//! The todo list is persisted per session and displayed in the UI.

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tauri::Emitter;

/// The status of a todo item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}

// Removed manual Default impl — derived via #[derive(Default)]

/// A single TODO item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
    /// Parent todo id — a tree node's stage. Absent = a root item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Step ids this step must wait on: it may only leave `pending` once
    /// every listed id is `completed`. Empty/absent = no ordering constraint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depends_on: Option<Vec<String>>,
    /// Concrete command/check proving this step is done (test / lint /
    /// typecheck / run). A step is "done" only when this passes AND every
    /// `depends_on` step is completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verify: Option<String>,
}

/// A todo list update event emitted to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub struct TodoListEvent {
    pub session_id: String,
    pub todos: Vec<TodoItem>,
}

/// Per-session todo list store, shared with the frontend commands.
///
/// Persisted to the database (`sessions.todos`) so the task-progress list
/// survives app restarts: `get` lazily falls back to the database on a
/// memory miss (the frontend panel re-hydrates when a session opens), and
/// `set` writes through immediately. `None` db = memory-only (tests).
pub struct TodoStore {
    todos: Arc<RwLock<HashMap<String, Vec<TodoItem>>>>,
    db: Option<Arc<crate::storage::database::Database>>,
}

impl Default for TodoStore {
    fn default() -> Self {
        Self {
            todos: Arc::new(RwLock::new(HashMap::new())),
            db: None,
        }
    }
}

impl TodoStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach the database so todo lists persist across restarts.
    pub fn with_db(mut self, db: Arc<crate::storage::database::Database>) -> Self {
        self.db = Some(db);
        self
    }

    /// Replace the todo list for a session. Empty list clears it.
    pub fn set(&self, session_id: &str, todos: Vec<TodoItem>) {
        {
            let mut guard = self.todos.write().unwrap_or_else(|e| e.into_inner());
            if todos.is_empty() {
                guard.remove(session_id);
            } else {
                guard.insert(session_id.to_string(), todos.clone());
            }
        }
        if let Some(ref db) = self.db {
            let json_str = if todos.is_empty() {
                None
            } else {
                serde_json::to_string(&todos).ok()
            };
            if let Some(ref s) = json_str {
                if let Err(e) = db.set_session_todos(session_id, Some(s)) {
                    tracing::warn!(session_id, error = %e, "Failed to persist session todos");
                }
            } else if let Err(e) = db.set_session_todos(session_id, None) {
                tracing::warn!(session_id, error = %e, "Failed to clear session todos");
            }
        }
    }

    /// Get the todo list for a session.
    ///
    /// Memory first; on a miss the database is consulted once and the
    /// result cached (a list written before a restart re-hydrates here).
    pub fn get(&self, session_id: &str) -> Option<Vec<TodoItem>> {
        if let Some(todos) = self
            .todos
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(session_id)
            .cloned()
        {
            return Some(todos);
        }
        let db = self.db.as_ref()?;
        let raw = match db.get_session_todos(session_id) {
            Ok(raw) => raw,
            Err(e) => {
                tracing::warn!(session_id, error = %e, "Failed to load session todos");
                return None;
            }
        };
        let raw = raw?;
        match serde_json::from_str::<Vec<TodoItem>>(&raw) {
            Ok(todos) => {
                if todos.is_empty() {
                    return None;
                }
                let mut guard = self.todos.write().unwrap_or_else(|e| e.into_inner());
                guard.insert(session_id.to_string(), todos.clone());
                Some(todos)
            }
            Err(e) => {
                tracing::warn!(session_id, error = %e, "Session todos JSON malformed");
                None
            }
        }
    }
}

/// Whether a todo-sync nudge is warranted after a tool round.
///
/// Real work happened (file writes / task mutations) with NO `todo_write`
/// call in the same round, while unfinished todo items remain — the task
/// panel is stale. The nudge fires at most once per run; the check is pure
/// so the loop can unit-test the trigger.
pub fn todo_sync_needed(todos: &[TodoItem], tool_names: &[&str]) -> bool {
    let wrote = tool_names.iter().any(|n| {
        matches!(
            *n,
            "write_file" | "edit_file" | "search_replace" | "apply_patch" | "task_manage"
        )
    });
    if !wrote {
        return false;
    }
    if tool_names.contains(&"todo_write") {
        return false;
    }
    todos.iter().any(|t| t.status != TodoStatus::Completed)
}

/// Whether any started/finished step is ahead of an unfinished dependency.
///
/// A step whose `depends_on` lists other steps may only leave `pending` once
/// every listed id is `completed`. Returns a human-readable description of
/// the first violation (for the loop's ordering nudge), or `None` when the
/// plan's ordering is consistent. A `depends_on` id that names no known step
/// is also a violation — the plan lost a step during a rewrite.
pub fn todo_order_violation(todos: &[TodoItem]) -> Option<String> {
    let by_id: HashMap<&str, &TodoItem> = todos.iter().map(|t| (t.id.as_str(), t)).collect();
    for t in todos {
        if t.status == TodoStatus::Pending {
            continue;
        }
        let Some(deps) = t.depends_on.as_deref() else {
            continue;
        };
        let unmet: Vec<&str> = deps
            .iter()
            .filter(|d| {
                !by_id
                    .get(d.as_str())
                    .map(|dep| dep.status == TodoStatus::Completed)
                    .unwrap_or(false)
            })
            .map(|s| s.as_str())
            .collect();
        if !unmet.is_empty() {
            return Some(format!(
                "step \"{}\" is {} but its dependency \"{}\" is not completed — a step \
                 may only start after every `depends_on` step is done",
                t.id,
                todo_status_name(t.status),
                unmet.join("\", \""),
            ));
        }
    }
    None
}

fn todo_status_name(status: TodoStatus) -> &'static str {
    match status {
        TodoStatus::Pending => "pending",
        TodoStatus::InProgress => "in_progress",
        TodoStatus::Completed => "completed",
    }
}

/// Tool for writing structured TODO lists.
pub struct TodoWriteTool {
    store: Arc<TodoStore>,
}

impl TodoWriteTool {
    pub fn new(store: Arc<TodoStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todo_write"
    }

    fn description(&self) -> &str {
        "Write or update the structured TODO list. Each item has an id, \
        content, status (pending/in_progress/completed), and optional priority. \
        Use this ONLY for genuinely multi-step work — three or more distinct \
        actions, or work spanning several turns. Do NOT create a todo list for \
        simple tasks: a single edit, one question, a lone search, or anything \
        finishable in this turn never gets a todo list.\n\n\
        HIERARCHY: for large tasks, use parent items as PHASES and child items \
        (set the child's \"parent\" to the phase's id) as the steps within that \
        phase. A parent item is a phase name; its children are the concrete \
        steps. Keep the tree at most two levels.\n\n\
        DEPENDENCIES & VERIFICATION: for a long task (a game, a system, a \
        multi-file build) the ORDER matters. Set each step's \"depends_on\" to \
        the ids it must wait for — a step may only leave \"pending\" once every \
        dependency is \"completed\". Never start or finish a step ahead of its \
        dependencies. Give each step a \"verify\" — a concrete command or check \
        (test / lint / typecheck / run) that proves the step is done. Mark a \
        step \"completed\" only when its verify passes AND its dependencies are \
        completed. Build in VERTICAL SLICES: each phase must leave the project \
        runnable/verifiable — a plan whose phases cannot run in dependency order \
        is mis-planned. The list is shown LIVE in the app's right panel as the \
        user's progress UI — write each item's content as a clear user-facing \
        description and keep statuses current as you work."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The complete todo list (replaces existing list)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Unique identifier for this todo"
                            },
                            "content": {
                                "type": "string",
                                "description": "The task description"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Current status of the todo"
                            },
                            "priority": {
                                "type": "string",
                                "enum": ["low", "medium", "high"],
                                "description": "Optional priority level"
                            },
                            "parent": {
                                "type": "string",
                                "description": "Optional parent todo id — makes this a child step of that phase item. Omit for root/phase items."
                            },
                            "depends_on": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "Optional todo ids this step must wait for. May only leave 'pending' once all listed ids are 'completed'."
                            },
                            "verify": {
                                "type": "string",
                                "description": "Optional concrete command/check proving this step is done (test/lint/typecheck/run)."
                            }
                        },
                        "required": ["id", "content", "status"]
                    }
                }
            },
            "required": ["todos"]
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
        let todos_arr = args
            .get("todos")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                crate::core::error::AppError::ToolNotFound("missing 'todos' array".into())
            })?;

        // Collect declared ids first so a `parent` reference can be validated
        // against ids that actually exist in this write — a dangling parent
        // would persist dirty data that the frontend then has to paper over.
        let ids: std::collections::HashSet<String> = todos_arr
            .iter()
            .filter_map(|item| item.get("id").and_then(|v| v.as_str()))
            .map(String::from)
            .collect();

        let mut todos = Vec::new();
        for item in todos_arr {
            let id = item
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("unnamed")
                .to_string();
            let content = item
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let status_str = item
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending");
            let status = match status_str {
                "in_progress" => TodoStatus::InProgress,
                "completed" => TodoStatus::Completed,
                _ => TodoStatus::Pending,
            };
            let priority = item
                .get("priority")
                .and_then(|v| v.as_str())
                .map(String::from);
            let parent_id = item
                .get("parent")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .filter(|s| ids.contains(*s))
                .map(String::from);
            let depends_on = item
                .get("depends_on")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect::<Vec<String>>()
                })
                .filter(|v| !v.is_empty());
            let verify = item
                .get("verify")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            todos.push(TodoItem {
                id,
                content,
                status,
                priority,
                parent_id,
                depends_on,
                verify,
            });
        }

        let completed = todos
            .iter()
            .filter(|t| t.status == TodoStatus::Completed)
            .count();
        let in_progress = todos
            .iter()
            .filter(|t| t.status == TodoStatus::InProgress)
            .count();
        let pending = todos
            .iter()
            .filter(|t| t.status == TodoStatus::Pending)
            .count();

        // Persist for the session (survives restarts) + emit to frontend.
        self.store.set(&ctx.session_id, todos.clone());
        let _ = ctx.app.emit(
            "todo-list-updated",
            TodoListEvent {
                session_id: ctx.session_id.clone(),
                todos: todos.clone(),
            },
        );

        Ok(ToolResult::success(format!(
            "TODO list updated: {} completed, {} in progress, {} pending ({} total).",
            completed,
            in_progress,
            pending,
            todos.len()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn todo_status_default_is_pending() {
        assert_eq!(TodoStatus::default(), TodoStatus::Pending);
    }

    fn store_with_db() -> (TodoStore, Arc<crate::storage::database::Database>) {
        let dir =
            std::env::temp_dir().join(format!("ddc-todo-test-{}", crate::core::ids::generate_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db =
            Arc::new(crate::storage::database::Database::open(&dir.join("t.db"), false).unwrap());
        db.run_migrations().unwrap();
        (TodoStore::new().with_db(db.clone()), db)
    }

    fn session_row(db: &crate::storage::database::Database) -> String {
        let session = crate::core::types::Session::new("m", "p");
        db.upsert_session(&session).unwrap();
        session.id
    }

    fn item(id: &str, status: TodoStatus) -> TodoItem {
        TodoItem {
            id: id.to_string(),
            content: format!("task {id}"),
            status,
            priority: None,
            parent_id: None,
            depends_on: None,
            verify: None,
        }
    }

    #[test]
    fn todo_store_set_get_clear() {
        let store = TodoStore::new();
        assert!(store.get("s1").is_none());
        store.set("s1", vec![item("a", TodoStatus::Pending)]);
        assert_eq!(store.get("s1").unwrap().len(), 1);
        store.set("s1", vec![]);
        assert!(store.get("s1").is_none());
        assert!(store.get("s2").is_none());
    }

    #[test]
    fn todo_survives_store_recreation() {
        let (store, db) = store_with_db();
        let sid = session_row(&db);
        store.set(
            &sid,
            vec![
                item("a", TodoStatus::Completed),
                item("b", TodoStatus::Pending),
            ],
        );
        drop(store);

        let store = TodoStore::new().with_db(db);
        let restored = store.get(&sid).expect("todos re-hydrated");
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[0].status, TodoStatus::Completed);
        assert_eq!(restored[1].status, TodoStatus::Pending);
    }

    #[test]
    fn todo_clear_removes_from_database() {
        let (store, db) = store_with_db();
        let sid = session_row(&db);
        store.set(&sid, vec![item("a", TodoStatus::Pending)]);
        assert!(db.get_session_todos(&sid).unwrap().is_some());
        store.set(&sid, vec![]);
        assert!(db.get_session_todos(&sid).unwrap().is_none());
    }

    #[test]
    fn todo_store_without_db_stays_memory_only() {
        let store = TodoStore::new();
        store.set("s1", vec![item("a", TodoStatus::Pending)]);
        assert_eq!(store.get("s1").unwrap().len(), 1);
    }

    #[test]
    fn todo_sync_needed_after_real_work_without_update() {
        // Wrote files, no todo_write, unfinished items → nudge.
        assert!(todo_sync_needed(
            &[item("a", TodoStatus::InProgress)],
            &["write_file"]
        ));
        assert!(todo_sync_needed(
            &[
                item("a", TodoStatus::Pending),
                item("b", TodoStatus::Completed)
            ],
            &["edit_file", "grep"]
        ));
    }

    #[test]
    fn todo_sync_not_needed_when_list_is_fresh() {
        // todo_write was called this round — the model is in sync.
        assert!(!todo_sync_needed(
            &[item("a", TodoStatus::InProgress)],
            &["write_file", "todo_write"]
        ));
        // No write-side work this round.
        assert!(!todo_sync_needed(
            &[item("a", TodoStatus::InProgress)],
            &["grep", "read_file"]
        ));
        // Everything is done — nothing left to sync.
        assert!(!todo_sync_needed(
            &[item("a", TodoStatus::Completed)],
            &["write_file"]
        ));
        // No todo list at all.
        assert!(!todo_sync_needed(&[], &["write_file"]));
    }

    fn item_with_deps(id: &str, status: TodoStatus, depends_on: &[&str]) -> TodoItem {
        let mut t = item(id, status);
        t.depends_on = Some(depends_on.iter().map(|s| s.to_string()).collect());
        t
    }

    #[test]
    fn order_violation_detects_step_ahead_of_dependency() {
        let todos = vec![
            item_with_deps("a", TodoStatus::Completed, &[]),
            item_with_deps("b", TodoStatus::InProgress, &["a"]),
            item_with_deps("c", TodoStatus::Completed, &["a", "b"]),
        ];
        // c is completed while b is only in_progress — out of order.
        let v = todo_order_violation(&todos).expect("violation found");
        assert!(v.contains("\"c\""), "{v}");
        assert!(v.contains("\"b\""), "{v}");
    }

    #[test]
    fn order_violation_allows_dependency_first() {
        let todos = vec![
            item_with_deps("a", TodoStatus::Completed, &[]),
            item_with_deps("b", TodoStatus::Completed, &["a"]),
            item_with_deps("c", TodoStatus::InProgress, &["a", "b"]),
        ];
        assert!(todo_order_violation(&todos).is_none());
    }

    #[test]
    fn order_violation_ignores_pending_step() {
        // A pending step may reference a not-yet-done dependency — it hasn't
        // started, so there is no ordering breach.
        let todos = vec![
            item("a", TodoStatus::Pending),
            item_with_deps("b", TodoStatus::Pending, &["a"]),
        ];
        assert!(todo_order_violation(&todos).is_none());
    }

    #[test]
    fn order_violation_detects_dangling_dependency() {
        let todos = vec![item_with_deps("a", TodoStatus::InProgress, &["ghost"])];
        let v = todo_order_violation(&todos).expect("dangling dep is a violation");
        assert!(v.contains("ghost"), "{v}");
    }

    #[test]
    fn todo_item_roundtrips_depends_on_and_verify() {
        let t = TodoItem {
            id: "s2".to_string(),
            content: "collision".to_string(),
            status: TodoStatus::Pending,
            priority: None,
            parent_id: None,
            depends_on: Some(vec!["s1".to_string()]),
            verify: Some("cargo test".to_string()),
        };
        let json = serde_json::to_string(&t).unwrap();
        let back: TodoItem = serde_json::from_str(&json).unwrap();
        assert_eq!(back.depends_on.as_deref(), Some(&["s1".to_string()][..]));
        assert_eq!(back.verify.as_deref(), Some("cargo test"));
    }

    #[test]
    fn todo_item_without_new_fields_still_deserializes() {
        // Old persisted rows (pre depends_on/verify) must still load.
        let json = r#"{"id":"a","content":"x","status":"completed"}"#;
        let back: TodoItem = serde_json::from_str(json).unwrap();
        assert_eq!(back.depends_on, None);
        assert_eq!(back.verify, None);
    }
}
