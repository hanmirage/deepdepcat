//! MCP client — manages communication with a single MCP server.
//!
//! Handles:
//! - Server initialization handshake
//! - Tool discovery (tools/list)
//! - Resource discovery (resources/list)
//! - Tool execution (tools/call)
//! - Prompt retrieval (prompts/get)

use crate::core::error::{AppError, AppResult};
use crate::mcp::transport::{
    is_stateless_fallback_error, HttpTransport, McpProtocol, McpTransport, SseTransport,
    StdioTransport, MCP_PROTOCOL_VERSION_LEGACY,
};
use crate::mcp::types::{McpPrompt, McpResource, McpTool};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::info;

/// Extract a human-readable hint from an MRTR `input_required` payload so
/// the agent can adjust its arguments instead of seeing an opaque error.
fn input_required_detail(result: &serde_json::Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for key in ["title", "description", "prompt", "message"] {
        if let Some(s) = result
            .get("input")
            .and_then(|i| i.get(key))
            .and_then(|v| v.as_str())
        {
            if !s.trim().is_empty() {
                parts.push(format!("{key}: {s}"));
            }
        }
    }
    if parts.is_empty() {
        if let Some(s) = result
            .get("input")
            .and_then(|i| i.get("schema"))
            .and_then(|v| v.as_str())
        {
            parts.push(format!("schema: {s}"));
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" Server says: {}", parts.join(" | "))
    }
}

/// Provider that collects user input for an MRTR `input_required` request.
/// Receives the server's prompt text and (optional) input schema; returns
/// the user's value, or `None` when the user cancelled / timed out.
pub type InputProvider = dyn Fn(
        String,
        Option<serde_json::Value>,
        Option<String>,
    ) -> Pin<Box<dyn Future<Output = Option<serde_json::Value>> + Send>>
    + Send
    + Sync;

/// Human-readable prompt for the input dialog.
fn input_required_prompt(result: &serde_json::Value) -> String {
    for key in ["description", "prompt", "title", "message"] {
        if let Some(s) = result
            .get("input")
            .and_then(|i| i.get(key))
            .and_then(|v| v.as_str())
        {
            if !s.trim().is_empty() {
                return s.trim().to_string();
            }
        }
    }
    "MCP 服务器请求补充输入".to_string()
}

/// Merge the collected user input into the request params.
fn inject_input(params: Option<Value>, input: Value) -> Option<Value> {
    match params {
        Some(Value::Object(mut map)) => {
            map.insert("input".to_string(), input);
            Some(Value::Object(map))
        }
        other => Some(serde_json::json!({ "input": input, "params": other })),
    }
}

/// A UI payload for the MCP Apps extension — an interactive HTML page the
/// server exposes via a `ui://` resource attached to a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpAppPayload {
    /// The `ui://` resource URI the HTML was fetched from.
    pub resource_uri: String,
    /// The rendered HTML document (fetched via `resources/read`).
    pub html: String,
    /// True when the tool call itself failed (the app is still shown).
    pub is_error: bool,
    /// CSP domains declared by the server via `_meta.ui.csp` (MCP Apps
    /// spec) — the host injects them into the sandboxed document. `None`
    /// means "restrictive default" (no external origins allowed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub csp: Option<Value>,
}

/// The outcome of a `tools/call` — the model-visible rendered text plus an
/// optional MCP Apps UI payload.
#[derive(Debug)]
pub struct CallToolOutcome {
    pub content: String,
    pub is_error: bool,
    /// Interactive UI (MCP Apps) when the server attached a `ui://` resource
    /// to the result.
    pub app: Option<McpAppPayload>,
}

/// Max HTML size for a rendered MCP App (a defensive cap — an oversized
/// payload is dropped instead of rendered).
pub const MAX_MCP_APP_HTML_BYTES: usize = 5 * 1024 * 1024;

/// The MCP Apps extension is transport-independent (postMessage), but the
/// resource itself is fetched with a plain `resources/read` over the
/// server's transport — with the `ui://` scheme.
const UI_SCHEME: &str = "ui://";

/// An MCP client — wraps a transport and provides high-level methods.
pub struct McpClient {
    transport: Arc<dyn McpTransport>,
    /// Tools exposed by the server — behind a lock so `tools/list_changed`
    /// notifications can hot-refresh the registry while tool wrappers keep
    /// holding the shared `Arc<McpClient>`.
    tools: std::sync::RwLock<Vec<McpTool>>,
    resources: Vec<McpResource>,
    prompts: Vec<McpPrompt>,
    name: String,
    protocol: McpProtocol,
    input_provider: std::sync::RwLock<Option<Arc<InputProvider>>>,
}

#[cfg(test)]
impl Default for McpClient {
    fn default() -> Self {
        Self {
            transport: Arc::new(HttpTransport::with_bearer("http://127.0.0.1:1", None)),
            tools: std::sync::RwLock::new(vec![]),
            resources: vec![],
            prompts: vec![],
            name: "mock".to_string(),
            protocol: McpProtocol::Legacy,
            input_provider: std::sync::RwLock::new(None),
        }
    }
}

impl McpClient {
    /// Create a stdio-based MCP client.
    pub async fn new_stdio(
        name: impl Into<String>,
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
    ) -> AppResult<Self> {
        let transport = StdioTransport::spawn(command, args, env).await?;
        let mut client = Self {
            transport: Arc::new(transport),
            tools: std::sync::RwLock::new(vec![]),
            resources: vec![],
            prompts: vec![],
            name: name.into(),
            protocol: McpProtocol::Legacy,
            input_provider: std::sync::RwLock::new(None),
        };

        client.initialize().await?;
        Ok(client)
    }

    /// Create an HTTP client, optionally attaching an OAuth bearer token
    /// to every request (from the persistent credential store).
    pub async fn new_http_with_bearer(
        name: impl Into<String>,
        url: &str,
        bearer_token: Option<String>,
    ) -> AppResult<Self> {
        let transport = HttpTransport::with_bearer(url, bearer_token);
        let mut client = Self {
            transport: Arc::new(transport),
            tools: std::sync::RwLock::new(vec![]),
            resources: vec![],
            prompts: vec![],
            name: name.into(),
            protocol: McpProtocol::Legacy,
            input_provider: std::sync::RwLock::new(None),
        };

        client.initialize().await?;
        Ok(client)
    }

    /// Create an SSE client — the classic legacy session transport: a GET
    /// event stream with JSON-RPC posted to the server-announced endpoint.
    pub async fn new_sse(
        name: impl Into<String>,
        url: &str,
        bearer_token: Option<String>,
    ) -> AppResult<Self> {
        let transport = SseTransport::connect(url, bearer_token).await?;
        let mut client = Self {
            transport: Arc::new(transport),
            tools: std::sync::RwLock::new(vec![]),
            resources: vec![],
            prompts: vec![],
            name: name.into(),
            protocol: McpProtocol::Legacy,
            input_provider: std::sync::RwLock::new(None),
        };
        client.initialize().await?;
        Ok(client)
    }

    /// Initialize the connection (handshake).
    async fn initialize(&mut self) -> AppResult<()> {
        info!(server = %self.name, "Initializing MCP connection");

        // stdio transports go STRAIGHT to the classic initialize handshake:
        // the 2026-07-28 stateless revision is an HTTP-routing protocol
        // (Mcp-Name headers + `_meta` envelope). Probing a strict-
        // validating stdio server (FastMCP 1.x) with `server/discover`
        // makes it reject the unknown method and stop responding — the
        // client then fails the connect with "MCP response channel closed".
        if !self.transport.is_http_like() {
            self.legacy_handshake().await?;
        } else {
            // HTTP/SSE: stateless first — probe `server/discover`. New
            // servers implement it without any handshake; legacy servers
            // answer MethodNotFound / UnsupportedProtocolVersion and we
            // fall back to the classic initialize handshake.
            match self
                .transport
                .request_stateless("server/discover", None)
                .await
            {
                Ok(_) => {
                    self.protocol = McpProtocol::Stateless2026;
                    info!(
                        server = %self.name,
                        "MCP server speaks stateless 2026-07-28"
                    );
                }
                Err(e)
                    if is_stateless_fallback_error(&e.to_string())
                        || e.to_string().contains("request timeout") =>
                {
                    info!(
                        server = %self.name,
                        error = %e,
                        "server/discover unsupported — falling back to legacy handshake"
                    );
                    self.legacy_handshake().await?;
                }
                Err(e) => return Err(e),
            }
        }

        // Discover tools and resources. `resources`/`prompts` are OPTIONAL
        // capabilities (MCP spec) — a tools-only server answers
        // `resources/list` / `prompts/list` with MethodNotFound (-32601),
        // which must NOT prevent it from connecting. Only a tools/list
        // failure is fatal: without tools the server is useless. The
        // initialize response's capabilities are discarded above, so we
        // cannot know in advance which are supported — tolerate discovery
        // errors for the optional surfaces and log them.
        self.refresh_tools().await?;
        if let Err(e) = self.refresh_resources().await {
            tracing::warn!(
                server = %self.name,
                error = %e,
                "MCP server has no resources (optional capability) — continuing"
            );
        }
        if let Err(e) = self.refresh_prompts().await {
            tracing::warn!(
                server = %self.name,
                error = %e,
                "MCP server has no prompts (optional capability) — continuing"
            );
        }

        info!(
            server = %self.name,
            protocol = ?self.protocol,
            tools = self.tools().len(),
            resources = self.resources.len(),
            prompts = self.prompts.len(),
            "MCP server initialized"
        );

        Ok(())
    }

    /// The classic `initialize` + `notifications/initialized` handshake for
    /// servers that do not speak the stateless 2026-07-28 revision.
    async fn legacy_handshake(&mut self) -> AppResult<()> {
        let params = serde_json::json!({
            "protocolVersion": MCP_PROTOCOL_VERSION_LEGACY,
            "capabilities": {
                "roots": { "listChanged": true },
                "sampling": {}
            },
            "clientInfo": {
                "name": "DeepDepCat",
                "version": "1.0.0"
            }
        });

        // Response payload is unused today — the initialized notification
        // and capability refresh carry the real protocol state.
        let _ = self.transport.request("initialize", Some(params)).await?;

        // Send initialized notification
        self.transport
            .notify("notifications/initialized", None)
            .await?;
        Ok(())
    }

    /// Route a request through the negotiated protocol mode. Stateless
    /// servers get the `_meta` envelope (and HTTP routing headers) on every
    /// request; legacy servers keep the session-based path.
    async fn rpc(&self, method: &str, params: Option<Value>) -> AppResult<Value> {
        self.rpc_with_session(method, params, None).await
    }

    /// Route a request through the negotiated protocol mode, carrying the
    /// CALLING SESSION into the MRTR input provider — unattended sessions
    /// auto-decline instead of parking a human dialog.
    async fn rpc_with_session(
        &self,
        method: &str,
        params: Option<Value>,
        session_id: Option<&str>,
    ) -> AppResult<Value> {
        let result = if self.protocol == McpProtocol::Stateless2026 {
            self.transport
                .request_stateless(method, params.clone())
                .await?
        } else {
            self.transport.request(method, params.clone()).await?
        };
        // 2026-07-28 MRTR: a server asking for more input mid-call signals
        // `resultType: "input_required"`. When an input provider is
        // installed (manager injects the elicitation channel), collect the
        // user's value and re-send the same call with it once — otherwise
        // surface a clear error instead of treating the interim payload as
        // a completed result.
        if result.get("resultType").and_then(|v| v.as_str()) == Some("input_required") {
            let provider = self
                .input_provider
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            if let Some(provider) = provider {
                let prompt = input_required_prompt(&result);
                let schema = result.get("input").and_then(|i| i.get("schema")).cloned();
                if let Some(input) =
                    provider(prompt, schema, session_id.map(|s| s.to_string())).await
                {
                    let retried_params = inject_input(params, input);
                    let retried = if self.protocol == McpProtocol::Stateless2026 {
                        self.transport
                            .request_stateless(method, retried_params)
                            .await?
                    } else {
                        self.transport.request(method, retried_params).await?
                    };
                    if retried.get("resultType").and_then(|v| v.as_str()) == Some("input_required")
                    {
                        let detail = input_required_detail(&retried);
                        return Err(AppError::Mcp(format!(
                            "MCP server still requires additional input after user response.{detail}"
                        )));
                    }
                    return Ok(retried);
                }
            }
            let detail = input_required_detail(&result);
            return Err(AppError::Mcp(format!(
                "MCP server requested additional input (MRTR resultType=input_required) — \
                     not supported yet.{}",
                detail
            )));
        }
        Ok(result)
    }

    /// Install the MRTR input provider — called by the manager with the
    /// elicitation channel so `input_required` results can ask the user.
    pub async fn set_input_provider(&self, provider: Arc<InputProvider>) {
        *self
            .input_provider
            .write()
            .unwrap_or_else(|e| e.into_inner()) = Some(provider);
    }

    /// Refresh the tool list from the server.
    pub async fn refresh_tools(&self) -> AppResult<()> {
        let result = self.rpc("tools/list", None).await?;

        if let Some(tools) = result.get("tools").and_then(|t| t.as_array()) {
            let refreshed: Vec<McpTool> = tools
                .iter()
                .filter_map(|t| serde_json::from_value::<McpTool>(t.clone()).ok())
                .collect();
            *self.tools.write().unwrap_or_else(|e| e.into_inner()) = refreshed;
        }

        Ok(())
    }

    /// Refresh the resource list from the server.
    pub async fn refresh_resources(&mut self) -> AppResult<()> {
        let result = self.rpc("resources/list", None).await?;

        if let Some(resources) = result.get("resources").and_then(|r| r.as_array()) {
            self.resources = resources
                .iter()
                .filter_map(|r| serde_json::from_value::<McpResource>(r.clone()).ok())
                .collect();
        }

        Ok(())
    }

    /// Refresh the prompt list from the server.
    pub async fn refresh_prompts(&mut self) -> AppResult<()> {
        let result = self.rpc("prompts/list", None).await?;

        if let Some(prompts) = result.get("prompts").and_then(|p| p.as_array()) {
            self.prompts = prompts
                .iter()
                .filter_map(|p| serde_json::from_value::<McpPrompt>(p.clone()).ok())
                .collect();
        }

        Ok(())
    }

    /// Get a prompt template by name, filling in the given arguments.
    pub async fn get_prompt(&self, name: &str, arguments: Value) -> AppResult<Value> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });

        self.rpc("prompts/get", Some(params)).await
    }

    /// Get the list of available prompts.
    pub fn prompts(&self) -> &[McpPrompt] {
        &self.prompts
    }

    /// Call a tool and surface an MCP Apps UI payload, WITHOUT tool metadata.
    ///
    /// Test-only convenience: the production paths pass the tool definition
    /// (to carry `_meta.ui.csp`) via [`call_tool_detailed_with_meta`]; this
    /// wrapper delegates with no metadata.
    #[cfg(test)]
    pub async fn call_tool_detailed(
        &self,
        name: &str,
        arguments: Value,
    ) -> AppResult<CallToolOutcome> {
        self.call_tool_detailed_with_meta(name, arguments, None, None)
            .await
    }

    /// Like [`call_tool_detailed`], but with the tool definition's MCP Apps
    /// metadata (`_meta.ui`) — the CSP domains declared there are carried
    /// into the payload so the host can sandbox the rendered document.
    pub async fn call_tool_detailed_with_meta(
        &self,
        name: &str,
        arguments: Value,
        tool: Option<&crate::mcp::types::McpTool>,
        session_id: Option<&str>,
    ) -> AppResult<CallToolOutcome> {
        let declared_csp = tool.and_then(|t| {
            t._meta
                .as_ref()
                .and_then(|m| m.get("ui"))
                .and_then(|ui| ui.get("csp"))
                .cloned()
        });

        let params = serde_json::json!({
            "name": name,
            "arguments": arguments,
        });

        let result = self
            .rpc_with_session("tools/call", Some(params), session_id)
            .await?;

        let call: crate::mcp::types::CallToolResult = match serde_json::from_value(result.clone()) {
            Ok(c) => c,
            Err(_) => {
                // Non-standard server — fall back to the raw JSON.
                return Ok(CallToolOutcome {
                    content: result.to_string(),
                    is_error: false,
                    app: None,
                });
            }
        };

        let is_error = call.is_error.unwrap_or(false);
        let content = Self::render_call_result(&call);

        // MCP Apps: look for a `ui://` resource block in the content.
        let app = self.fetch_ui_resource(&call, is_error, declared_csp).await;

        Ok(CallToolOutcome {
            content,
            is_error,
            app,
        })
    }

    /// Fetch the interactive HTML for an MCP Apps result, when the tool
    /// result carries a `ui://` resource block. Oversized payloads are
    /// dropped (logged, never rendered). The `csp` domains come from the
    /// tool's `_meta.ui.csp` declaration — injected by the host so the app
    /// can only reach origins its server declared.
    async fn fetch_ui_resource(
        &self,
        call: &crate::mcp::types::CallToolResult,
        is_error: bool,
        declared_csp: Option<Value>,
    ) -> Option<McpAppPayload> {
        let uri = call
            .content
            .iter()
            .filter_map(|block| {
                let resource = block.resource.as_ref()?;
                let uri = resource.get("uri")?.as_str()?;
                if !uri.starts_with(UI_SCHEME) {
                    return None;
                }
                Some(uri.to_string())
            })
            .next()?;

        let html = self.read_resource(&uri).await.ok()?;
        if html.len() > MAX_MCP_APP_HTML_BYTES {
            info!(
                server = %self.name,
                bytes = html.len(),
                "MCP App HTML exceeds size cap — not rendered"
            );
            return None;
        }

        Some(McpAppPayload {
            resource_uri: uri,
            html,
            is_error,
            csp: declared_csp,
        })
    }

    /// Render a `CallToolResult` as text for the model.
    fn render_call_result(call: &crate::mcp::types::CallToolResult) -> String {
        // Prefer the structured content — it carries the tool's actual
        // return value that text blocks may omit.
        if let Some(structured) = &call.structured_content {
            return serde_json::to_string_pretty(structured)
                .unwrap_or_else(|_| structured.to_string());
        }
        // Otherwise concatenate text content blocks.
        let mut text = String::new();
        for item in &call.content {
            if let Some(t) = &item.text {
                text.push_str(t);
            }
        }
        text
    }

    /// Read a resource from the MCP server.
    pub async fn read_resource(&self, uri: &str) -> AppResult<String> {
        let params = serde_json::json!({
            "uri": uri,
        });

        let result = self.rpc("resources/read", Some(params)).await?;

        if let Some(contents) = result.get("contents").and_then(|c| c.as_array()) {
            let mut text = String::new();
            for item in contents {
                if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                    text.push_str(t);
                }
            }
            Ok(text)
        } else {
            Ok(result.to_string())
        }
    }

    /// Get the server name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the list of available tools.
    pub fn tools(&self) -> Vec<McpTool> {
        self.tools.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Ping the server to verify it's alive.
    pub async fn ping(&self) -> AppResult<()> {
        if self.protocol == McpProtocol::Stateless2026 {
            // `ping` was removed in 2026-07-28 — the discovery endpoint is
            // the liveness probe: any successful stateless request proves
            // the server is alive. This MUST be a real call: a no-op would
            // let the pool's liveness loop refresh heartbeats forever even
            // after the server died, so a dead HTTP server would never be
            // reconnected.
            self.transport
                .request_stateless("server/discover", None)
                .await?;
            return Ok(());
        }
        self.transport.request("ping", None).await?;
        Ok(())
    }

    /// Register a handler for server-initiated requests.
    ///
    /// Transports that support server pushes (stdio) invoke the handler for
    /// requests like `elicitation/create`; the returned value is written back
    /// as the JSON-RPC response. No-op for transports that cannot receive
    /// server pushes (HTTP).
    pub async fn set_server_request_handler(
        &self,
        handler: crate::mcp::transport::ServerRequestHandler,
    ) {
        self.transport.set_server_request_handler(handler).await;
    }

    /// Register a handler invoked when the server announces
    /// `notifications/tools/list_changed` — the manager hot-refreshes the
    /// tool registry without a full reconnect.
    pub async fn set_tool_list_changed_handler(&self, handler: Arc<dyn Fn() + Send + Sync>) {
        self.transport.set_tool_list_changed_handler(handler).await;
    }

    /// Close the connection.
    pub async fn close(&self) -> AppResult<()> {
        self.transport.close().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::error::AppError;
    use crate::mcp::transport::McpTransport;
    use crate::mcp::types::{CallToolResult, McpCallContentBlock};
    use async_trait::async_trait;

    #[test]
    fn input_required_detail_picks_server_hint() {
        let result = serde_json::json!({
            "resultType": "input_required",
            "input": { "title": "More details", "description": "Need a date range" }
        });
        let detail = input_required_detail(&result);
        assert!(detail.contains("Need a date range"), "{detail}");
        assert!(detail.contains("More details"), "{detail}");

        let bare = serde_json::json!({ "resultType": "input_required" });
        assert_eq!(input_required_detail(&bare), "");
    }

    #[test]
    fn inject_input_merges_into_object_params() {
        let params = serde_json::json!({ "name": "search", "arguments": { "q": "x" } });
        let merged = inject_input(Some(params), serde_json::json!({ "date": "2026-01-01" }))
            .expect("params present");
        assert_eq!(merged["input"]["date"], "2026-01-01");
        assert_eq!(merged["name"], "search");

        let bare = inject_input(None, serde_json::json!("hello"));
        assert_eq!(bare.expect("envelope")["input"], "hello");
    }

    #[test]
    fn input_required_prompt_prefers_description() {
        let result = serde_json::json!({
            "resultType": "input_required",
            "input": { "title": "T", "description": "Describe the range" }
        });
        assert_eq!(input_required_prompt(&result), "Describe the range");
        let bare = serde_json::json!({ "resultType": "input_required" });
        assert!(!input_required_prompt(&bare).is_empty());
    }

    #[tokio::test]
    async fn rpc_input_required_collects_input_and_retries_once() {
        let transport = MockTransport::new(vec![]);
        transport.push_response(serde_json::json!({
            "resultType": "input_required",
            "input": { "description": "Need a date range" }
        }));
        transport.push_response(serde_json::json!({ "ok": true }));

        let client = McpClient {
            transport: Arc::new(transport),
            tools: std::sync::RwLock::new(vec![]),
            resources: vec![],
            prompts: vec![],
            name: "mrtr".to_string(),
            protocol: McpProtocol::Legacy,
            input_provider: std::sync::RwLock::new(Some(Arc::new(
                |message: String,
                 schema: Option<serde_json::Value>,
                 _session_id: Option<String>| {
                    Box::pin(async move {
                        assert!(message.contains("date range"));
                        assert!(schema.is_none());
                        Some(serde_json::json!({ "from": "2026-01-01" }))
                    })
                },
            ))),
        };

        let result = client
            .rpc("tools/call", Some(serde_json::json!({ "name": "x" })))
            .await
            .expect("retried call succeeds");
        assert_eq!(result["ok"], true);
    }

    #[tokio::test]
    async fn rpc_input_required_without_provider_errors_clearly() {
        let transport = MockTransport::new(vec![(
            "tools/call".to_string(),
            serde_json::json!({
                "resultType": "input_required",
                "input": { "description": "Need input" }
            }),
        )]);
        let client = McpClient {
            transport: Arc::new(transport),
            tools: std::sync::RwLock::new(vec![]),
            resources: vec![],
            prompts: vec![],
            name: "mrtr".to_string(),
            protocol: McpProtocol::Legacy,
            input_provider: std::sync::RwLock::new(None),
        };
        let err = client
            .rpc("tools/call", None)
            .await
            .expect_err("input_required must error without a provider");
        assert!(err.to_string().contains("additional input"), "{err}");
        assert!(err.to_string().contains("Need input"), "{err}");
    }

    /// Test transport with scripted responses keyed by method.
    struct MockTransport {
        responses: std::collections::HashMap<String, serde_json::Value>,
        queue: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<serde_json::Value>>>,
        connected: std::sync::atomic::AtomicBool,
        calls: Arc<std::sync::Mutex<Vec<String>>>,
        http_like: bool,
    }

    impl MockTransport {
        fn new(responses: Vec<(String, serde_json::Value)>) -> Self {
            Self {
                responses: responses.into_iter().collect(),
                queue: std::sync::Arc::new(
                    std::sync::Mutex::new(std::collections::VecDeque::new()),
                ),
                connected: std::sync::atomic::AtomicBool::new(true),
                calls: Arc::new(std::sync::Mutex::new(Vec::new())),
                http_like: true,
            }
        }

        /// Queue one response served before the keyed map (for scripts
        /// where the same method must answer differently per call).
        fn push_response(&self, value: serde_json::Value) {
            self.queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push_back(value);
        }

        /// A stdio-shaped mock: the stateless probe must be skipped.
        fn new_stdio_like(responses: Vec<(String, serde_json::Value)>) -> Self {
            let mut mock = Self::new(responses);
            mock.http_like = false;
            mock
        }

        fn call_log(&self) -> Arc<std::sync::Mutex<Vec<String>>> {
            self.calls.clone()
        }
    }

    #[async_trait]
    impl McpTransport for MockTransport {
        fn is_http_like(&self) -> bool {
            self.http_like
        }

        async fn request(
            &self,
            method: &str,
            _params: Option<serde_json::Value>,
        ) -> AppResult<serde_json::Value> {
            self.calls
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(format!("legacy:{method}"));
            if let Some(value) = self
                .queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()
            {
                return Ok(value);
            }
            self.responses.get(method).cloned().ok_or_else(|| {
                AppError::Mcp(format!("MCP error [-32601]: Method not found: {method}"))
            })
        }

        async fn request_stateless(
            &self,
            method: &str,
            _params: Option<serde_json::Value>,
        ) -> AppResult<serde_json::Value> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("stateless:{method}"));
            if let Some(value) = self
                .queue
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()
            {
                return Ok(value);
            }
            self.responses.get(method).cloned().ok_or_else(|| {
                AppError::Mcp(format!("MCP error [-32601]: Method not found: {method}"))
            })
        }

        async fn notify(&self, _method: &str, _params: Option<serde_json::Value>) -> AppResult<()> {
            Ok(())
        }

        async fn close(&self) -> AppResult<()> {
            self.connected
                .store(false, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        }
    }

    fn client_with(transport: Arc<dyn McpTransport>) -> McpClient {
        McpClient {
            transport,
            tools: std::sync::RwLock::new(vec![]),
            resources: vec![],
            prompts: vec![],
            name: "mock-server".to_string(),
            protocol: McpProtocol::Legacy,
            input_provider: std::sync::RwLock::new(None),
        }
    }

    #[tokio::test]
    async fn initialize_negotiates_stateless_when_discover_succeeds() {
        let mock = MockTransport::new(vec![
            (
                "server/discover".into(),
                serde_json::json!({ "protocolVersion": "2026-07-28" }),
            ),
            ("tools/list".into(), serde_json::json!({ "tools": [] })),
            (
                "resources/list".into(),
                serde_json::json!({ "resources": [] }),
            ),
            ("prompts/list".into(), serde_json::json!({ "prompts": [] })),
        ]);
        let calls = mock.call_log();
        let mut client = client_with(Arc::new(mock));

        client.initialize().await.expect("stateless init succeeds");

        assert_eq!(client.protocol, McpProtocol::Stateless2026);
        let log = calls.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(log.contains(&"stateless:server/discover".to_string()));
        assert!(log.contains(&"stateless:tools/list".to_string()));
        assert!(!log.iter().any(|c| c.starts_with("legacy:")));
    }

    #[tokio::test]
    async fn initialize_falls_back_to_legacy_when_discover_unsupported() {
        let mock = MockTransport::new(vec![
            (
                "initialize".into(),
                serde_json::json!({ "protocolVersion": "2024-11-05" }),
            ),
            ("tools/list".into(), serde_json::json!({ "tools": [] })),
            (
                "resources/list".into(),
                serde_json::json!({ "resources": [] }),
            ),
            ("prompts/list".into(), serde_json::json!({ "prompts": [] })),
        ]);
        let calls = mock.call_log();
        let mut client = client_with(Arc::new(mock));

        client.initialize().await.expect("legacy init succeeds");

        assert_eq!(client.protocol, McpProtocol::Legacy);
        let log = calls.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(log.contains(&"stateless:server/discover".to_string()));
        assert!(log.contains(&"legacy:initialize".to_string()));
        assert!(log.contains(&"legacy:tools/list".to_string()));
        assert!(!log.iter().any(|c| c == "stateless:tools/list"));
    }

    #[tokio::test]
    async fn initialize_stdio_skips_stateless_probe() {
        // stdio transports go STRAIGHT to the classic initialize handshake:
        // probing a strict-validating stdio server (FastMCP 1.x) with
        // `server/discover` makes it reject the unknown method and stop
        // responding — the connect then failed with "MCP response channel
        // closed".
        let mock = MockTransport::new_stdio_like(vec![
            (
                "initialize".into(),
                serde_json::json!({ "protocolVersion": "2024-11-05" }),
            ),
            ("tools/list".into(), serde_json::json!({ "tools": [] })),
            (
                "resources/list".into(),
                serde_json::json!({ "resources": [] }),
            ),
            ("prompts/list".into(), serde_json::json!({ "prompts": [] })),
        ]);
        let calls = mock.call_log();
        let mut client = client_with(Arc::new(mock));

        client
            .initialize()
            .await
            .expect("legacy stdio init succeeds");

        assert_eq!(client.protocol, McpProtocol::Legacy);
        let log = calls.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert!(log.contains(&"legacy:initialize".to_string()));
        assert!(log.contains(&"legacy:tools/list".to_string()));
        assert!(
            !log.iter().any(|c| c.starts_with("stateless:")),
            "stdio must never send the stateless probe: {log:?}"
        );
    }

    #[tokio::test]
    async fn stateless_input_required_result_is_a_clear_error() {
        let mock = MockTransport::new(vec![(
            "tools/call".into(),
            serde_json::json!({
                "resultType": "input_required",
                "inputRequests": []
            }),
        )]);
        let mut client = client_with(Arc::new(mock));
        client.protocol = McpProtocol::Stateless2026;

        let err = client
            .call_tool_detailed("needs_input", serde_json::json!({}))
            .await
            .expect_err("MRTR must surface as a clear error");

        assert!(err.to_string().contains("MRTR"));
    }

    #[tokio::test]
    async fn stateless_ping_probes_discovery() {
        let mock = MockTransport::new(vec![(
            "server/discover".into(),
            serde_json::json!({ "protocolVersion": "2026-07-28" }),
        )]);
        let calls = mock.call_log();
        let mut client = client_with(Arc::new(mock));
        client.protocol = McpProtocol::Stateless2026;

        client
            .ping()
            .await
            .expect("stateless ping probes server/discover");

        assert!(
            calls
                .lock()
                .unwrap()
                .contains(&"stateless:server/discover".to_string()),
            "liveness must be a real request — a no-op would never reconnect a dead server"
        );
    }

    #[tokio::test]
    async fn stateless_ping_failure_propagates() {
        // No server/discover response → Method not found → the liveness
        // check fails and the pool marks the server disconnected.
        let mock = MockTransport::new(vec![]);
        let mut client = client_with(Arc::new(mock));
        client.protocol = McpProtocol::Stateless2026;

        assert!(
            client.ping().await.is_err(),
            "a dead/unresponsive stateless server must fail the liveness probe"
        );
    }

    #[test]
    fn render_prefers_structured_content() {
        let call = CallToolResult {
            content: vec![McpCallContentBlock {
                kind: "text".into(),
                text: Some("human summary".into()),
                image: None,
                mime_type: None,
                resource: None,
            }],
            structured_content: Some(serde_json::json!({"rows": [1, 2, 3]})),
            is_error: None,
        };
        let rendered = McpClient::render_call_result(&call);
        assert!(rendered.contains("\"rows\""));
        assert!(rendered.contains("1"));
        assert!(rendered.contains("3"));
        assert!(!rendered.contains("human summary"));
    }

    #[test]
    fn render_falls_back_to_text_content() {
        let call = CallToolResult {
            content: vec![McpCallContentBlock {
                kind: "text".into(),
                text: Some("plain result".into()),
                image: None,
                mime_type: None,
                resource: None,
            }],
            structured_content: None,
            is_error: None,
        };
        assert_eq!(McpClient::render_call_result(&call), "plain result");
    }

    #[test]
    fn render_empty_content_returns_empty() {
        let call = CallToolResult {
            content: vec![],
            structured_content: None,
            is_error: None,
        };
        assert_eq!(McpClient::render_call_result(&call), "");
    }

    #[tokio::test]
    async fn call_tool_detailed_carries_ui_payload() {
        let transport: Arc<dyn McpTransport> = Arc::new(MockTransport::new(vec![
            (
                "tools/call".into(),
                serde_json::json!({
                    "content": [
                        { "type": "text", "text": "dashboard ready" },
                        {
                            "type": "resource",
                            "resource": { "uri": "ui://app/dashboard", "mimeType": "text/html" }
                        }
                    ],
                    "isError": false
                }),
            ),
            (
                "resources/read".into(),
                serde_json::json!({
                    "contents": [
                        { "uri": "ui://app/dashboard", "mimeType": "text/html", "text": "<!DOCTYPE html><h1>hi</h1>" }
                    ]
                }),
            ),
        ]));
        let client = client_with(transport);

        let outcome = client
            .call_tool_detailed("make_dashboard", serde_json::json!({}))
            .await
            .expect("call succeeds");

        assert_eq!(outcome.content, "dashboard ready");
        assert!(!outcome.is_error);
        let app = outcome.app.expect("ui payload present");
        assert_eq!(app.resource_uri, "ui://app/dashboard");
        assert!(app.html.contains("<h1>hi</h1>"));
        assert!(!app.is_error);
    }

    #[tokio::test]
    async fn call_tool_detailed_without_ui_block_has_no_payload() {
        let transport: Arc<dyn McpTransport> = Arc::new(MockTransport::new(vec![(
            "tools/call".into(),
            serde_json::json!({
                "content": [{ "type": "text", "text": "plain" }],
                "isError": false
            }),
        )]));
        let client = client_with(transport);

        let outcome = client
            .call_tool_detailed("plain_tool", serde_json::json!({}))
            .await
            .expect("call succeeds");

        assert_eq!(outcome.content, "plain");
        assert!(outcome.app.is_none());
    }

    #[tokio::test]
    async fn call_tool_detailed_skips_non_ui_resource_blocks() {
        let transport: Arc<dyn McpTransport> = Arc::new(MockTransport::new(vec![(
            "tools/call".into(),
            serde_json::json!({
                "content": [
                    { "type": "text", "text": "link" },
                    {
                        "type": "resource",
                        "resource": { "uri": "file:///etc/hosts", "mimeType": "text/plain" }
                    }
                ],
                "isError": false
            }),
        )]));
        let client = client_with(transport);

        let outcome = client
            .call_tool_detailed("link_tool", serde_json::json!({}))
            .await
            .expect("call succeeds");

        assert!(outcome.app.is_none());
    }

    #[tokio::test]
    async fn call_tool_detailed_drops_oversized_html() {
        let big = "<!DOCTYPE html>".to_string() + &"x".repeat(MAX_MCP_APP_HTML_BYTES);
        let transport: Arc<dyn McpTransport> = Arc::new(MockTransport::new(vec![
            (
                "tools/call".into(),
                serde_json::json!({
                    "content": [
                        {
                            "type": "resource",
                            "resource": { "uri": "ui://app/big", "mimeType": "text/html" }
                        }
                    ],
                    "isError": false
                }),
            ),
            (
                "resources/read".into(),
                serde_json::json!({
                    "contents": [
                        { "uri": "ui://app/big", "mimeType": "text/html", "text": big }
                    ]
                }),
            ),
        ]));
        let client = client_with(transport);

        let outcome = client
            .call_tool_detailed("big_tool", serde_json::json!({}))
            .await
            .expect("call succeeds");

        assert!(outcome.app.is_none());
    }

    #[tokio::test]
    async fn call_tool_detailed_preserves_is_error_on_payload() {
        let transport: Arc<dyn McpTransport> = Arc::new(MockTransport::new(vec![
            (
                "tools/call".into(),
                serde_json::json!({
                    "content": [
                        { "type": "text", "text": "failed" },
                        {
                            "type": "resource",
                            "resource": { "uri": "ui://app/fail", "mimeType": "text/html" }
                        }
                    ],
                    "isError": true
                }),
            ),
            (
                "resources/read".into(),
                serde_json::json!({
                    "contents": [
                        { "uri": "ui://app/fail", "mimeType": "text/html", "text": "<h1>err</h1>" }
                    ]
                }),
            ),
        ]));
        let client = client_with(transport);

        let outcome = client
            .call_tool_detailed("fail_tool", serde_json::json!({}))
            .await
            .expect("call succeeds");

        assert!(outcome.is_error);
        let app = outcome.app.expect("ui payload present even on failure");
        assert!(app.is_error);
    }
}
