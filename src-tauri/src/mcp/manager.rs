//! MCP manager — manages all MCP server connections.
//!
//! Responsibilities:
//! - Connect/disconnect MCP servers
//! - Aggregate tools from all connected servers
//! - Route tool calls to the correct server
//! - Wrap MCP tools as native DeepDepCat tools

use crate::core::config::McpServerConfig;
use crate::core::error::{AppError, AppResult};
use crate::bootstrap::AppState;
use crate::core::stream::emit_stream;
use crate::core::types::StreamEvent;
use crate::hooks::{HookContext, HookEvent};
use crate::mcp::client::McpClient;
use crate::mcp::tool_bridge::McpToolWrapper;
use crate::mcp::transport::ServerRequestHandler;
use crate::mcp::types::McpTool;
use crate::tools::registry::ToolRegistry;
use serde_json::json;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Emit an MCP connection status event to the frontend (settings page).
/// No-op when the app handle isn't installed yet (tests / pre-setup).
fn emit_status(
    app: Option<&AppHandle>,
    name: &str,
    status: &str,
    error: Option<&str>,
    tools: Option<usize>,
) {
    let Some(app) = app else { return };
    let mut payload = serde_json::json!({ "name": name, "status": status });
    if let Some(error) = error {
        payload["error"] = serde_json::json!(error);
    }
    if let Some(tools) = tools {
        payload["tools"] = serde_json::json!(tools);
    }
    let _ = app.emit("mcp-status-changed", payload);
}

/// The MCP manager — manages all MCP server connections.
pub struct McpManager {
    clients: Arc<RwLock<HashMap<String, Arc<McpClient>>>>,
    /// App data dir — used to load persisted OAuth credentials for
    /// HTTP/SSE servers.
    app_data_dir: Option<std::path::PathBuf>,
    /// Connection pool — tracks per-server health and drives reconnection.
    connection_pool: Option<Arc<crate::mcp::connection_pool::McpConnectionPool>>,
    /// Desired server configs (filled by sync_configs) — used by reconnect.
    configs: Arc<RwLock<HashMap<String, McpServerConfig>>>,
    /// Shared tool registry — used by reconnect to re-register tools.
    registry: Arc<RwLock<Option<Arc<ToolRegistry>>>>,
    /// App handle — used by reconnect (elicitation handler needs it).
    app: Arc<RwLock<Option<AppHandle>>>,
    /// Server names with a connect in flight — serializes per-name connects
    /// so startup sync, settings connect, and the reconnect handler can
    /// never double-spawn the same server (which would register its tools
    /// twice and leave a zombie process).
    connecting: Arc<tokio::sync::Mutex<HashSet<String>>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            app_data_dir: None,
            connection_pool: None,
            configs: Arc::new(RwLock::new(HashMap::new())),
            registry: Arc::new(RwLock::new(None)),
            app: Arc::new(RwLock::new(None)),
            connecting: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
        }
    }

    /// Point the manager at the app data dir so HTTP/SSE connections can
    /// attach persisted OAuth credentials.
    pub fn with_app_data_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.app_data_dir = Some(dir);
        self
    }

    /// Attach the connection pool — the manager registers every connection
    /// with the pool and the pool's health checker drives reconnection
    /// through the installed handler.
    pub fn with_connection_pool(
        mut self,
        pool: Arc<crate::mcp::connection_pool::McpConnectionPool>,
    ) -> Self {
        self.connection_pool = Some(pool);
        self
    }

    /// Install the reconnect handler on the connection pool.
    ///
    /// Must be called after the pool exists and once per app lifetime. The
    /// handler re-connects a server using the stored config, re-registers
    /// its tools, and records a heartbeat on success.
    pub fn install_reconnect_handler(self: &Arc<Self>, app: AppHandle) {
        *self.app.blocking_write() = Some(app.clone());
        if let Some(pool) = &self.connection_pool {
            let this = self.clone();
            let handler: Arc<crate::mcp::connection_pool::ReconnectHandler> =
                Arc::new(move |name: String| {
                    let this = this.clone();
                    let app = app.clone();
                    Box::pin(async move {
                        this.reconnect(&name, &app).await;
                    })
                });
            let pool = pool.clone();
            tauri::async_runtime::spawn(async move {
                pool.set_reconnect_handler(handler).await;
            });
        }
    }

    /// Reconnect a server by name (invoked by the pool's health checker).
    async fn reconnect(&self, name: &str, app: &AppHandle) {
        let config = self.configs.read().await.get(name).cloned();
        let registry = self.registry.read().await.clone();
        let (Some(config), Some(registry)) = (config, registry) else {
            warn!(server = %name, "Cannot reconnect — server config or registry missing");
            return;
        };
        emit_status(Some(app), name, "connecting", None, None);
        info!(server = %name, "Reconnecting MCP server");
        // Close the stale client and unregister its tools BEFORE spawning a
        // fresh one: a liveness timeout may have marked a still-running
        // stdio child dead, and replacing it without close() would orphan
        // the process. Unregistering first also guarantees a fresh server
        // with FEWER tools never leaves stale wrappers pointing at the
        // closed client.
        let _ = self.disconnect(name).await;
        match self.connect(&config, &registry, app).await {
            Ok(()) => {
                if let Some(pool) = &self.connection_pool {
                    // Reset the backoff counter — the connection is healthy again.
                    pool.record_heartbeat(name).await;
                }
                info!(server = %name, "MCP server reconnected");
            }
            Err(e) => {
                warn!(server = %name, error = %e, "MCP reconnection failed — will retry with backoff");
                if let Some(pool) = &self.connection_pool {
                    // Drop back to Disconnected so the checker retries on
                    // the next cycle instead of stalling in Reconnecting
                    // until the heartbeat goes stale.
                    pool.record_reconnect_failure(name).await;
                }
            }
        }
    }

    /// Remember the desired config for a server connected manually from
    /// settings. The pool's reconnect handler rebuilds connections from
    /// this map — without the entry a dropped manual server can never be
    /// re-established until the app restarts.
    pub async fn remember_config(&self, cfg: &McpServerConfig) {
        if cfg.enabled {
            self.configs
                .write()
                .await
                .insert(cfg.name.clone(), cfg.clone());
        }
    }

    /// Forget the config of a removed server so a pending reconnect never
    /// resurrects it.
    pub async fn forget_config(&self, name: &str) {
        self.configs.write().await.remove(name);
    }
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpManager {
    /// Connect to an MCP server and register its tools into the given registry.
    ///
    /// The `app` handle is used to surface server-initiated elicitation
    /// requests to the frontend and to register pending responders in
    /// `AppState`.
    pub async fn connect(
        &self,
        config: &McpServerConfig,
        registry: &ToolRegistry,
        app: &AppHandle,
    ) -> AppResult<()> {
        let result = self.connect_inner(config, registry, app).await;
        match &result {
            Ok(()) => {
                let tool_count = self
                    .clients
                    .read()
                    .await
                    .get(&config.name)
                    .map(|c| c.tools().len())
                    .unwrap_or(0);
                  emit_status(Some(app), &config.name, "connected", None, Some(tool_count));
                  // McpServerConnected hook — observability for MCP
                  // lifecycle (workspace-global pseudo-session; hooks can
                  // filter on the server name).
                  app.state::<AppState>()
                      .hook_executor
                      .execute_observe(
                          &HookContext::new(HookEvent::McpServerConnected, "workspace")
                              .with_data("server", json!(config.name))
                              .with_data("tools", json!(tool_count)),
                      )
                      .await;
              }
            Err(e) => {
                emit_status(Some(app), &config.name, "error", Some(&e.to_string()), None);
            }
        }
        result
    }

    /// The actual connect work — kept separate from `connect` so the
    /// status event is emitted exactly once per attempt.
    async fn connect_inner(
        &self,
        config: &McpServerConfig,
        registry: &ToolRegistry,
        app: &AppHandle,
    ) -> AppResult<()> {
        // Serialize per-name connects: two concurrent connects for the same
        // server must not both pass the contains_key teardown below and
        // register duplicate tools / spawn two child processes.
        {
            let mut connecting = self.connecting.lock().await;
            if !connecting.insert(config.name.clone()) {
                return Err(AppError::Mcp(format!(
                    "MCP server '{}' is already connecting",
                    config.name
                )));
            }
        }
        let result = self.connect_inner_unlocked(config, registry, app).await;
        self.connecting.lock().await.remove(&config.name);
        result
    }

    /// The actual connect work — run under the per-name connect guard.
    async fn connect_inner_unlocked(
        &self,
        config: &McpServerConfig,
        registry: &ToolRegistry,
        app: &AppHandle,
    ) -> AppResult<()> {
        info!(server = %config.name, "Connecting to MCP server");

        // A re-connect with the same name (config edited in settings) must
        // tear down the old client and unregister its tools first —
        // otherwise the registry ends up with two wrappers for the same
        // tool, one pointing at a zombie process.
        if self.clients.read().await.contains_key(&config.name) {
            let _ = self.disconnect(&config.name).await;
        }

        let client = match config.transport_type.as_str() {
            "stdio" => {
                let command = config
                    .command
                    .as_ref()
                    .ok_or_else(|| AppError::Mcp("Missing command for stdio transport".into()))?;
                McpClient::new_stdio(&config.name, command, &config.args, &config.env).await?
            }
            "http" => {
                let url = config
                    .url
                    .as_ref()
                    .ok_or_else(|| AppError::Mcp("Missing URL for HTTP transport".into()))?;
                // Attach the persisted OAuth credential (if any) for this
                // server so requests carry Authorization: Bearer. Expired
                // credentials with a refresh token are renewed first.
                let bearer = self.bearer_for(&config.name, url).await;
                McpClient::new_http_with_bearer(&config.name, url, bearer).await?
            }
            "sse" => {
                let url = config
                    .url
                    .as_ref()
                    .ok_or_else(|| AppError::Mcp("Missing URL for SSE transport".into()))?;
                let bearer = self.bearer_for(&config.name, url).await;
                McpClient::new_sse(&config.name, url, bearer).await?
            }
            _ => {
                return Err(AppError::Mcp(format!(
                    "Unknown transport type: {}",
                    config.transport_type
                )))
            }
        };

        // Register MCP tools with the tool registry
        let client = Arc::new(client);
        self.install_elicitation_handler(&client, config, app)
            .await?;
        self.install_input_provider(&client, config, app).await?;
        self.install_tool_list_changed_handler(&client, config, app)
            .await;
        self.register_mcp_tools(&client, registry).await?;

        if let Some(pool) = &self.connection_pool {
            pool.register(config.name.clone()).await;
        }

        self.clients
            .write()
            .await
            .insert(config.name.clone(), client);
        info!(server = %config.name, "MCP server connected");
        Ok(())
    }

    /// Load the persisted OAuth credential for a server and auto-renew it
    /// when expired (OAuth2 refresh grant). Returns the fresh access token.
    async fn bearer_for(&self, server_name: &str, url: &str) -> Option<String> {
        let dir = self.app_data_dir.as_ref()?;
        let mut store = crate::mcp::credentials::McpCredentialStore::load_from(dir).ok()?;
        if store.refresh_expired(dir).await.ok().unwrap_or(false) {
            info!(server = %server_name, "Refreshed expired MCP OAuth credential");
        }
        store.get(server_name, url).map(|c| c.access_token.clone())
    }

    /// Install the server-initiated request handler on a client's transport.
    ///
    /// stdio transports can receive `elicitation/create` from the server.
    /// The handler registers a pending responder in `AppState`, emits the
    /// `Elicitation` stream event to the frontend, and waits for the user's
    /// reply (5-minute timeout) before returning the JSON-RPC response.
    async fn install_elicitation_handler(
        &self,
        client: &Arc<McpClient>,
        config: &McpServerConfig,
        app: &AppHandle,
    ) -> AppResult<()> {
        let pending = {
            let state = app.state::<AppState>();
            state.pending_elicitations.clone()
        };
        let server_name = config.name.clone();
        let app = app.clone();

        let handler: ServerRequestHandler = Arc::new(move |req| {
            let pending = pending.clone();
            let app = app.clone();
            let server_name = server_name.clone();
            Box::pin(async move {
                if req.method != "elicitation/create" {
                    return None;
                }
                let params = req.params.unwrap_or_default();
                let message = params
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or_default()
                    .to_string();
                let requested_schema = params
                    .get("requestedSchema")
                    .cloned()
                    .or_else(|| params.get("requested_schema").cloned());

                let elicitation_id = crate::core::ids::generate_id();
                let (tx, rx) = tokio::sync::oneshot::channel();
                pending.lock().await.insert(elicitation_id.clone(), tx);

                emit_stream(
                    &app,
                    StreamEvent::Elicitation {
                        elicitation_id: elicitation_id.clone(),
                        server_name: server_name.clone(),
                        message,
                        requested_schema,
                    },
                );

                // Server-push elicitation has no session context (the
                // transport does not carry it). When ANY unattended session
                // exists, bound the wait hard — an unattended run must not
                // squat on a human dialog. Interactive-only runs keep the
                // full 5-minute window.
                let app_state = app.state::<AppState>();
                let unattended = app_state.unattended_sessions.clone();
                let unattended_any = !unattended.lock().await.is_empty();
                let wait = if unattended_any {
                    Duration::from_secs(20)
                } else {
                    Duration::from_secs(300)
                };
                match tokio::time::timeout(wait, rx).await {
                    Ok(Ok(result)) => {
                        // Release the slot on every outcome — a leaked
                        // oneshot wedges the entry in the map forever.
                        pending.lock().await.remove(&elicitation_id);
                        Some(serde_json::json!({
                            "action": result.action,
                            "content": result.content,
                        }))
                    }
                    _ => {
                        pending.lock().await.remove(&elicitation_id);
                        Some(serde_json::json!({
                            "action": "cancel",
                            "content": null,
                        }))
                    }
                }
            })
        });

        client.set_server_request_handler(handler).await;
        Ok(())
    }

    /// Install the MRTR input provider on the client: when a tool call
    /// returns `resultType: input_required`, ask the user through the SAME
    /// elicitation channel the server-push path uses, then re-send the call
    /// with the collected input. `None` (cancel/timeout) leaves the caller
    /// with the original "additional input required" error.
    async fn install_input_provider(
        &self,
        client: &Arc<McpClient>,
        config: &McpServerConfig,
        app: &AppHandle,
    ) -> AppResult<()> {
        let pending = {
            let state = app.state::<AppState>();
            state.pending_elicitations.clone()
        };
        let server_name = config.name.clone();
        let app = app.clone();

        let provider: Arc<crate::mcp::client::InputProvider> =
            Arc::new(move |message, schema, session_id| {
                let pending = pending.clone();
                let app = app.clone();
                let server_name = server_name.clone();
                Box::pin(async move {
                    // Unattended sessions have no human — decline immediately
                    // instead of parking a dialog for 5 minutes.
                    if let Some(sid) = session_id.as_deref() {
                        let state = app.state::<AppState>();
                        if state.is_unattended(sid).await {
                            return None;
                        }
                    }
                    let elicitation_id = crate::core::ids::generate_id();
                    let (tx, rx) = tokio::sync::oneshot::channel();
                    pending.lock().await.insert(elicitation_id.clone(), tx);

                    emit_stream(
                        &app,
                        StreamEvent::Elicitation {
                            elicitation_id: elicitation_id.clone(),
                            server_name: server_name.clone(),
                            message,
                            requested_schema: schema,
                        },
                    );

                    match tokio::time::timeout(Duration::from_secs(300), rx).await {
                        Ok(Ok(result)) => {
                            pending.lock().await.remove(&elicitation_id);
                            Some(serde_json::json!({
                                "action": result.action,
                                "content": result.content,
                            }))
                        }
                        _ => {
                            pending.lock().await.remove(&elicitation_id);
                            None
                        }
                    }
                })
            });

        client.set_input_provider(provider).await;
        Ok(())
    }

    /// Install the `tools/list_changed` notification handler on the
    /// transport: the server announces its tool list changed, and the
    /// manager hot-refreshes the registry WITHOUT a full reconnect (no
    /// child restart, no dropped in-flight calls).
    async fn install_tool_list_changed_handler(
        &self,
        client: &Arc<McpClient>,
        config: &McpServerConfig,
        app: &AppHandle,
    ) {
        let manager = app.state::<AppState>().mcp_manager.clone();
        let server_name = config.name.clone();
        let handler: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let manager = manager.clone();
            let name = server_name.clone();
            tokio::spawn(async move {
                manager.refresh_server_tools(&name).await;
            });
        });
        client.set_tool_list_changed_handler(handler).await;
    }

    /// Hot-refresh one server's tools after `notifications/tools/list_changed`:
    /// re-fetches `tools/list` and swaps the registry entries for the
    /// server's namespace.
    pub async fn refresh_server_tools(&self, name: &str) {
        let client = self.clients.read().await.get(name).cloned();
        let Some(client) = client else {
            warn!(server = %name, "tools/list_changed for unknown server — ignoring");
            return;
        };
        if let Err(e) = client.refresh_tools().await {
            warn!(server = %name, error = %e, "tools/list_changed refresh failed");
            return;
        }
        let Some(registry) = self.registry.read().await.clone() else {
            warn!(server = %name, "tools/list_changed refresh skipped — no registry");
            return;
        };
        let removed = registry.unregister_prefix(&format!("{name}__"));
        if let Err(e) = self.register_mcp_tools(&client, &registry).await {
            warn!(server = %name, error = %e, "re-registering MCP tools after refresh failed");
            return;
        }
        info!(
            server = %name,
            removed,
            count = client.tools().len(),
            "Hot-refreshed MCP tools after list_changed"
        );
    }

    /// Disconnect from an MCP server.
    pub async fn disconnect(&self, name: &str) -> AppResult<()> {
        if let Some(client) = self.clients.write().await.remove(name) {
            client.close().await?;
            if let Some(pool) = &self.connection_pool {
                pool.unregister(name).await;
            }
            // Unregister the server's tools from the SHARED registry —
            // otherwise the model keeps seeing tools of a disconnected
            // server. MCP tools are namespaced `server__tool`, so a prefix
            // removal is exact.
            if let Some(registry) = self.registry.read().await.as_ref() {
                let removed = registry.unregister_prefix(&format!("{name}__"));
                if removed > 0 {
                    info!(server = %name, removed, "Unregistered MCP tools from shared registry");
                }
            }
              info!(server = %name, "MCP server disconnected");
              // McpServerDisconnected hook — same lifecycle observability.
              if let Some(app) = self.app.read().await.as_ref() {
                  app.state::<AppState>()
                      .hook_executor
                      .execute_observe(
                          &HookContext::new(HookEvent::McpServerDisconnected, "workspace")
                              .with_data("server", json!(name)),
                      )
                      .await;
              }
          }
        emit_status(
            self.app.read().await.as_ref(),
            name,
            "disconnected",
            None,
            None,
        );
        Ok(())
    }

    /// Register MCP tools as native DeepDepCat tools.
    async fn register_mcp_tools(
        &self,
        client: &Arc<McpClient>,
        registry: &ToolRegistry,
    ) -> AppResult<()> {
        let server_name = client.name().to_string();
        for tool in client.tools() {
            let wrapper = McpToolWrapper::new(&server_name, tool.clone(), client.clone());
            registry.register(Arc::new(wrapper));
            info!(
                server = %server_name,
                tool = %client.name(),
                "Registered MCP tool"
            );
        }
        Ok(())
    }

    /// Get a list of all connected server names.
    pub async fn list_servers(&self) -> Vec<String> {
        self.clients.read().await.keys().cloned().collect()
    }

    /// Start a background liveness check for all connected servers.
    ///
    /// Pings each server every `interval_secs` seconds. A failed or timed
    /// out ping marks the connection as disconnected in the pool, which
    /// drives the exponential-backoff reconnection loop.
    pub fn start_liveness_check(&self, interval_secs: u64, timeout_secs: u64) {
        let clients = self.clients.clone();
        let pool = self.connection_pool.clone();
        tokio::spawn(async move {
            let interval = tokio::time::Duration::from_secs(interval_secs);
            let timeout = tokio::time::Duration::from_secs(timeout_secs);

            loop {
                tokio::time::sleep(interval).await;

                let servers: Vec<(String, Arc<McpClient>)> = {
                    let guard = clients.read().await;
                    guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                };

                for (name, client) in servers {
                    let healthy = tokio::time::timeout(timeout, client.ping()).await;
                    match healthy {
                        Ok(Ok(())) => {
                            debug!(server = %name, "MCP liveness check passed");
                            // Refresh the pool's heartbeat on EVERY successful
                            // ping — the pool checker otherwise only refreshes
                            // on reconnect, so a HEALTHY server's heartbeat
                            // goes stale after 2×interval and the checker
                            // force-reconnects it every ~3 minutes (stdio
                            // processes restarted, tools briefly unregistered).
                            if let Some(ref pool) = pool {
                                pool.record_heartbeat(&name).await;
                            }
                        }
                        Ok(Err(e)) => {
                            warn!(server = %name, error = %e, "MCP liveness check failed — marking disconnected");
                            if let Some(ref pool) = pool {
                                pool.mark_disconnected(&name).await;
                            }
                        }
                        Err(_) => {
                            warn!(server = %name, "MCP liveness check timed out — marking disconnected");
                            if let Some(ref pool) = pool {
                                pool.mark_disconnected(&name).await;
                            }
                        }
                    }
                }
            }
        });
    }

    /// Get all tools from all connected servers.
    pub async fn list_all_tools(&self) -> Vec<(String, McpTool)> {
        let clients = self.clients.read().await;
        let mut tools = Vec::new();
        for (name, client) in clients.iter() {
            for tool in client.tools() {
                tools.push((name.clone(), tool.clone()));
            }
        }
        tools
    }

    /// List prompts exposed by a connected server.
    pub async fn list_prompts(
        &self,
        server_name: &str,
    ) -> AppResult<Vec<crate::mcp::types::McpPrompt>> {
        let clients = self.clients.read().await;
        let client = clients
            .get(server_name)
            .ok_or_else(|| AppError::Mcp(format!("Server '{}' not connected", server_name)))?;
        Ok(client.prompts().to_vec())
    }

    /// Get a prompt template from a server with arguments filled in.
    pub async fn get_prompt(
        &self,
        server_name: &str,
        prompt_name: &str,
        arguments: Value,
    ) -> AppResult<Value> {
        let clients = self.clients.read().await;
        let client = clients
            .get(server_name)
            .ok_or_else(|| AppError::Mcp(format!("Server '{}' not connected", server_name)))?;
        client.get_prompt(prompt_name, arguments).await
    }

    /// Proxy an MCP Apps view request to its server (MCP Apps spec — the
    /// view acts as an MCP client over postMessage; the host forwards).
    ///
    /// Only `tools/call` and `resources/read` are accepted — the exact
    /// subset the spec grants to views. The server is taken from the
    /// request (the frontend binds it to the view's origin server), so a
    /// view can never reach a server it did not come from.
    pub async fn proxy_ui_request(
        &self,
        server_name: &str,
        method: &str,
        params: Value,
    ) -> AppResult<Value> {
        let clients = self.clients.read().await;
        let client = clients
            .get(server_name)
            .ok_or_else(|| AppError::Mcp(format!("Server '{}' not connected", server_name)))?;

        match method {
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(|n| n.as_str())
                    .ok_or_else(|| AppError::Mcp("tools/call requires a string 'name'".into()))?;
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                // Pass the tool's definition so the declared CSP (_meta.ui.csp)
                // travels into the returned app payload — otherwise the
                // re-invoked view renders with the restrictive default and
                // the app's own scripts/CDN origins are blocked (the
                // model-driven path already carries it).
                let tools = client.tools();
                let tool = tools.iter().find(|t| t.name == name);
                let outcome = client
                    .call_tool_detailed_with_meta(name, arguments, tool, None)
                    .await?;
                Ok(json!({
                    "content": outcome.content,
                    "isError": outcome.is_error,
                    "app": outcome.app,
                }))
            }
            "resources/read" => {
                let uri = params.get("uri").and_then(|u| u.as_str()).ok_or_else(|| {
                    AppError::Mcp("resources/read requires a string 'uri'".into())
                })?;
                let text = client.read_resource(uri).await?;
                Ok(json!({ "text": text }))
            }
            _ => Err(AppError::Mcp(format!(
                "MCP Apps proxy rejects method '{}' (allowed: tools/call, resources/read)",
                method
            ))),
        }
    }
}

/// Result of comparing desired MCP config against the current connections.
#[derive(Debug, Clone, Default)]
pub struct McpConfigDiff {
    /// Server names that are new or not yet connected — must be connected.
    pub added: Vec<String>,
    /// Server names that were removed from config — must be disconnected.
    pub removed: Vec<String>,
    /// Server names whose config is unchanged — keep the live client.
    pub retained: Vec<String>,
}

impl McpManager {
    /// Compare a desired server config list against currently connected servers.
    ///
    /// Mirrors the upstream config-diff semantics: identical configs keep
    /// their live clients; new or changed configs are reconnected.
    pub async fn config_diff(&self, desired: &[McpServerConfig]) -> McpConfigDiff {
        let mut diff = McpConfigDiff::default();
        let connected = self.list_servers().await;
        let previous = self.configs.read().await;

        for cfg in desired {
            if !cfg.enabled {
                continue;
            }
            if connected.iter().any(|name| name == &cfg.name) {
                // Name match is NOT enough: a changed URL/command/args must
                // take effect — compare against the config the live client
                // was built from (the last `sync_configs` snapshot). A
                // changed server is reconnected (removed + re-added so the
                // tools are unregistered before the fresh connect).
                let changed = previous.get(&cfg.name).is_some_and(|old| old != cfg);
                if changed {
                    diff.removed.push(cfg.name.clone());
                    diff.added.push(cfg.name.clone());
                } else {
                    diff.retained.push(cfg.name.clone());
                }
            } else {
                diff.added.push(cfg.name.clone());
            }
        }

        for name in &connected {
            let still_desired = desired.iter().any(|cfg| cfg.enabled && &cfg.name == name);
            if !still_desired {
                diff.removed.push(name.clone());
            }
        }

        diff
    }

    /// Synchronize connections to match the desired config list.
    ///
    /// Connects new servers and disconnects removed ones, keeping
    /// unchanged servers alive. Also records the desired configs so the
    /// pool's reconnect handler can re-establish dropped connections.
    /// Returns the number of servers connected.
    pub async fn sync_configs(
        &self,
        desired: &[McpServerConfig],
        registry: Arc<ToolRegistry>,
        app: &AppHandle,
    ) -> AppResult<usize> {
        let diff = self.config_diff(desired).await;

        // Remember the desired configs + registry for reconnection.
        {
            let mut configs = self.configs.write().await;
            configs.clear();
            for cfg in desired.iter().filter(|c| c.enabled) {
                configs.insert(cfg.name.clone(), cfg.clone());
            }
        }
        *self.registry.write().await = Some(registry.clone());

        for name in &diff.removed {
            if let Err(e) = self.disconnect(name).await {
                warn!(server = %name, error = %e, "Failed to disconnect removed MCP server");
            }
        }

        let mut connected_count = 0;
        for cfg in desired.iter().filter(|c| c.enabled) {
            if !diff.added.iter().any(|n| n == &cfg.name) {
                continue;
            }
            match self.connect(cfg, &registry, app).await {
                Ok(()) => connected_count += 1,
                Err(e) => warn!(server = %cfg.name, error = %e, "Failed to connect MCP server"),
            }
        }

        Ok(connected_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(name: &str, enabled: bool) -> McpServerConfig {
        McpServerConfig {
            name: name.to_string(),
            transport_type: "stdio".to_string(),
            command: Some("npx".to_string()),
            args: vec![],
            env: std::collections::HashMap::new(),
            url: None,
            enabled,
        }
    }

    #[tokio::test]
    async fn diff_marks_added_removed_retained() {
        let manager = McpManager::new();
        // Simulate two already-connected servers.
        manager
            .clients
            .write()
            .await
            .insert("keep".to_string(), Arc::new(McpClient::default()));
        manager
            .clients
            .write()
            .await
            .insert("drop".to_string(), Arc::new(McpClient::default()));

        let desired = vec![cfg("keep", true), cfg("new", true), cfg("disabled", false)];

        let diff = manager.config_diff(&desired).await;
        assert_eq!(diff.added, vec!["new"]);
        assert_eq!(diff.removed, vec!["drop"]);
        assert_eq!(diff.retained, vec!["keep"]);
    }

    #[tokio::test]
    async fn diff_reconnects_when_config_content_changes() {
        // Same server NAME with a changed URL must be reconnected — a
        // name-only diff would keep the stale connection to the old endpoint
        // until restart (settings changes silently not taking effect).
        let manager = McpManager::new();
        manager
            .clients
            .write()
            .await
            .insert("srv".to_string(), Arc::new(McpClient::default()));

        let mut old = cfg("srv", true);
        old.url = Some("http://old.example".into());
        manager.configs.write().await.insert("srv".to_string(), old);

        let mut desired = cfg("srv", true);
        desired.url = Some("http://new.example".into());

        let diff = manager.config_diff(&[desired]).await;
        assert!(
            diff.removed.contains(&"srv".to_string()),
            "changed server must be disconnected first"
        );
        assert!(
            diff.added.contains(&"srv".to_string()),
            "changed server must be reconnected"
        );
        assert!(!diff.retained.contains(&"srv".to_string()));
    }

    #[tokio::test]
    async fn diff_keeps_retained_when_config_unchanged() {
        let manager = McpManager::new();
        manager
            .clients
            .write()
            .await
            .insert("srv".to_string(), Arc::new(McpClient::default()));

        let mut same = cfg("srv", true);
        same.url = Some("http://same.example".into());
        manager
            .configs
            .write()
            .await
            .insert("srv".to_string(), same.clone());

        let diff = manager.config_diff(&[same]).await;
        assert_eq!(diff.retained, vec!["srv"]);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
    }

    #[tokio::test]
    async fn diff_all_new_when_nothing_connected() {
        let manager = McpManager::new();
        let desired = vec![cfg("a", true), cfg("b", true)];
        let diff = manager.config_diff(&desired).await;
        assert_eq!(diff.added, vec!["a", "b"]);
        assert!(diff.removed.is_empty());
        assert!(diff.retained.is_empty());
    }

    #[tokio::test]
    async fn diff_ignores_disabled_servers() {
        let manager = McpManager::new();
        let desired = vec![cfg("off", false)];
        let diff = manager.config_diff(&desired).await;
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
    }

    #[tokio::test]
    async fn remembered_manual_config_drives_reconnect_diff() {
        // A server connected manually from settings must be tracked like a
        // startup-synced one: a config edit is detected as remove+add so
        // the reconnect path can rebuild it after a drop.
        let manager = McpManager::new();
        let mut manual = cfg("manual", true);
        manual.url = Some("http://old.example".into());
        manager.remember_config(&manual).await;

        manager
            .clients
            .write()
            .await
            .insert("manual".to_string(), Arc::new(McpClient::default()));

        let mut changed = manual.clone();
        changed.url = Some("http://new.example".into());
        let diff = manager.config_diff(&[changed]).await;
        assert!(
            diff.removed.contains(&"manual".to_string()),
            "changed manual server must be disconnected first"
        );
        assert!(
            diff.added.contains(&"manual".to_string()),
            "changed manual server must be reconnected"
        );
    }

    #[tokio::test]
    async fn forget_config_removes_reconnect_source() {
        let manager = McpManager::new();
        let server = cfg("gone", true);
        manager.remember_config(&server).await;
        assert!(
            manager.configs.read().await.contains_key("gone"),
            "remembered config is present"
        );
        manager.forget_config("gone").await;
        assert!(
            !manager.configs.read().await.contains_key("gone"),
            "forgotten config cannot resurrect the server"
        );
    }

    #[tokio::test]
    async fn remember_config_skips_disabled_servers() {
        let manager = McpManager::new();
        let off = cfg("off", false);
        manager.remember_config(&off).await;
        assert!(
            !manager.configs.read().await.contains_key("off"),
            "disabled servers are never remembered for reconnect"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_unknown_server() {
        let manager = McpManager::new();
        let err = manager
            .proxy_ui_request("ghost", "tools/call", json!({"name": "x"}))
            .await
            .expect_err("unknown server errors");
        assert!(err.to_string().contains("not connected"));
    }

    #[tokio::test]
    async fn proxy_rejects_non_whitelisted_methods() {
        let manager = McpManager::new();
        manager
            .clients
            .write()
            .await
            .insert("srv".to_string(), Arc::new(McpClient::default()));

        for method in ["ui/initialize", "prompts/get", "anything/else"] {
            let err = manager
                .proxy_ui_request("srv", method, json!({}))
                .await
                .expect_err("non-whitelisted method rejected");
            assert!(
                err.to_string().contains("rejects"),
                "unexpected error for {method}: {err}"
            );
        }
    }

    #[tokio::test]
    async fn proxy_tools_call_requires_name() {
        let manager = McpManager::new();
        manager
            .clients
            .write()
            .await
            .insert("srv".to_string(), Arc::new(McpClient::default()));

        let err = manager
            .proxy_ui_request("srv", "tools/call", json!({}))
            .await
            .expect_err("missing name rejected");
        assert!(err.to_string().contains("'name'"));
    }
}
