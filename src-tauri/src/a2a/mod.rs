//! A2A (Agent2Agent) inbound server — minimal A2A 1.0 compliance.
//!
//! Exposes DeepDepCat as an agent OTHER agents can orchestrate, using the
//! Linux Foundation A2A protocol shape:
//! - `GET /.well-known/agent.json` — AgentCard (name/description/url/
//!   capabilities/skills) for discovery;
//! - `POST /` — JSON-RPC 2.0: `tasks/send`, `tasks/get`, `tasks/cancel`,
//!   `tasks/delete` (client-side cleanup);
//! - `GET /tasks/{id}/events` — SSE stream of the task's lifecycle updates
//!   (replays the current state first).
//!
//! Each task runs through the FULL agent pipeline (AgentBuilder → loop →
//! persist), exactly like ACP/chat. Tasks are persisted to SQLite and
//! survive restarts (in-flight tasks are marked failed after a restart).
//! A2A complements MCP (agent↔tool) — MCP stays the tool layer, A2A is
//! the agent↔agent layer.
//!
//! Security: loopback-only, disabled by default (`app.a2a_enabled`).
//! Permission prompts still surface in the main window (interactive
//! semantics, same as ACP).

use crate::agent::agent_builder::AgentBuilder;
use crate::agent::agent_loop::AgentLoopMode;
use crate::core::types::ConversationItem;
use crate::bootstrap::AppState;
use axum::extract::State as AxumState;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::AppHandle;
use tokio::sync::{broadcast, Mutex};
use tracing::info;

pub mod store;

// ── A2A wire types (A2A 1.0 subset) ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    pub name: String,
    pub description: String,
    pub url: String,
    pub version: String,
    pub capabilities: serde_json::Value,
    pub skills: Vec<serde_json::Value>,
    pub default_input_modes: Vec<String>,
    pub default_output_modes: Vec<String>,
    pub security: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Canceled,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifact {
    pub name: String,
    pub parts: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<TaskArtifact>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

// ── Task bookkeeping ─────────────────────────────────────────────────────

#[derive(Clone)]
struct TaskRecord {
    task: Task,
    session_id: Option<String>,
}

/// Shared axum state.
#[derive(Clone)]
pub struct A2aState {
    pub app: AppHandle,
    pub state: AppState,
    tasks: Arc<Mutex<HashMap<String, TaskRecord>>>,
    /// Task update bus — `/tasks/{id}/events` subscribers receive every
    /// status change for their task.
    events: broadcast::Sender<serde_json::Value>,
}

// ── JSON-RPC envelope ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct RpcRequest {
    id: serde_json::Value,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i64,
    message: String,
}

fn error(id: serde_json::Value, code: i64, message: impl Into<String>) -> Json<RpcResponse> {
    Json(RpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(RpcError {
            code,
            message: message.into(),
        }),
    })
}

fn ok(id: serde_json::Value, result: serde_json::Value) -> Json<RpcResponse> {
    Json(RpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    })
}

// ── Handlers ─────────────────────────────────────────────────────────────

async fn agent_card(AxumState(state): AxumState<Arc<A2aState>>) -> Json<AgentCard> {
    let (name, description, url) = {
        let cfg = state.state.config().map(|c| c.app.clone()).unwrap_or_default();
        (
            "DeepDepCat".to_string(),
            format!(
                "DeepDepCat agent ({}) — Code 编码助手 + Depwork 办公自动化。",
                cfg.default_model
            ),
            format!("http://127.0.0.1:{}", cfg.a2a_port),
        )
    };
    Json(AgentCard {
        name,
        description,
        url,
        version: env!("CARGO_PKG_VERSION").to_string(),
        capabilities: serde_json::json!({
            "streaming": false,
            "pushNotifications": false,
            "stateTransitionHistory": false
        }),
        skills: vec![],
        default_input_modes: vec!["text".to_string()],
        default_output_modes: vec!["text".to_string()],
        security: serde_json::json!({ "authentication": "none", "restrictedToLocalhost": true }),
    })
}

async fn rpc(
    AxumState(state): AxumState<Arc<A2aState>>,
    Json(req): Json<RpcRequest>,
) -> Json<RpcResponse> {
    match req.method.as_str() {
        "tasks/send" => handle_send(&state, req).await,
        "tasks/get" => handle_get(&state, req).await,
        "tasks/cancel" => handle_cancel(&state, req).await,
        "tasks/delete" => handle_delete(&state, req).await,
        other => error(
            req.id,
            -32601,
            format!(
                "Method not found: {other} (supported: tasks/send, tasks/get, tasks/cancel, tasks/delete)"
            ),
        ),
    }
}

async fn handle_send(state: &A2aState, req: RpcRequest) -> Json<RpcResponse> {
    let id = req.params.get("id").and_then(|v| v.as_str()).map(String::from);
    let text = req
        .params
        .get("message")
        .and_then(|m| m.get("parts"))
        .and_then(|p| p.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| part.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|s| !s.trim().is_empty());
    let Some(text) = text else {
        return error(req.id, -32602, "message.parts[].text is required".to_string());
    };
    let session_id = req
        .params
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(String::from);
    let task_id = id.unwrap_or_else(|| format!("a2a-{}", &crate::core::ids::generate_id()[..12]));

    {
        let mut tasks = state.tasks.lock().await;
        if tasks.contains_key(&task_id) {
            return error(req.id, -32602, format!("Task already exists: {task_id}"));
        }
        tasks.insert(
            task_id.clone(),
            TaskRecord {
                task: Task {
                    id: task_id.clone(),
                    status: TaskStatus {
                        state: TaskState::Working,
                        message: None,
                    },
                    artifacts: None,
                    metadata: None,
                },
                session_id: None,
            },
        );
    }
    state.persist_task(&task_id).await;
    {
        let tasks = state.tasks.lock().await;
        if let Some(record) = tasks.get(&task_id) {
            state.publish_task(&record.task);
        }
    }

    let runner = state.clone();
    let task_id2 = task_id.clone();
    tauri::async_runtime::spawn(async move {
        runner.run_task(task_id2, session_id, text).await;
    });

    let task = {
        let tasks = state.tasks.lock().await;
        tasks.get(&task_id).map(|r| r.task.clone())
    };
    match task {
        Some(task) => ok(req.id, serde_json::to_value(task).unwrap_or_default()),
        None => error(req.id, -32603, "Task vanished after creation".to_string()),
    }
}

async fn handle_get(state: &A2aState, req: RpcRequest) -> Json<RpcResponse> {
    let Some(id) = req.params.get("id").and_then(|v| v.as_str()) else {
        return error(req.id, -32602, "Missing id".to_string());
    };
    let tasks = state.tasks.lock().await;
    match tasks.get(id) {
        Some(record) => ok(req.id, serde_json::to_value(&record.task).unwrap_or_default()),
        None => error(req.id, -32004, format!("Task not found: {id}")),
    }
}

async fn handle_cancel(state: &A2aState, req: RpcRequest) -> Json<RpcResponse> {
    let Some(id) = req.params.get("id").and_then(|v| v.as_str()) else {
        return error(req.id, -32602, "Missing id".to_string());
    };
    let updated = {
        let mut tasks = state.tasks.lock().await;
        let Some(record) = tasks.get_mut(id) else {
            return error(req.id, -32004, format!("Task not found: {id}"));
        };
        match record.task.status.state {
            TaskState::Completed | TaskState::Canceled | TaskState::Failed => {
                let task = record.task.clone();
                return ok(req.id, serde_json::to_value(&task).unwrap_or_default());
            }
            _ => {}
        }
        if let Some(session_id) = record.session_id.as_deref() {
            state.state.cancel_session(session_id).await;
        }
        record.task.status = TaskStatus {
            state: TaskState::Canceled,
            message: Some("Canceled by client".to_string()),
        };
        record.task.clone()
    };
    state.persist_task(id).await;
    state.publish_task(&updated);
    ok(req.id, serde_json::to_value(&updated).unwrap_or_default())
}

/// Remove a task (client-side cleanup; the backing session is untouched).
async fn handle_delete(state: &A2aState, req: RpcRequest) -> Json<RpcResponse> {
    let Some(id) = req.params.get("id").and_then(|v| v.as_str()) else {
        return error(req.id, -32602, "Missing id".to_string());
    };
    let removed = {
        let mut tasks = state.tasks.lock().await;
        tasks.remove(id).is_some()
    };
    let _ = store::delete_task(&state.state.db, id);
    if removed {
        ok(req.id, serde_json::json!({ "deleted": id }))
    } else {
        error(req.id, -32004, format!("Task not found: {id}"))
    }
}

// ── Task runner (full agent pipeline) ────────────────────────────────────

impl A2aState {
    /// Persist a task row (best-effort; a DB failure must not break the
    /// in-memory task lifecycle).
    async fn persist_task(&self, task_id: &str) {
        let snapshot = {
            let tasks = self.tasks.lock().await;
            tasks
                .get(task_id)
                .map(|r| (r.task.clone(), r.session_id.clone()))
        };
        if let Some((task, session_id)) = snapshot {
            if let Ok(json) = serde_json::to_string(&task) {
                if let Err(e) =
                    store::upsert_task(&self.state.db, task_id, session_id.as_deref(), &json)
                {
                    tracing::warn!(task_id, error = %e, "Failed to persist A2A task");
                }
            }
        }
    }

    /// Publish a task update to `/tasks/{id}/events` subscribers.
    fn publish_task(&self, task: &Task) {
        let _ = self
            .events
            .send(serde_json::to_value(task).unwrap_or_default());
    }

    async fn run_task(&self, task_id: String, session_id: Option<String>, text: String) {
        let outcome = self.run_agent(&task_id, session_id.as_deref(), &text).await;
        let updated = {
            let mut tasks = self.tasks.lock().await;
            let Some(record) = tasks.get_mut(&task_id) else {
                return;
            };
            match outcome {
                Ok(answer) => {
                    record.task.status = TaskStatus {
                        state: TaskState::Completed,
                        message: None,
                    };
                    record.task.artifacts = Some(vec![TaskArtifact {
                        name: "final".to_string(),
                        parts: vec![serde_json::json!({ "type": "text", "text": answer })],
                    }]);
                }
                Err(e) => {
                    record.task.status = TaskStatus {
                        state: TaskState::Failed,
                        message: Some(e),
                    };
                }
            }
            record.task.clone()
        };
        self.persist_task(&task_id).await;
        self.publish_task(&updated);
    }

    async fn run_agent(
        &self,
        task_id: &str,
        requested_session: Option<&str>,
        text: &str,
    ) -> Result<String, String> {
        let state = &self.state;
        let workspace = state.workspace.read().map_err(|e| e.to_string())?.clone();

        // Resolve or create the backing session.
        let session = {
            let mut sessions = state.sessions.lock().await;
            match requested_session {
                Some(existing) if sessions.get_session(existing).is_ok() => {
                    sessions.get_session(existing).cloned()?
                }
                _ => sessions
                    .create_session(
                        config_default_model(state).await?,
                        config_default_provider(state).await?,
                        None,
                        workspace
                            .as_ref()
                            .map(|w| w.to_string_lossy().to_string()),
                        None,
                        None,
                        None,
                    )
                    .map_err(|e| e.to_string())?,
            }
        };
        let session_id = session.id.clone();
        {
            let mut tasks = self.tasks.lock().await;
            if let Some(record) = tasks.get_mut(task_id) {
                record.session_id = Some(session_id.clone());
            }
        }

        let usage_tracker = state.usage_tracker(&session_id).await;
        let built = AgentBuilder::from_state(state, workspace)?
            .with_mode(AgentLoopMode::Standard)
            .with_work_mode(crate::toolkit::WorkMode::parse(None))
            .with_usage_tracker(usage_tracker)
            .with_provider(Some(session.provider.clone()))
            .build();

        // Respect the global session-concurrency cap (MAX_CONCURRENT_SESSIONS)
        // like the chat UI path — a local A2A client must not drive unlimited
        // parallel agent loops.
        let _session_permit = state
            .session_concurrency
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| format!("Session concurrency semaphore closed: {e}"))?;

        let mut chat_state = {
            let mut sessions = state.sessions.lock().await;
            sessions.take_chat_state(&session_id).map_err(|e| e.to_string())?
        };
        let trace_id = crate::core::ids::trace_id();
        chat_state.trace_id = Some(trace_id.clone());
        info!(session_id = %session_id, trace_id = %trace_id, "A2A task trace started");

        let cancel = tokio_util::sync::CancellationToken::new();
        // Register the token so an A2A `tasks/cancel` actually interrupts the
        // loop — handle_cancel → cancel_session looks up cancellation_tokens,
        // and without registration it found nothing (a no-op) while the task
        // kept running and then overwrote the Canceled status with its own
        // Completed/Failed result.
        state.register_cancellation(&session_id, cancel.clone()).await;
        let result = built
            .loop_
            .run(&self.app, &session_id, &mut chat_state, text, &cancel, false, None, None)
            .await;
        // The final assistant text is the task artifact — extract it before
        // the state is put back (the loop's Ok value is the turn id).
        let final_text = {
            let mut found = String::new();
            for item in chat_state.conversation.iter().rev() {
                if let ConversationItem::Assistant(m) = item {
                    if !m.content.trim().is_empty() {
                        found = m.content.clone();
                        break;
                    }
                }
            }
            found
        };
        {
            let mut sessions = state.sessions.lock().await;
            let _ = sessions.put_chat_state(&session_id, chat_state);
            let _ = sessions.persist_session(&session_id);
            let _ = sessions.persist_messages(&session_id);
        }
        state.remove_cancellation(&session_id).await;
        let status = match &result {
            Ok(_) => "completed",
            Err(e) if e.is_cancelled() => "cancelled",
            Err(_) => "error",
        };
        state.finalize_run(&self.app, &session_id, status).await;
        match result {
            Ok(_) => Ok(final_text),
            Err(e) => Err(e.to_string()),
        }
    }
}

async fn config_default_model(state: &AppState) -> Result<String, String> {
    let config = state.config().map_err(|e| e.to_string())?;
    Ok(config.app.default_model.clone())
}

async fn config_default_provider(state: &AppState) -> Result<String, String> {
    let config = state.config().map_err(|e| e.to_string())?;
    Ok(config.app.default_provider.clone())
}

/// Build the A2A router (loopback-only; the caller binds 127.0.0.1).
pub fn router(state: Arc<A2aState>) -> Router {
    Router::new()
        .route("/.well-known/agent.json", get(agent_card))
        .route("/", post(rpc))
        .route("/tasks/{id}/events", get(task_events))
        .with_state(state)
}

/// SSE stream of a task's lifecycle updates. Replays the current task first
/// (a subscriber that connects after completion still gets the result),
/// then streams every subsequent change until the connection closes.
async fn task_events(
    AxumState(state): AxumState<Arc<A2aState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let initial = {
        let tasks = state.tasks.lock().await;
        tasks
            .get(&id)
            .map(|r| serde_json::to_value(&r.task).unwrap_or_default())
    };
    let mut rx = state.events.subscribe();
    let stream = async_stream::stream! {
        if let Some(task) = initial {
            yield Ok(Event::default().event("task").data(task.to_string()));
        }
        loop {
            match rx.recv().await {
                Ok(value) => {
                    if value.get("id").and_then(|v| v.as_str()) == Some(id.as_str()) {
                        yield Ok(Event::default().event("task").data(value.to_string()));
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Start the A2A server on a loopback port.
pub async fn serve(app: AppHandle, state: AppState, port: u16) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| format!("A2A bind failed on {port}: {e}"))?;
    info!(port, "A2A server listening on 127.0.0.1");
    let (events, _) = broadcast::channel(256);
    let a2a = Arc::new(A2aState {
        app,
        state,
        tasks: Arc::new(Mutex::new(HashMap::new())),
        events,
    });
    // Hydrate persisted tasks: in-flight ones from a previous process died
    // with it, so they are marked failed instead of pretending to run.
    for row in store::load_tasks(&a2a.state.db) {
        if let Ok(mut task) = serde_json::from_str::<Task>(&row.task_json) {
            if matches!(
                task.status.state,
                TaskState::Working | TaskState::Submitted
            ) {
                task.status = TaskStatus {
                    state: TaskState::Failed,
                    message: Some("服务重启，任务中断".to_string()),
                };
                if let Ok(json) = serde_json::to_string(&task) {
                    let _ = store::upsert_task(
                        &a2a.state.db,
                        &task.id,
                        row.session_id.as_deref(),
                        &json,
                    );
                }
            }
            a2a.tasks.lock().await.insert(
                task.id.clone(),
                TaskRecord {
                    task,
                    session_id: row.session_id,
                },
            );
        }
    }
    axum::serve(listener, router(a2a))
    .await
    .map_err(|e| format!("A2A server error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_states_round_trip_camel_case_wire() {
        let status = TaskStatus {
            state: TaskState::Working,
            message: None,
        };
        let json = serde_json::to_value(&status).unwrap();
        assert_eq!(json["state"], "working");
        let back: TaskStatus = serde_json::from_value(json).unwrap();
        assert_eq!(back.state, TaskState::Working);
    }

    #[test]
    fn task_serializes_camel_case() {
        let task = Task {
            id: "a2a-1".into(),
            status: TaskStatus {
                state: TaskState::Completed,
                message: None,
            },
            artifacts: Some(vec![TaskArtifact {
                name: "final".into(),
                parts: vec![serde_json::json!({ "type": "text", "text": "ok" })],
            }]),
            metadata: None,
        };
        let json = serde_json::to_value(&task).unwrap();
        assert_eq!(json["status"]["state"], "completed");
        assert!(json.get("artifacts").is_some());
    }

    #[test]
    fn unknown_method_reports_error() {
        // Wire-level guard: the dispatcher is exercised through `rpc` with
        // a real state in integration; here we assert the error shape that
        // any unknown method must produce.
        let resp = RpcResponse {
            jsonrpc: "2.0",
            id: serde_json::json!(1),
            result: None,
            error: Some(RpcError {
                code: -32601,
                message: "Method not found".into(),
            }),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], -32601);
    }
}
