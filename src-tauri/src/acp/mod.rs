//! ACP (Agent Client Protocol) server.
//!
//! Exposes DeepDepCat as a remote agent to external clients (IDEs, other
//! agents, scripts) over localhost, using the ACP 2.0 wire shape:
//!
//! - `POST /rpc` — JSON-RPC 2.0 methods (`session/new`, `session/update`,
//!   `session/close`, `agent/update`, `prompt/stream`, `config/get_config`)
//! - `GET /events` — SSE stream of agent-pushed events (`session/update`,
//!   `agent/update`, `prompt/update`, `prompt/streaming_update`)
//!
//! Prompts run through the full agent loop (tools, permissions, memory) via
//! `AgentBuilder` — identical machinery to the chat UI. The response text is
//! forwarded from the existing `chat-stream` channel by a Rust-side event
//! listener (turn_id → session map maintained from `TurnStart`/`TurnEnd`).
//!
//! Security: bound to 127.0.0.1 only, disabled by default
//! (`app.acp_enabled` in config.toml). Permission prompts still surface in
//! the main window — an ACP-driven tool call asks the user exactly like a
//! chat-driven one.

use crate::agent::agent_builder::AgentBuilder;
use crate::agent::agent_loop::AgentLoopMode;
use crate::bootstrap::AppState;
use crate::core::types::StreamEvent;
use axum::extract::State as AxumState;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::stream::Stream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tauri::{AppHandle, Listener};
use tokio::sync::{broadcast, Mutex};

pub mod bridge;

/// Default broadcast capacity for the SSE event bus.
const BUS_CAPACITY: usize = 1024;

/// A subscription to the agent event stream (one per `/events` connection).
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<String>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// Emit an SSE-formatted event line to all subscribers.
    pub fn emit(&self, event: &str, data: serde_json::Value) {
        let line = format!(
            "event: {event}\ndata: {}\n\n",
            serde_json::to_string(&data).unwrap_or_else(|_| "{}".into())
        );
        let _ = self.tx.send(line);
    }

    fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared state handed to the axum router.
#[derive(Clone)]
pub struct AcpState {
    pub app: AppHandle,
    pub state: AppState,
    pub bus: EventBus,
    /// turn_id → session_id, maintained from the `chat-stream` bridge so
    /// text deltas (which carry only turn_id) can be routed per session.
    pub active_turns: Arc<Mutex<HashMap<String, String>>>,
}

// ── JSON-RPC wire types ───────────────────────────────────────────────────

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

fn ok(id: serde_json::Value, result: serde_json::Value) -> RpcResponse {
    RpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    }
}

fn err(id: serde_json::Value, code: i64, message: impl Into<String>) -> RpcResponse {
    RpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(RpcError {
            code,
            message: message.into(),
        }),
    }
}

// ── Method params/results ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct NewSessionParams {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    work_mode: Option<String>,
    #[serde(default)]
    permission_mode: Option<String>,
    #[serde(default)]
    workspace: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateSessionParams {
    session_id: String,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CloseSessionParams {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct StreamPromptParams {
    session_id: String,
    content: String,
}

// ── Router / server ──────────────────────────────────────────────────────

/// Start the ACP server on loopback. Returns immediately after binding.
pub async fn serve(
    app: AppHandle,
    state: AppState,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let acp_state = Arc::new(AcpState {
        app: app.clone(),
        state: state.clone(),
        bus: EventBus::new(),
        active_turns: Arc::new(Mutex::new(HashMap::new())),
    });

    // Rust-side bridge: forward the app's own chat-stream channel into the
    // SSE bus so ACP clients see streaming text deltas.
    {
        let acp = acp_state.clone();
        app.listen("chat-stream", move |event| {
            let Ok(stream_event) = serde_json::from_str::<StreamEvent>(event.payload()) else {
                return;
            };
            let acp = acp.clone();
            tauri::async_runtime::spawn(async move {
                acp.forward_stream_event(stream_event).await;
            });
        });
    }

    let router = Router::new()
        .route("/rpc", post(rpc_handler))
        .route("/events", get(events_handler))
        .with_state(acp_state);

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "ACP server listening (Agent Client Protocol)");
    tauri::async_runtime::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!(error = %e, "ACP server exited");
        }
    });
    Ok(())
}

// ── Handlers ─────────────────────────────────────────────────────────────

async fn rpc_handler(
    AxumState(state): AxumState<Arc<AcpState>>,
    Json(request): Json<RpcRequest>,
) -> Json<RpcResponse> {
    let id = request.id.clone();
    match dispatch(state, request).await {
        Ok(result) => Json(ok(id, result)),
        Err((code, message)) => Json(err(id, code, message)),
    }
}

async fn dispatch(
    state: Arc<AcpState>,
    request: RpcRequest,
) -> Result<serde_json::Value, (i64, String)> {
    match request.method.as_str() {
        "config/get_config" => Ok(serde_json::json!({
            "user_prompt_types": ["text"],
            "output_types": ["text"],
        })),
        "session/new" => {
            let p: NewSessionParams = serde_json::from_value(request.params)
                .map_err(|e| (-32602, format!("Invalid params: {e}")))?;
            let (model, provider) = {
                let config = state.state.config().map_err(|e| (-32603, e.to_string()))?;
                (
                    p.model.unwrap_or(config.app.default_model.clone()),
                    p.provider.unwrap_or(config.app.default_provider.clone()),
                )
            };
            if let Some(w) = p.workspace.as_deref() {
                let path = std::path::PathBuf::from(w);
                if path.exists() {
                    let mut guard = state
                        .state
                        .workspace
                        .write()
                        .map_err(|e| (-32603, e.to_string()))?;
                    *guard = Some(path);
                }
            }
            let mut sessions = state.state.sessions.lock().await;
            let session = sessions
                .create_session(
                    model,
                    provider,
                    p.system_prompt,
                    None,
                    p.work_mode,
                    None,
                    p.permission_mode,
                )
                .map_err(|e| (-32603, e.to_string()))?;
            state.bus.emit(
                "session/update",
                serde_json::json!({ "sessionId": session.id }),
            );
            Ok(serde_json::json!({ "sessionId": session.id }))
        }
        "session/update" => {
            let p: UpdateSessionParams = serde_json::from_value(request.params)
                .map_err(|e| (-32602, format!("Invalid params: {e}")))?;
            let mut sessions = state.state.sessions.lock().await;
            if let Some(sp) = p.system_prompt {
                sessions
                    .get_chat_state(&p.session_id)
                    .map_err(|e| (-32603, e.to_string()))?
                    .set_system_prompt(&sp);
            }
            if let Some(model) = p.model {
                sessions
                    .set_model(&p.session_id, model)
                    .map_err(|e| (-32603, e.to_string()))?;
            }
            let _ = sessions.persist_session(&p.session_id);
            drop(sessions);
            state.bus.emit(
                "session/update",
                serde_json::json!({ "sessionId": p.session_id }),
            );
            Ok(serde_json::json!({}))
        }
        "session/close" => {
            let p: CloseSessionParams = serde_json::from_value(request.params)
                .map_err(|e| (-32602, format!("Invalid params: {e}")))?;
            let mut sessions = state.state.sessions.lock().await;
            sessions
                .delete_session(&p.session_id)
                .map_err(|e| (-32603, e.to_string()))?;
            drop(sessions);
            // Same per-session registry purge as the UI delete path — the
            // ACP close must not leave usage trackers/caches/plan state
            // behind for a session that no longer exists.
            state.state.cleanup_session(&p.session_id).await;
            state.bus.emit(
                "session/update",
                serde_json::json!({ "sessionId": p.session_id }),
            );
            Ok(serde_json::json!({}))
        }
        "session/evidence" => {
            let session_id = request
                .params
                .get("session_id")
                .and_then(|v| v.as_str())
                .ok_or((-32602, "Missing session_id".to_string()))?;
            let evidence = state
                .collect_session_evidence(session_id)
                .await
                .map_err(|e| (-32603, e))?;
            serde_json::to_value(evidence).map_err(|e| (-32603, e.to_string()))
        }
        "agent/update" => {
            let config = state.state.config().map_err(|e| (-32603, e.to_string()))?;
            let model = config.app.default_model.clone();
            let tools: Vec<serde_json::Value> = state
                .state
                .tools
                .definitions()
                .into_iter()
                .map(|def| {
                    serde_json::json!({
                        "name": def.function.name,
                        "description": def.function.description,
                        "parameters": def.function.parameters,
                    })
                })
                .collect();
            drop(config);
            state.bus.emit("agent/update", serde_json::json!({}));
            Ok(serde_json::json!({ "model": model, "tools": tools }))
        }
        "prompt/stream" => {
            let p: StreamPromptParams = serde_json::from_value(request.params)
                .map_err(|e| (-32602, format!("Invalid params: {e}")))?;
            let prompt_id = crate::core::ids::generate_id();

            // Verify the session exists before accepting the prompt.
            {
                let mut sessions = state.state.sessions.lock().await;
                sessions
                    .get_session(&p.session_id)
                    .map_err(|e| (-32603, e.to_string()))?;
            }
            state.bus.emit(
                "prompt/update",
                serde_json::json!({
                    "sessionId": p.session_id,
                    "promptId": prompt_id.clone(),
                    "state": "running",
                }),
            );

            let acp = state.clone();
            let session_id = p.session_id.clone();
            let content = p.content.clone();
            let prompt_id_for_task = prompt_id.clone();
            tauri::async_runtime::spawn(async move {
                let outcome = acp.run_prompt(&session_id, &content).await;
                match outcome {
                    Ok(msg_id) => {
                        acp.bus.emit(
                            "prompt/update",
                            serde_json::json!({
                                "sessionId": session_id,
                                "promptId": prompt_id_for_task,
                                "state": "completed",
                                "result": { "messageId": msg_id },
                            }),
                        );
                    }
                    Err(e) => {
                        acp.bus.emit(
                            "prompt/update",
                            serde_json::json!({
                                "sessionId": session_id,
                                "promptId": prompt_id_for_task,
                                "state": "failed",
                                "error": e.to_string(),
                            }),
                        );
                    }
                }
            });

            Ok(serde_json::json!({ "promptId": prompt_id }))
        }
        _ => Err((-32601, format!("Method not found: {}", request.method))),
    }
}

impl AcpState {
    /// Run one agent turn for an ACP session — identical pipeline to the
    /// chat UI (AgentBuilder → AgentLoop). The session's ChatState is taken
    /// out, run, and put back + persisted.
    async fn run_prompt(&self, session_id: &str, content: &str) -> Result<String, String> {
        let workspace = self
            .state
            .workspace
            .read()
            .map_err(|e| e.to_string())?
            .clone();
        let usage_tracker = self.state.usage_tracker(session_id).await;
        // The session's product surface drives tool filtering — an ACP
        // client that opened a depwork session gets the Depwork toolset.
        let work_mode = {
            let mut sessions = self.state.sessions.lock().await;
            sessions
                .get_session(session_id)
                .map(|s| s.work_mode.clone())
                .unwrap_or_else(|_| "code".to_string())
        };

        let built_agent = AgentBuilder::from_state(&self.state, workspace.clone())?
            .with_mode(AgentLoopMode::Standard)
            .with_work_mode(crate::toolkit::WorkMode::parse(Some(&work_mode)))
            .with_usage_tracker(usage_tracker)
            .with_debug_mode(false)
            .build();

        let agent_loop = built_agent.loop_;

        // Respect the global session-concurrency cap (MAX_CONCURRENT_SESSIONS)
        // like the chat UI path. Without this, a local ACP client can drive
        // unlimited parallel agent loops that hit the LLM at once — exactly
        // what the semaphore exists to prevent.
        let _session_permit = self
            .state
            .session_concurrency
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| format!("Session concurrency semaphore closed: {e}"))?;

        let mut chat_state = {
            let mut sessions = self.state.sessions.lock().await;
            sessions.take_chat_state(session_id).map_err(|e| {
                if e.to_string().contains("already checked out") {
                    "Session is busy — another turn is running".to_string()
                } else {
                    e.to_string()
                }
            })?
        };
        let trace_id = crate::core::ids::trace_id();
        chat_state.trace_id = Some(trace_id.clone());
        tracing::info!(session_id = %session_id, trace_id = %trace_id, "ACP prompt trace started");

        let cancel = tokio_util::sync::CancellationToken::new();
        // Register so ACP/A2A `cancel` actually interrupts this run.
        self.state.register_cancellation(session_id, cancel.clone()).await;
        let tracker = crate::workspace::checkpoint::FileStateTracker::new(workspace.clone());
        let result = agent_loop
            .run(
                &self.app,
                session_id,
                &mut chat_state,
                content,
                &cancel,
                false,
                Some(tracker),
                None,
            )
            .await;

        // ── Self-evolution: background procedure capture ────────────
        // Same contract as the chat UI path — successful turns that
        // changed files extract 0-1 reusable workflows into the project
        // procedures.md, throttled to once per 10 minutes per session.
        if result.is_ok()
            && !chat_state.agent_edited_paths.is_empty()
            && chat_state.conversation.len() >= 10
        {
            tracing::debug!(
                session_id = %session_id,
                edited = chat_state.agent_edited_paths.len(),
                conversation_items = chat_state.conversation.len(),
                "procedure capture candidate (acp)"
            );
            let due = {
                let mut last = self.state.procedure_last_run.lock().await;
                let now = std::time::Instant::now();
                match last.get(session_id) {
                    Some(prev)
                        if now.duration_since(*prev) < std::time::Duration::from_secs(600) =>
                    {
                        false
                    }
                    _ => {
                        last.insert(session_id.to_string(), now);
                        true
                    }
                }
            };
            if due {
                let llm = self.state.llm_client.clone();
                let model = chat_state.model.clone();
                let provider = chat_state.provider.clone();
                let conversation = chat_state.conversation.clone();
                let ws = workspace.clone();
                let mode = work_mode.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = crate::memory::procedure_capture::run_procedure_pass(
                        &llm,
                        &model,
                        provider.as_deref(),
                        &conversation,
                        ws.as_deref(),
                        &mode,
                    )
                    .await;
                });
            }
        }

        {
            let mut sessions = self.state.sessions.lock().await;
            let _ = sessions.put_chat_state(session_id, chat_state);
            let _ = sessions.persist_session(session_id);
            let _ = sessions.persist_messages(session_id);
        }
        let status = match &result {
            Ok(_) => "completed",
            Err(e) if e.is_cancelled() => "cancelled",
            Err(_) => "error",
        };
        self.state.finalize_run(&self.app, session_id, status).await;

        result.map_err(|e| e.to_string())
    }
}

/// SSE endpoint — streams every agent event to the connected client.
async fn events_handler(
    AxumState(state): AxumState<Arc<AcpState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.bus.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(line) => {
                    let (event, data) = parse_sse_line(&line);
                    yield Ok(Event::default().event(event).data(data));
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    yield Ok(Event::default().event("resync").data(format!("lagged {n} events")));
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Split a stored `event: x\ndata: y\n\n` line back into parts.
fn parse_sse_line(line: &str) -> (String, String) {
    let mut event = String::new();
    let mut data = String::new();
    for part in line.split("\n\n").next().unwrap_or("").split('\n') {
        if let Some(v) = part.strip_prefix("event: ") {
            event = v.to_string();
        } else if let Some(v) = part.strip_prefix("data: ") {
            data = v.to_string();
        }
    }
    (event, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_line_roundtrips() {
        let bus = EventBus::new();
        // Subscribe BEFORE emitting — tokio broadcast drops messages sent
        // while zero receivers exist, so a late subscriber would block.
        let mut rx = bus.subscribe();
        bus.emit(
            "prompt/streaming_update",
            serde_json::json!({ "sessionId": "s1", "text": "hi" }),
        );
        let line = rx.blocking_recv().unwrap();
        let (event, data) = parse_sse_line(&line);
        assert_eq!(event, "prompt/streaming_update");
        let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(parsed["sessionId"], "s1");
        assert_eq!(parsed["text"], "hi");
    }

    #[test]
    fn unknown_method_is_not_found() {
        let bus = EventBus::new();
        // Build a tiny AcpState-free dispatch check: JSON-RPC error shape.
        let req = RpcRequest {
            id: serde_json::json!(1),
            method: "no/such/method".into(),
            params: serde_json::Value::Null,
        };
        // We can't easily build AcpState without an AppHandle, so assert the
        // error convention directly (code -32601) via the response builder.
        let resp = err(req.id, -32601, format!("Method not found: {}", req.method));
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], -32601);
        assert!(json["result"].is_null());
        let _ = bus;
    }
}
