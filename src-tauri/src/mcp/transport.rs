//! MCP transport — handles communication with MCP servers.
//!
//! Three transport implementations:
//! - StdioTransport: spawns a child process, communicates via stdin/stdout
//! - SseTransport: connects via Server-Sent Events
//! - HttpTransport: uses HTTP POST requests

use crate::core::error::{AppError, AppResult};
use crate::mcp::types::{JsonRpcMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};
use async_trait::async_trait;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{oneshot, Mutex};
use tracing::{debug, info};

/// MCP protocol mode after negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpProtocol {
    /// Legacy session handshake (`initialize` + `notifications/initialized`),
    /// negotiated at 2024-11-05.
    Legacy,
    /// Stateless 2026-07-28: no handshake; every request carries `_meta`
    /// (protocolVersion / clientInfo / clientCapabilities) and HTTP requests
    /// add the Mcp-Method / Mcp-Name / MCP-Protocol-Version routing headers.
    Stateless2026,
}

/// The 2026-07-28 stateless protocol revision identifier.
pub const MCP_PROTOCOL_VERSION_2026_07_28: &str = "2026-07-28";

/// Legacy protocol version used for the initialize handshake.
pub const MCP_PROTOCOL_VERSION_LEGACY: &str = "2024-11-05";

const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

/// Attach the stateless `_meta` envelope to request params. The 2026-07-28
/// spec makes every request self-describing: protocol version, client
/// identity, and capabilities travel with the request instead of a session
/// handshake.
pub(crate) fn with_stateless_meta(params: Option<serde_json::Value>) -> Option<serde_json::Value> {
    let meta = serde_json::json!({
        META_PROTOCOL_VERSION: MCP_PROTOCOL_VERSION_2026_07_28,
        META_CLIENT_INFO: {
            "name": "DeepDepCat",
            "version": env!("CARGO_PKG_VERSION"),
        },
        META_CLIENT_CAPABILITIES: {},
    });
    match params {
        Some(serde_json::Value::Object(mut map)) => {
            map.insert("_meta".into(), meta);
            Some(serde_json::Value::Object(map))
        }
        Some(other) => Some(other),
        None => Some(serde_json::json!({ "_meta": meta })),
    }
}

/// The `Mcp-Name` routing header value: the tool name for `tools/call`,
/// otherwise the JSON-RPC method itself (SEP-2243).
pub(crate) fn mcp_name_header(method: &str, params: Option<&serde_json::Value>) -> String {
    if method == "tools/call" {
        params
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or(method)
            .to_string()
    } else {
        method.to_string()
    }
}

/// Whether an MCP error signals "this server does not speak the 2026-07-28
/// stateless protocol" — the fallback-to-legacy-handshake condition.
pub(crate) fn is_stateless_fallback_error(message: &str) -> bool {
    let lower = message.to_lowercase();
    if lower.contains("unsupported protocol") || lower.contains("protocol version") {
        return true;
    }
    for code in ["-32601", "-32022", "-32004", "http 404", "http 405"] {
        if lower.contains(code) {
            return true;
        }
    }
    false
}

/// One in-flight request awaiting its JSON-RPC response. The method and
/// params are kept so a reconnected SSE stream can transparently re-send
/// IDEMPOTENT calls instead of letting them die with the old stream.
struct PendingRequest {
    method: String,
    params: Option<serde_json::Value>,
    tx: oneshot::Sender<JsonRpcResponse>,
}

/// Methods safe to re-send after an SSE reconnect: they have no side
/// effects, so a duplicate delivery is harmless. `tools/call` and
/// `resources/read` are deliberately excluded (the server may have already
/// executed the call before the stream dropped).
fn is_idempotent_method(method: &str) -> bool {
    matches!(
        method,
        "initialize"
            | "ping"
            | "tools/list"
            | "resources/list"
            | "prompts/list"
            | "server/discover"
    )
}

/// Read at most `cap` bytes of a response body. Error responses from a
/// misbehaving server must not be drained unboundedly into memory.
async fn read_limited_text(response: reqwest::Response, cap: usize) -> String {
    let mut out = String::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(bytes) = chunk else { break };
        let remaining = cap.saturating_sub(out.len());
        if remaining == 0 {
            break;
        }
        let text = String::from_utf8_lossy(&bytes);
        let take = text.len().min(remaining);
        out.push_str(&text[..take]);
    }
    out
}

/// Handler for server-initiated JSON-RPC requests (e.g. `elicitation/create`).
///
/// The transport invokes this when the server sends a request *to* the
/// client. The handler returns the result payload, which the transport
/// writes back as a JSON-RPC response. `None` means "no response" (the
/// request was handled out-of-band or the connection is closing).
pub type ServerRequestHandler = Arc<
    dyn Fn(JsonRpcRequest) -> Pin<Box<dyn Future<Output = Option<serde_json::Value>> + Send>>
        + Send
        + Sync,
>;

/// Handler invoked when the server announces
/// `notifications/tools/list_changed` — the manager hot-refreshes the tool
/// registry without a full reconnect.
pub type ToolListChangedHandler = Arc<dyn Fn() + Send + Sync>;

/// The transport trait for MCP communication.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a request and wait for the response.
    async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> AppResult<serde_json::Value>;

    /// Send a request under the stateless 2026-07-28 protocol: request
    /// params carry the `_meta` envelope (protocol version, client info,
    /// capabilities) and HTTP adds the routing headers. Defaults to the
    /// legacy path for transports without stateless support (tests/mocks).
    async fn request_stateless(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        self.request(method, params).await
    }

    /// Send a notification (no response expected).
    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> AppResult<()>;

    /// Close the transport.
    async fn close(&self) -> AppResult<()>;

    /// Register a handler for server-initiated requests.
    ///
    /// Default: no-op (HTTP transports cannot receive server pushes).
    async fn set_server_request_handler(&self, _handler: ServerRequestHandler) {}

    /// Register a handler invoked when the server announces
    /// `notifications/tools/list_changed` — the manager hot-refreshes the
    /// tool registry without a full reconnect. Default: no-op (HTTP).
    async fn set_tool_list_changed_handler(&self, _handler: ToolListChangedHandler) {}

    /// Whether this transport speaks the HTTP-flavored 2026-07-28 stateless
    /// revision (routing headers + `_meta` envelope). stdio returns `false`:
    /// a stdio server is probed with the classic `initialize` handshake, NOT
    /// `server/discover` — strict-validating stdio servers (FastMCP 1.x)
    /// reject the unknown method and stop responding, which surfaced as
    /// "MCP response channel closed" on connect.
    fn is_http_like(&self) -> bool {
        false
    }
}

/// stdio transport — communicates with a child process via stdin/stdout.
pub struct StdioTransport {
    child: Arc<Mutex<Option<Child>>>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, PendingRequest>>>,
    next_id: Arc<AtomicU64>,
    connected: Arc<std::sync::atomic::AtomicBool>,
    /// Last chunk of the child's stderr (capped) — when the process dies
    /// mid-request, this carries the ACTUAL crash reason (Python traceback,
    /// npx error, missing module…) into the "channel closed" error instead
    /// of an opaque message.
    stderr_tail: Arc<std::sync::Mutex<String>>,
    /// Handler for server-initiated requests (e.g. elicitation/create).
    server_request_handler: Arc<Mutex<Option<ServerRequestHandler>>>,
    /// Invoked when the server announces `notifications/tools/list_changed`.
    tool_list_changed_handler: Arc<Mutex<Option<ToolListChangedHandler>>>,
}

impl StdioTransport {
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
    ) -> AppResult<Self> {
        let mut cmd = Command::new(command);
        crate::core::proc::no_window_tokio(&mut cmd);
        cmd.args(args);
        for (k, v) in env {
            cmd.env(k, v);
        }
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::Mcp("Failed to capture stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::Mcp("Failed to capture stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AppError::Mcp("Failed to capture stderr".into()))?;
        let stderr_tail = Arc::new(std::sync::Mutex::new(String::new()));

        let pending: Arc<Mutex<HashMap<u64, PendingRequest>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let next_id = Arc::new(AtomicU64::new(1));
        let connected = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let server_request_handler: Arc<Mutex<Option<ServerRequestHandler>>> =
            Arc::new(Mutex::new(None));
        let tool_list_changed_handler: Arc<Mutex<Option<ToolListChangedHandler>>> =
            Arc::new(Mutex::new(None));

        // Spawn a reader task
        let pending_clone = pending.clone();
        let connected_clone = connected.clone();
        let handler_clone = server_request_handler.clone();
        let tool_handler_clone = tool_list_changed_handler.clone();
        let stdin_shared = Arc::new(Mutex::new(stdin));
        let stdin_clone = stdin_shared.clone();
        let stderr_tail_clone = stderr_tail.clone();
        // Drain the child's stderr in the background. stdio MCP servers
        // (npx/Python) log to stderr constantly; an unread pipe fills up and
        // BLOCKS the child on its next write — every subsequent request then
        // hangs until the 60s timeout. The reader discards lines (debug-
        // logged) so the child never stalls.
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::with_capacity(1024);
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buf);
                        debug!(stderr = %line.trim_end(), "MCP stdio stderr");
                        capture_stderr_tail(
                            &mut stderr_tail_clone.lock().unwrap_or_else(|e| e.into_inner()),
                            &line,
                            STDERR_TAIL_CAP,
                        );
                    }
                    Err(_) => break,
                }
            }
        });
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                // A misbehaving server could emit a multi-megabyte single
                // line; skip parsing it instead of letting serde allocate
                // a giant object (the reader still drains the pipe).
                if line.len() > MAX_STDIO_LINE {
                    tracing::warn!(bytes = line.len(), "MCP stdio line exceeds cap — skipping");
                    continue;
                }
                debug!(line = %line, "MCP stdio received");

                if let Ok(msg) = serde_json::from_str::<JsonRpcMessage>(&line) {
                    match msg {
                        JsonRpcMessage::Response(resp) => {
                            if let Some(pending) = pending_clone.lock().await.remove(&resp.id) {
                                let tx = pending.tx;
                                let _ = tx.send(resp);
                            }
                        }
                        JsonRpcMessage::Notification(notif) => {
                            debug!(method = %notif.method, "MCP notification received");
                            if notif.method == "notifications/tools/list_changed" {
                                let handler = tool_handler_clone.lock().await.clone();
                                if let Some(handler) = handler {
                                    handler();
                                }
                            }
                        }
                        JsonRpcMessage::Request(req) => {
                            debug!(method = %req.method, "MCP server request received");
                            let handler = handler_clone.lock().await.clone();
                            if let Some(handler) = handler {
                                // Route server-initiated requests (elicitation/
                                // create, sampling/create…) to the registered
                                // handler. The handler may wait on the USER
                                // (elicitation dialog) — spawn it so the
                                // reader keeps draining stdout. Otherwise one
                                // pending user prompt would stall every other
                                // in-flight request until the 60s timeout.
                                let stdin = stdin_clone.clone();
                                tokio::spawn(async move {
                                    let id = req.id;
                                    if let Some(result) = handler(req).await {
                                        let response = JsonRpcResponse {
                                            jsonrpc: "2.0".to_string(),
                                            id,
                                            result: Some(result),
                                            error: None,
                                        };
                                        if let Ok(payload) = serde_json::to_string(&response) {
                                            let mut stdin = stdin.lock().await;
                                            let _ = stdin.write_all(payload.as_bytes()).await;
                                            let _ = stdin.write_all(b"\n").await;
                                            let _ = stdin.flush().await;
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
            }

            info!("MCP stdio reader task ended");
            connected_clone.store(false, Ordering::Relaxed);
            // The child is gone — no response will ever arrive for the
            // in-flight requests. Fail them all so waiting callers resolve
            // instead of hanging forever on a dead transport.
            pending_clone.lock().await.clear();
        });

        Ok(Self {
            child: Arc::new(Mutex::new(Some(child))),
            stdin: stdin_shared,
            pending,
            next_id,
            connected,
            stderr_tail,
            server_request_handler,
            tool_list_changed_handler,
        })
    }
}

/// Maximum stderr bytes retained for the channel-closed diagnostic.
const STDERR_TAIL_CAP: usize = 4_000;
/// Maximum length of a single stdio JSON-RPC line we will parse.
const MAX_STDIO_LINE: usize = 8 * 1024 * 1024;

/// Append a stderr line to the capped tail sink, keeping only the MOST
/// RECENT output (the crash reason is at the end of the traceback).
fn capture_stderr_tail(sink: &mut String, line: &str, cap: usize) {
    sink.push_str(line);
    sink.push('\n');
    if sink.len() <= cap {
        return;
    }
    let overflow = sink.len() - cap;
    let mut start = overflow;
    while !sink.is_char_boundary(start) {
        start += 1;
    }
    sink.drain(..start);
}

/// Build the channel-closed error message, appending the server's stderr
/// tail when there is one — the difference between "server died, here is
/// why" and an opaque "MCP response channel closed".
fn channel_closed_message(method: &str, stderr_tail: &str) -> String {
    let mut msg = format!(
        "MCP server closed the connection before answering '{method}' (response channel closed) — \
         the server process likely exited or was restarted"
    );
    let tail = stderr_tail.trim_end();
    if !tail.is_empty() {
        msg.push_str(&format!("\n\nServer stderr tail:\n{tail}"));
    }
    msg
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        // The child is already gone (reader task ended) — fail fast with
        // the crash reason instead of writing into a broken pipe and
        // returning a confusing "write failed".
        if !self.connected.load(Ordering::Relaxed) {
            let tail = self
                .stderr_tail
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            return Err(AppError::Mcp(channel_closed_message(method, &tail)));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params.clone());
        let message = serde_json::to_string(&request)?;

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(
            id,
            PendingRequest {
                method: method.to_string(),
                params: params.clone(),
                tx,
            },
        );

        {
            let mut stdin = self.stdin.lock().await;
            if let Err(e) = stdin.write_all(message.as_bytes()).await {
                // The request never reached the server — drop the pending
                // slot (a leaked oneshot sender wedges the entry forever).
                self.pending.lock().await.remove(&id);
                return Err(AppError::Mcp(format!("MCP write failed: {e}")));
            }
            if let Err(e) = stdin.write_all(b"\n").await {
                self.pending.lock().await.remove(&id);
                return Err(AppError::Mcp(format!("MCP write failed: {e}")));
            }
            if let Err(e) = stdin.flush().await {
                self.pending.lock().await.remove(&id);
                return Err(AppError::Mcp(format!("MCP flush failed: {e}")));
            }
        }

        debug!(method = %method, id, "MCP request sent");

        let response = match tokio::time::timeout(std::time::Duration::from_secs(60), rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => {
                // Channel closed (reader drained pending on child exit).
                self.pending.lock().await.remove(&id);
                let tail = self
                    .stderr_tail
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                return Err(AppError::Mcp(channel_closed_message(method, &tail)));
            }
            Err(_) => {
                // Timeout — the pending oneshot must be released, otherwise
                // every timed-out call leaks an entry in the map forever.
                self.pending.lock().await.remove(&id);
                return Err(AppError::Mcp(format!("MCP request timeout: {}", method)));
            }
        };

        if let Some(err) = response.error {
            return Err(AppError::Mcp(format!(
                "MCP error [{}]: {}",
                err.code, err.message
            )));
        }

        response
            .result
            .ok_or_else(|| AppError::Mcp("MCP response has no result".into()))
    }

    async fn request_stateless(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        // stdio has no HTTP routing headers; the stateless protocol only
        // adds the self-describing `_meta` envelope to the JSON-RPC params.
        self.request(method, with_stateless_meta(params)).await
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> AppResult<()> {
        let notif = crate::mcp::types::JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };
        let message = serde_json::to_string(&notif)?;

        let mut stdin = self.stdin.lock().await;
        stdin.write_all(message.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;

        debug!(method = %method, "MCP notification sent");
        Ok(())
    }

    async fn close(&self) -> AppResult<()> {
        self.connected.store(false, Ordering::Relaxed);
        // Drop every pending sender so any waiting caller resolves with a
        // channel-closed error instead of hanging on the closing transport.
        let pending = std::mem::take(&mut *self.pending.lock().await);
        drop(pending);
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
        }
        Ok(())
    }

    async fn set_server_request_handler(&self, handler: ServerRequestHandler) {
        *self.server_request_handler.lock().await = Some(handler);
    }

    async fn set_tool_list_changed_handler(&self, handler: ToolListChangedHandler) {
        *self.tool_list_changed_handler.lock().await = Some(handler);
    }
}

/// HTTP transport — sends requests over HTTP POST.
pub struct HttpTransport {
    url: String,
    client: reqwest::Client,
    next_id: Arc<AtomicU64>,
    connected: Arc<std::sync::atomic::AtomicBool>,
    /// Optional OAuth bearer token attached to every request.
    bearer_token: Option<String>,
    /// Bounded concurrent in-flight requests per server.
    semaphore: Arc<tokio::sync::Semaphore>,
}

/// Maximum concurrent HTTP requests per MCP server.
const HTTP_CONCURRENCY: usize = 8;
/// Extra retries after the initial attempt (1 + 2 = 3 total).
const HTTP_RETRIES: usize = 2;

/// Status codes safe to retry: rate-limited or server-side transient.
/// 4xx client errors are never retried (a bad payload won't fix itself),
/// and 408 timeouts are excluded because the server MAY have executed the
/// call (tools/call side effects must not run twice).
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504)
}

fn retry_delay(attempt: usize) -> std::time::Duration {
    let base_ms = 300u64 * (1 << attempt.min(3));
    let jitter = rand::random::<u64>() % (base_ms / 2);
    std::time::Duration::from_millis(base_ms + jitter)
}

/// Exponential backoff for SSE reconnects (500ms → 30s cap) + jitter.
fn sse_reconnect_delay(attempt: u32) -> std::time::Duration {
    let base_ms = 500u64 * (1 << attempt.min(5));
    let base_ms = base_ms.min(30_000);
    let jitter = rand::random::<u64>() % (base_ms / 2);
    std::time::Duration::from_millis(base_ms + jitter)
}

impl HttpTransport {
    /// Create an HTTP transport that attaches an OAuth bearer token.
    pub fn with_bearer(url: impl Into<String>, bearer_token: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_default();

        Self {
            url: url.into(),
            client,
            next_id: Arc::new(AtomicU64::new(1)),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            bearer_token,
            semaphore: Arc::new(tokio::sync::Semaphore::new(HTTP_CONCURRENCY)),
        }
    }

    /// Build the POST request with stateless routing headers + bearer.
    fn build_request(
        &self,
        payload: &serde_json::Value,
        stateless: bool,
        method: &str,
        idempotency_key: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let mut builder = self.client.post(&self.url).json(payload);
        if let Some(key) = idempotency_key {
            // Same key across retries so the server can dedupe an already
            // executed call.
            builder = builder.header("Idempotency-Key", key);
        }
        if stateless {
            let name = mcp_name_header(method, payload.get("params"));
            builder = builder
                .header("Mcp-Method", method)
                .header("Mcp-Name", name)
                .header("MCP-Protocol-Version", MCP_PROTOCOL_VERSION_2026_07_28);
        }
        if let Some(ref token) = self.bearer_token {
            builder = builder.bearer_auth(token);
        }
        builder
    }

    /// Send with bounded concurrency and limited semantic retries.
    ///
    /// A retry can DOUBLE-EXECUTE a side-effecting call: `tools/call`
    /// (create_order), `resources/read`, `resources/subscribe` may already
    /// have been processed by the server before the response was lost (a
    /// proxy 502, a connection reset mid-response). The Idempotency-Key
    /// header is set, but standard MCP servers do not honor it, so it is NOT
    /// protection. Only idempotent discovery/handshake methods and
    /// fire-and-forget notifications are re-sent on transient errors — the
    /// same policy the SSE reconnect path uses (`is_idempotent_method`).
    /// Everything else fails immediately on the first error.
    async fn send_retrying(
        &self,
        payload: &serde_json::Value,
        stateless: bool,
        method: &str,
    ) -> AppResult<reqwest::Response> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| AppError::Mcp("MCP HTTP concurrency limit reached".to_string()))?;
        let idempotency_key = uuid::Uuid::new_v4().to_string();
        let retryable = is_idempotent_method(method) || method.starts_with("notifications/");
        let mut last: Option<AppError> = None;
        for attempt in 0..=HTTP_RETRIES {
            match self
                .build_request(payload, stateless, method, Some(&idempotency_key))
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    if !status.is_success() {
                        let body = read_limited_text(response, 4096).await;
                        if retryable && is_retryable_status(status) && attempt < HTTP_RETRIES {
                            last = Some(AppError::Mcp(format!("HTTP {status}: {body}")));
                            tokio::time::sleep(retry_delay(attempt)).await;
                            continue;
                        }
                        return Err(AppError::Mcp(format!("HTTP {status}: {body}")));
                    }
                    return Ok(response);
                }
                Err(e) => {
                    if retryable && attempt < HTTP_RETRIES {
                        last = Some(AppError::Mcp(format!("HTTP request failed: {e}")));
                        tokio::time::sleep(retry_delay(attempt)).await;
                        continue;
                    }
                    return Err(AppError::Mcp(format!(
                        "HTTP request failed after {} retries: {e}",
                        HTTP_RETRIES
                    )));
                }
            }
        }
        Err(last.unwrap_or_else(|| AppError::Mcp("HTTP request failed".to_string())))
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    fn is_http_like(&self) -> bool {
        true
    }

    async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.post(JsonRpcRequest::new(id, method, params), false)
            .await
    }

    async fn request_stateless(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let params = with_stateless_meta(params);
        self.post(JsonRpcRequest::new(id, method, params), true)
            .await
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> AppResult<()> {
        // HTTP notifications are fire-and-forget JSON-RPC POSTs — but they
        // must actually be SENT. The legacy handshake's
        // `notifications/initialized` is a real protocol step; silently
        // no-op'ing it left strict HTTP servers half-initialized.
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };
        let payload = serde_json::to_value(&notif)?;
        let _ = self.send_retrying(&payload, false, method).await?;
        Ok(())
    }

    async fn close(&self) -> AppResult<()> {
        self.connected.store(false, Ordering::Relaxed);
        Ok(())
    }
}

impl HttpTransport {
    /// POST a JSON-RPC request and parse the response. In stateless mode the
    /// 2026-07-28 routing headers are attached so gateways can route and
    /// meter without parsing bodies.
    async fn post(&self, request: JsonRpcRequest, stateless: bool) -> AppResult<serde_json::Value> {
        let payload = serde_json::to_value(&request)?;
        let method = request.method.clone();
        let response = self.send_retrying(&payload, stateless, &method).await?;

        let json: JsonRpcResponse = response.json().await?;

        if let Some(err) = json.error {
            return Err(AppError::Mcp(format!(
                "MCP error [{}]: {}",
                err.code, err.message
            )));
        }

        json.result
            .ok_or_else(|| AppError::Mcp("MCP response has no result".into()))
    }
}

/// One Server-Sent Events dispatch — `event` defaults to `message` when the
/// stream omits it (per the SSE spec), `data` is the joined payload.
#[derive(Debug, PartialEq)]
pub(crate) struct SseEvent {
    pub event: String,
    pub data: String,
}

/// Incremental SSE parser — feeds byte chunks, yields complete events.
/// Handles CRLF/LF, multi-line `data:` (joined with `\n`), comments, and an
/// empty event name defaulting to `message`.
pub(crate) struct SseParser {
    buffer: Vec<u8>,
    event: String,
    data_lines: Vec<String>,
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            event: String::new(),
            data_lines: Vec::new(),
        }
    }

    /// Feed a chunk of bytes; returns any complete events terminated by a
    /// blank line.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        // Defend against a stream that never sends newlines: cap the
        // pending buffer and drop the run until the next line boundary.
        if self.buffer.len() >= MAX_SSE_BUFFER {
            tracing::warn!(
                bytes = self.buffer.len(),
                "SSE buffer exceeded cap — discarding"
            );
            self.buffer.clear();
            self.event.clear();
            self.data_lines.clear();
        }
        self.buffer.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(nl) = self.buffer.iter().position(|&b| b == b'\n') {
            let raw_line: Vec<u8> = self.buffer.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&raw_line[..raw_line.len() - 1]);
            if let Some(ev) = self.process_line(&line) {
                events.push(ev);
            }
        }
        events
    }

    /// Flush a trailing event at end-of-stream (no blank-line terminator).
    pub fn finish(&mut self) -> Option<SseEvent> {
        if !self.buffer.is_empty() {
            let line = String::from_utf8_lossy(&std::mem::take(&mut self.buffer)).into_owned();
            let _ = self.process_line(&line);
        }
        self.finish_event()
    }

    /// Consume one line (without its `\n`); returns an event when the line
    /// was the blank terminator.
    fn process_line(&mut self, line: &str) -> Option<SseEvent> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.finish_event();
        }
        if line.starts_with(':') {
            // Comment — ignore (leading colon per SSE spec).
            return None;
        }
        if let Some(value) = line.strip_prefix("event:") {
            self.event = value.trim().to_string();
            return None;
        }
        if let Some(value) = line.strip_prefix("data:") {
            self.data_lines
                .push(value.trim_start_matches(' ').to_string());
            return None;
        }
        // `id:` / `retry:` are not needed for MCP transport — ignore.
        None
    }

    fn finish_event(&mut self) -> Option<SseEvent> {
        if self.data_lines.is_empty() && self.event.is_empty() {
            return None;
        }
        let event = if self.event.is_empty() {
            "message".to_string()
        } else {
            std::mem::take(&mut self.event)
        };
        let data = std::mem::take(&mut self.data_lines).join("\n");
        Some(SseEvent { event, data })
    }
}

/// Maximum bytes buffered between SSE line terminators before discarding.
const MAX_SSE_BUFFER: usize = 8 * 1024 * 1024;

/// SSE transport — the classic MCP Server-Sent Events transport.
///
/// The client opens a GET stream on the configured URL; the server's first
/// `endpoint` event names the POST endpoint for JSON-RPC messages.
/// Responses and server-initiated requests arrive back on the stream as
/// `message` events. This is the legacy session transport — the stateless
/// 2026-07-28 revision is HTTP-flavored and handled by [`HttpTransport`].
pub struct SseTransport {
    client: reqwest::Client,
    sse_url: reqwest::Url,
    /// POST endpoint once the server announces it (None until then).
    messages: Arc<tokio::sync::watch::Sender<Option<reqwest::Url>>>,
    pending: Arc<tokio::sync::Mutex<HashMap<u64, PendingRequest>>>,
    next_id: Arc<AtomicU64>,
    connected: Arc<std::sync::atomic::AtomicBool>,
    bearer_token: Option<String>,
    server_request_handler: Arc<tokio::sync::Mutex<Option<ServerRequestHandler>>>,
    tool_list_changed_handler: Arc<tokio::sync::Mutex<Option<ToolListChangedHandler>>>,
    reader: Arc<tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl SseTransport {
    /// Open the SSE stream. Returns once the GET response headers arrive
    /// (the `endpoint` event may still be in flight — `request` waits for
    /// it with a bounded timeout).
    pub async fn connect(url: &str, bearer_token: Option<String>) -> AppResult<Self> {
        let sse_url = reqwest::Url::parse(url)
            .map_err(|e| AppError::Mcp(format!("Invalid SSE URL '{url}': {e}")))?;
        // The SSE stream lives for the whole connection — no overall client
        // timeout (per-request waits are bounded by oneshot timeouts).
        let client = reqwest::Client::builder().build().unwrap_or_default();

        let mut builder = client
            .get(sse_url.clone())
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache");
        if let Some(ref token) = bearer_token {
            builder = builder.bearer_auth(token);
        }
        let response = builder.send().await?;
        if !response.status().is_success() {
            return Err(AppError::Mcp(format!(
                "SSE HTTP {}: {}",
                response.status(),
                read_limited_text(response, 4096).await
            )));
        }

        let (messages_tx, _) = tokio::sync::watch::channel(None);
        let transport = Self {
            client,
            sse_url,
            messages: Arc::new(messages_tx),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            bearer_token,
            server_request_handler: Arc::new(Mutex::new(None)),
            tool_list_changed_handler: Arc::new(Mutex::new(None)),
            reader: Arc::new(Mutex::new(None)),
        };

        let reader = transport.spawn_reader(response);
        *transport.reader.lock().await = Some(reader);
        Ok(transport)
    }

    fn spawn_reader(&self, response: reqwest::Response) -> tokio::task::JoinHandle<()> {
        let pending = self.pending.clone();
        let connected = self.connected.clone();
        let handler = self.server_request_handler.clone();
        let tool_handler = self.tool_list_changed_handler.clone();
        let client = self.client.clone();
        let bearer_token = self.bearer_token.clone();
        let messages_url = self.messages.clone();
        let sse_url = self.sse_url.clone();

        tokio::spawn(async move {
            let mut response = Some(response);
            // Transport-level reconnect: when the stream drops while the
            // transport is still open (close() not called), re-open the GET
            // with backoff instead of forcing the pool to tear down and
            // rebuild the whole client (tool registry churn). In-flight
            // requests keep their own 60s timeouts; the `endpoint` event is
            // re-announced on the new stream and refreshes the watch.
            loop {
                // `bytes_stream` consumes the response; an Option + take
                // lets the reconnect path hand back a fresh response without
                // the borrow checker losing track of the move.
                let mut stream = match response.take() {
                    Some(resp) => resp.bytes_stream(),
                    None => break,
                };
                let mut parser = SseParser::new();
                let mut stream_ended_cleanly = true;
                while let Some(chunk) = stream.next().await {
                    match chunk {
                        Ok(bytes) => {
                            for event in parser.push(&bytes) {
                                handle_sse_event(
                                    &event,
                                    &pending,
                                    &handler,
                                    &tool_handler,
                                    &client,
                                    &bearer_token,
                                    &messages_url,
                                    &sse_url,
                                )
                                .await;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "SSE stream read error");
                            stream_ended_cleanly = false;
                            break;
                        }
                    }
                }
                if stream_ended_cleanly {
                    if let Some(event) = parser.finish() {
                        handle_sse_event(
                            &event,
                            &pending,
                            &handler,
                            &tool_handler,
                            &client,
                            &bearer_token,
                            &messages_url,
                            &sse_url,
                        )
                        .await;
                    }
                }

                // close() was called — stop for good.
                if !connected.load(Ordering::Relaxed) {
                    break;
                }
                tracing::warn!("SSE stream ended — reconnecting");

                let mut attempt = 0u32;
                let mut reconnected = false;
                loop {
                    tokio::time::sleep(sse_reconnect_delay(attempt)).await;
                    if !connected.load(Ordering::Relaxed) {
                        break;
                    }
                    let mut builder = client
                        .get(sse_url.clone())
                        .header("Accept", "text/event-stream")
                        .header("Cache-Control", "no-cache");
                    if let Some(ref token) = bearer_token {
                        builder = builder.bearer_auth(token);
                    }
                    match builder.send().await {
                        Ok(resp) if resp.status().is_success() => {
                            response = Some(resp);
                            reconnected = true;
                            break;
                        }
                        Ok(resp) => {
                            tracing::warn!(status = %resp.status(), "SSE reconnect rejected");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "SSE reconnect failed");
                        }
                    }
                    attempt += 1;
                }
                if reconnected {
                    tracing::info!("SSE stream reconnected");
                    // Re-send idempotent in-flight calls on the fresh stream
                    // (same ids — the server dedupes or re-executes safely).
                    // Clone the endpoint OUT of the watch Ref first — the
                    // Ref is !Send and must not live across the await below.
                    let fresh_endpoint = messages_url.subscribe().borrow_and_update().clone();
                    if let Some(url) = fresh_endpoint {
                        resend_idempotent_pending(&pending, &url, &client, &bearer_token).await;
                    }
                    continue;
                }
                break;
            }
            connected.store(false, Ordering::Relaxed);
            // The stream is gone — every in-flight request must resolve
            // instead of hanging forever.
            pending.lock().await.clear();
        })
    }

    /// Resolve the POST endpoint, waiting (bounded) for the server's
    /// `endpoint` event.
    async fn messages_url(&self) -> AppResult<reqwest::Url> {
        let mut rx = self.messages.subscribe();
        if let Some(url) = rx.borrow_and_update().clone() {
            return Ok(url);
        }
        let wait = async {
            loop {
                if let Some(url) = rx.borrow_and_update().clone() {
                    return Ok(url);
                }
                rx.changed().await.map_err(|_| {
                    AppError::Mcp("SSE channel closed before endpoint event".into())
                })?;
            }
        };
        match tokio::time::timeout(std::time::Duration::from_secs(60), wait).await {
            Ok(Ok(url)) => Ok(url),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(AppError::Mcp(
                "SSE server did not announce an endpoint within 60s".into(),
            )),
        }
    }

    async fn post_json(&self, url: &reqwest::Url, body: &serde_json::Value) -> AppResult<()> {
        let mut builder = self.client.post(url.clone()).json(body);
        if let Some(ref token) = self.bearer_token {
            builder = builder.bearer_auth(token);
        }
        let response = builder.send().await?;
        if !response.status().is_success() {
            return Err(AppError::Mcp(format!(
                "SSE POST HTTP {}: {}",
                response.status(),
                read_limited_text(response, 4096).await
            )));
        }
        Ok(())
    }
}

/// Re-send idempotent in-flight requests after an SSE reconnect so they
/// resolve on the fresh stream instead of timing out. Side-effecting calls
/// (tools/call, resources/read) are left alone — the server may have
/// already executed them before the stream dropped.
async fn resend_idempotent_pending(
    pending: &tokio::sync::Mutex<HashMap<u64, PendingRequest>>,
    url: &reqwest::Url,
    client: &reqwest::Client,
    bearer_token: &Option<String>,
) {
    let ids: Vec<u64> = pending.lock().await.keys().copied().collect();
    for id in ids {
        let (method, params) = {
            let guard = pending.lock().await;
            match guard.get(&id) {
                Some(p) if is_idempotent_method(&p.method) => (p.method.clone(), p.params.clone()),
                _ => continue,
            }
        };
        let request = JsonRpcRequest::new(id, &method, params);
        let mut builder = client.post(url.clone()).json(&request);
        if let Some(ref token) = bearer_token {
            builder = builder.bearer_auth(token);
        }
        match builder.send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::debug!(id, method = %method, "Re-sent idempotent MCP request after SSE reconnect");
            }
            Ok(resp) => {
                tracing::warn!(id, method = %method, status = %resp.status(), "Idempotent resend rejected");
            }
            Err(e) => {
                tracing::warn!(id, method = %method, error = %e, "Idempotent resend failed");
            }
        }
    }
}

/// Dispatch one parsed SSE event: `endpoint` announces the POST URL,
/// `message` carries JSON-RPC traffic.
#[allow(clippy::too_many_arguments)]
async fn handle_sse_event(
    event: &SseEvent,
    pending: &tokio::sync::Mutex<HashMap<u64, PendingRequest>>,
    handler: &tokio::sync::Mutex<Option<ServerRequestHandler>>,
    tool_handler: &tokio::sync::Mutex<Option<ToolListChangedHandler>>,
    client: &reqwest::Client,
    bearer_token: &Option<String>,
    messages: &tokio::sync::watch::Sender<Option<reqwest::Url>>,
    sse_url: &reqwest::Url,
) {
    if event.event == "endpoint" {
        let resolved = reqwest::Url::parse(&event.data)
            .or_else(|_| sse_url.join(&event.data))
            .ok();
        if let Some(url) = resolved {
            let _ = messages.send(Some(url));
        } else {
            tracing::warn!(data = %event.data, "SSE endpoint event not parseable");
        }
        return;
    }
    if event.event != "message" {
        tracing::debug!(event = %event.event, "Ignoring SSE event");
        return;
    }
    let Ok(msg) = serde_json::from_str::<JsonRpcMessage>(&event.data) else {
        tracing::debug!(data = %event.data, "SSE message not JSON-RPC");
        return;
    };
    match msg {
        JsonRpcMessage::Response(resp) => {
            if let Some(pending) = pending.lock().await.remove(&resp.id) {
                let tx = pending.tx;
                let _ = tx.send(resp);
            }
        }
        JsonRpcMessage::Notification(notif) => {
            tracing::debug!(method = %notif.method, "SSE notification received");
            if notif.method == "notifications/tools/list_changed" {
                let handler = tool_handler.lock().await.clone();
                if let Some(handler) = handler {
                    handler();
                }
            }
        }
        JsonRpcMessage::Request(req) => {
            let handler = handler.lock().await.clone();
            if let Some(handler) = handler {
                // Same non-blocking rule as stdio: the handler may wait on
                // the user, so process it on a separate task and keep the
                // SSE stream draining (responses arrive on the same stream).
                let client = client.clone();
                let bearer_token = bearer_token.clone();
                let messages = messages.clone();
                tokio::spawn(async move {
                    let id = req.id;
                    if let Some(result) = handler(req).await {
                        let response = JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            id,
                            result: Some(result),
                            error: None,
                        };
                        let messages_url = messages.subscribe().borrow_and_update().clone();
                        if let Some(url) = messages_url {
                            let mut builder = client.post(url).json(&response);
                            if let Some(ref token) = bearer_token {
                                builder = builder.bearer_auth(token);
                            }
                            if let Err(e) = builder.send().await {
                                tracing::warn!(error = %e, "SSE server request response POST failed");
                            }
                        }
                    }
                });
            }
        }
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    fn is_http_like(&self) -> bool {
        // SSE is the legacy session transport: probing with the stateless
        // `server/discover` POST would confuse strict servers — go straight
        // to the classic initialize handshake (same policy as stdio).
        false
    }

    async fn request(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        if !self.connected.load(Ordering::Relaxed) {
            return Err(AppError::Mcp(format!(
                "MCP SSE connection closed before answering '{method}'"
            )));
        }
        let messages_url = self.messages_url().await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let request = JsonRpcRequest::new(id, method, params.clone());
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(
            id,
            PendingRequest {
                method: method.to_string(),
                params: params.clone(),
                tx,
            },
        );

        let outcome = tokio::time::timeout(std::time::Duration::from_secs(60), async {
            self.post_json(&messages_url, &serde_json::to_value(&request)?)
                .await?;
            rx.await.map_err(|_| {
                AppError::Mcp(format!(
                    "MCP SSE channel closed before answering '{method}'"
                ))
            })
        })
        .await;

        let response = match outcome {
            Ok(Ok(resp)) => resp,
            Ok(Err(e)) => {
                self.pending.lock().await.remove(&id);
                return Err(e);
            }
            Err(_) => {
                self.pending.lock().await.remove(&id);
                return Err(AppError::Mcp(format!("MCP SSE request timeout: {method}")));
            }
        };
        self.pending.lock().await.remove(&id);

        if let Some(err) = response.error {
            return Err(AppError::Mcp(format!(
                "MCP error [{}]: {}",
                err.code, err.message
            )));
        }
        response
            .result
            .ok_or_else(|| AppError::Mcp("MCP response has no result".into()))
    }

    async fn notify(&self, method: &str, params: Option<serde_json::Value>) -> AppResult<()> {
        let messages_url = self.messages_url().await?;
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };
        self.post_json(&messages_url, &serde_json::to_value(&notif)?)
            .await
    }

    async fn close(&self) -> AppResult<()> {
        self.connected.store(false, Ordering::Relaxed);
        let pending = std::mem::take(&mut *self.pending.lock().await);
        drop(pending);
        if let Some(reader) = self.reader.lock().await.take() {
            reader.abort();
        }
        Ok(())
    }

    async fn set_server_request_handler(&self, handler: ServerRequestHandler) {
        *self.server_request_handler.lock().await = Some(handler);
    }

    async fn set_tool_list_changed_handler(&self, handler: ToolListChangedHandler) {
        *self.tool_list_changed_handler.lock().await = Some(handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stateless_meta_preserves_existing_params() {
        let params = serde_json::json!({ "name": "search", "arguments": { "q": "x" } });
        let wrapped = with_stateless_meta(Some(params)).expect("params stay present");
        assert_eq!(wrapped["name"], "search");
        let meta = wrapped["_meta"].as_object().expect("_meta injected");
        assert_eq!(meta[META_PROTOCOL_VERSION], MCP_PROTOCOL_VERSION_2026_07_28);
        assert!(meta[META_CLIENT_INFO]["name"].is_string());
        assert!(meta[META_CLIENT_CAPABILITIES].is_object());
    }

    #[test]
    fn stateless_meta_creates_envelope_for_null_params() {
        let wrapped = with_stateless_meta(None).expect("envelope created");
        assert!(wrapped["_meta"][META_PROTOCOL_VERSION].is_string());
    }

    #[test]
    fn mcp_name_header_uses_tool_name_for_call() {
        let params = serde_json::json!({ "name": "make_dashboard", "arguments": {} });
        assert_eq!(
            mcp_name_header("tools/call", Some(&params)),
            "make_dashboard"
        );
        assert_eq!(mcp_name_header("tools/list", None), "tools/list");
    }

    #[test]
    fn stateless_fallback_error_classification() {
        assert!(is_stateless_fallback_error(
            "MCP error [-32601]: Method not found"
        ));
        assert!(is_stateless_fallback_error(
            "MCP error [-32022]: UnsupportedProtocolVersionError"
        ));
        assert!(is_stateless_fallback_error(
            "MCP error [-32004]: Unsupported protocol version"
        ));
        assert!(is_stateless_fallback_error("HTTP 405: Method Not Allowed"));
        assert!(!is_stateless_fallback_error(
            "MCP error [-32602]: Invalid params"
        ));
        assert!(!is_stateless_fallback_error("HTTP 401: Unauthorized"));
    }

    #[test]
    fn sse_parser_discards_unbounded_newline_less_run() {
        let mut parser = SseParser::new();
        // Feed over the cap without any newline — the parser must drop the
        // runaway run instead of growing the buffer forever.
        let blob = vec![b'x'; MAX_SSE_BUFFER];
        let events = parser.push(&blob);
        assert!(events.is_empty());

        // A normal event afterwards still parses.
        let events = parser.push(b"event: message\ndata: {\"ok\":true}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "message");
        assert!(events[0].data.contains("ok"));
    }

    #[test]
    fn http_retry_status_classification() {
        assert!(is_retryable_status(reqwest::StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(is_retryable_status(reqwest::StatusCode::BAD_GATEWAY));
        assert!(is_retryable_status(
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(is_retryable_status(reqwest::StatusCode::GATEWAY_TIMEOUT));
        // Client errors and 408 must NOT be retried (side effects may have
        // run / a bad payload won't fix itself).
        assert!(!is_retryable_status(reqwest::StatusCode::BAD_REQUEST));
        assert!(!is_retryable_status(reqwest::StatusCode::UNAUTHORIZED));
        assert!(!is_retryable_status(reqwest::StatusCode::NOT_FOUND));
        assert!(!is_retryable_status(reqwest::StatusCode::REQUEST_TIMEOUT));
    }

    /// A minimal HTTP server that responds to each accepted connection with
    /// the given statuses (in order). Returns the base URL and a shared count
    /// of requests actually served.
    async fn spawn_status_server(
        statuses: Vec<u16>,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let url = format!("http://{}", listener.local_addr().expect("addr"));
        let served = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let served_clone = served.clone();
        tokio::spawn(async move {
            for status in statuses {
                let (mut sock, _) = listener.accept().await.expect("accept");
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                let mut body_len: Option<usize> = None;
                loop {
                    if let Some(len) = body_len {
                        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            if buf.len() >= end + 4 + len {
                                break;
                            }
                        }
                    }
                    let n = sock.read(&mut tmp).await.expect("read");
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if body_len.is_none() {
                        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&buf[..end]);
                            body_len = Some(
                                headers
                                    .lines()
                                    .find_map(|l| {
                                        l.to_ascii_lowercase()
                                            .strip_prefix("content-length:")
                                            .and_then(|v| v.trim().parse::<usize>().ok())
                                    })
                                    .unwrap_or(0),
                            );
                        }
                    }
                }
                served_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let body = if status == 200 {
                    r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#
                } else {
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"boom"}}"#
                };
                let resp = format!(
                    "HTTP/1.1 {status} {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    if status == 200 { "OK" } else { "Error" },
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        (url, served)
    }

    #[tokio::test]
    async fn http_transport_does_not_retry_side_effecting_methods() {
        // tools/call (create_order) must NOT be re-sent after a 502 — the
        // server may have already executed the call, so a retry would create
        // the order twice. Idempotency-Key is not honored by standard MCP
        // servers, so the retry gate is the real protection.
        let (url, served) = spawn_status_server(vec![502, 502, 502]).await;
        let transport = HttpTransport::with_bearer(url, None);

        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": "create_order", "arguments": {} }
        });
        let result = transport
            .send_retrying(&payload, false, "tools/call")
            .await;
        assert!(
            result.is_err(),
            "tools/call must fail immediately, not retry a possibly-executed call"
        );
        assert_eq!(
            served.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "side-effecting tools/call must be sent exactly ONCE"
        );
    }

    #[tokio::test]
    async fn http_transport_retries_idempotent_methods() {
        // ping is idempotent — a 502 then a 200 succeeds via one retry.
        let (url, served) = spawn_status_server(vec![502, 200]).await;
        let transport = HttpTransport::with_bearer(url, None);
        let payload = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
        let result = transport.send_retrying(&payload, false, "ping").await;
        assert!(result.is_ok(), "idempotent ping retries through a 502");
        assert_eq!(served.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[test]
    fn retry_delays_grow_and_stay_bounded() {
        let d0 = retry_delay(0).as_millis();
        let d1 = retry_delay(1).as_millis();
        let d2 = retry_delay(2).as_millis();
        assert!((300..600).contains(&d0));
        assert!((600..1200).contains(&d1));
        assert!(d2 >= 1200);
        let big = sse_reconnect_delay(20).as_millis();
        assert!(big <= 45_000, "SSE backoff must stay bounded");
    }

    #[test]
    fn stderr_tail_keeps_most_recent_output() {
        let mut sink = String::new();
        // Under the cap: everything is kept.
        capture_stderr_tail(&mut sink, "Traceback (most recent call last):", 4000);
        capture_stderr_tail(&mut sink, "ModuleNotFoundError: No module named 'x'", 4000);
        assert!(sink.contains("ModuleNotFoundError"));
        assert!(sink.contains("Traceback"));

        // Over the cap: only the RECENT tail survives — the crash reason is
        // at the end of the traceback, so the front is dropped.
        let mut big = String::new();
        for i in 0..100 {
            capture_stderr_tail(&mut big, &format!("line {i:03} of noise"), 300);
        }
        assert!(big.len() <= 400, "tail stays bounded: {}", big.len());
        assert!(!big.contains("line 000"), "oldest output dropped");
        assert!(big.contains("line 099"), "newest output kept");
    }

    #[test]
    fn channel_closed_message_carries_the_crash_reason() {
        let msg = channel_closed_message(
            "initialize",
            "Traceback (most recent call last):\nModuleNotFoundError: No module named 'mcp'\n",
        );
        assert!(msg.contains("initialize"));
        assert!(msg.contains("response channel closed"));
        assert!(
            msg.contains("ModuleNotFoundError"),
            "stderr tail surfaces: {msg}"
        );

        // A clean close (no stderr) stays concise and still explains itself.
        let clean = channel_closed_message("tools/list", "");
        assert!(clean.contains("tools/list"));
        assert!(!clean.contains("stderr tail"));
    }

    #[tokio::test]
    async fn http_notify_actually_posts_json_rpc() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");

        let server = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            let mut body_len: Option<usize> = None;
            loop {
                if let Some(len) = body_len {
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        if buf.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let n = sock.read(&mut tmp).await.expect("read");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if body_len.is_none() {
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&buf[..end]);
                        body_len = Some(
                            headers
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .and_then(|v| v.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0),
                        );
                    }
                }
            }
            let end = buf
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("header end");
            let body = String::from_utf8_lossy(&buf[end + 4..]).to_string();
            let _ = sock
                .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                .await;
            body
        });

        let transport = HttpTransport::with_bearer(format!("http://{addr}"), None);
        transport
            .notify("notifications/initialized", None)
            .await
            .expect("notify succeeds");

        let body = server.await.expect("server task");
        assert!(
            body.contains("\"method\":\"notifications/initialized\""),
            "notification method posted: {body}"
        );
        assert!(
            body.contains("\"jsonrpc\":\"2.0\""),
            "jsonrpc envelope: {body}"
        );
    }

    #[test]
    fn sse_parser_handles_multiline_data_and_crlf() {
        let mut parser = SseParser::new();
        let events = parser.push(
            b"event: endpoint\ndata: http://x/messages\r\n\r\n\
              event: message\r\ndata: {\"jsonrpc\":\"2.0\"}\r\ndata: ,\"id\":1}\r\n\r\n",
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, "endpoint");
        assert_eq!(events[0].data, "http://x/messages");
        assert_eq!(events[1].event, "message");
        assert_eq!(events[1].data, "{\"jsonrpc\":\"2.0\"}\n,\"id\":1}");
    }

    #[test]
    fn sse_parser_defaults_to_message_and_ignores_comments() {
        let mut parser = SseParser::new();
        let events = parser.push(b": a comment\n\ndata: hello\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "message");
        assert_eq!(events[0].data, "hello");
        assert!(parser.finish().is_none());
    }

    #[test]
    fn sse_parser_flushes_trailing_event_without_blank_line() {
        let mut parser = SseParser::new();
        parser.push(b"event: message\ndata: tail");
        let ev = parser.finish().expect("trailing event flushed");
        assert_eq!(ev.event, "message");
        assert_eq!(ev.data, "tail");
    }

    #[tokio::test]
    async fn sse_transport_legacy_roundtrip_over_local_servers() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let sse_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind sse");
        let sse_addr = sse_listener.local_addr().expect("sse addr");
        let msg_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind msg");
        let msg_addr = msg_listener.local_addr().expect("msg addr");

        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();

        // SSE side: accept GET, announce the endpoint, then deliver the
        // JSON-RPC response produced by the POST side.
        let sse_task = tokio::spawn(async move {
            let (mut sock, _) = sse_listener.accept().await.expect("sse accept");
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = sock.read(&mut tmp).await.expect("read");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = sock
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                      Cache-Control: no-cache\r\n\r\n",
                )
                .await;
            let _ = sock
                .write_all(
                    format!("event: endpoint\ndata: http://{msg_addr}/messages\n\n").as_bytes(),
                )
                .await;
            let _ = sock.flush().await;
            let resp = resp_rx.await.expect("response produced");
            let _ = sock
                .write_all(format!("event: message\ndata: {resp}\n\n").as_bytes())
                .await;
            let _ = sock.flush().await;
            // Keep the stream open until the test tears down.
            let mut sink = [0u8; 1024];
            while sock.read(&mut sink).await.unwrap_or(0) > 0 {}
        });

        // POST side: read the JSON-RPC request, answer with a canned result.
        let msg_task = tokio::spawn(async move {
            let (mut sock, _) = msg_listener.accept().await.expect("msg accept");
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let mut body_len = None;
            loop {
                if let Some(len) = body_len {
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        if buf.len() >= end + 4 + len {
                            break;
                        }
                    }
                }
                let n = sock.read(&mut tmp).await.expect("read");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if body_len.is_none() {
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&buf[..end]);
                        body_len = Some(
                            headers
                                .lines()
                                .find_map(|l| {
                                    l.to_ascii_lowercase()
                                        .strip_prefix("content-length:")
                                        .and_then(|v| v.trim().parse::<usize>().ok())
                                })
                                .unwrap_or(0),
                        );
                    }
                }
            }
            let end = buf
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .expect("header end");
            let body = String::from_utf8_lossy(&buf[end + 4..]).to_string();
            let request: serde_json::Value = serde_json::from_str(&body).expect("json-rpc body");
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": { "protocolVersion": "2024-11-05" }
            });
            let _ = resp_tx.send(response.to_string());
            let _ = sock
                .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                .await;
        });

        let transport = SseTransport::connect(&format!("http://{sse_addr}/sse"), None)
            .await
            .expect("connect");

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(15),
            transport.request(
                "initialize",
                Some(serde_json::json!({ "protocolVersion": "2024-11-05" })),
            ),
        )
        .await
        .expect("request resolves in time")
        .expect("request succeeds");

        assert_eq!(result["protocolVersion"], "2024-11-05");
        sse_task.abort();
        let _ = msg_task.await;
    }

    #[tokio::test]
    async fn sse_tool_list_changed_notification_fires_handler() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let sse_listener = TcpListener::bind("127.0.0.1:0").await.expect("bind sse");
        let sse_addr = sse_listener.local_addr().expect("sse addr");

        let sse_task = tokio::spawn(async move {
            let (mut sock, _) = sse_listener.accept().await.expect("sse accept");
            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                let n = sock.read(&mut tmp).await.expect("read");
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let _ = sock
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                      Cache-Control: no-cache\r\n\r\n",
                )
                .await;
            let _ = sock
                .write_all(b"event: endpoint\ndata: http://127.0.0.1:1/messages\n\n")
                .await;
            let _ = sock.flush().await;
            // Give the test time to install the handler before the
            // notification lands (the reader would otherwise consume it
            // with no handler registered).
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            let _ = sock
                .write_all(
                    b"event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n",
                )
                .await;
            let _ = sock.flush().await;
            let mut sink = [0u8; 1024];
            while sock.read(&mut sink).await.unwrap_or(0) > 0 {}
        });

        let transport = SseTransport::connect(&format!("http://{sse_addr}/sse"), None)
            .await
            .expect("sse connect");

        let fired = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let fired_clone = fired.clone();
        transport
            .set_tool_list_changed_handler(Arc::new(move || {
                fired_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }))
            .await;

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while fired.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "tools/list_changed handler never fired"
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(fired.load(std::sync::atomic::Ordering::Relaxed), 1);

        transport.close().await.expect("close");
        sse_task.abort();
    }
}
