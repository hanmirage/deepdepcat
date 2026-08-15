//! LSP client — spawns a language server process and speaks JSON-RPC
//! over stdio.
//!
//! Lifecycle:
//! 1. `LspClient::start` spawns the detected server binary
//! 2. `initialize` negotiates capabilities (with startup timeout)
//! 3. `did_open`/`did_change` keep the server's document state in sync
//! 4. `request` dispatches a request and matches the response by id
//!
//! The reader loop is a background task; pending requests await a
//! oneshot channel keyed by request id. Requests time out after
//! [`REQUEST_TIMEOUT`].

use super::location_from_uri;
use super::protocol::{self, Position, Range};
use crate::core::error::{AppError, AppResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};

/// Timeout for individual requests.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// Startup grace period for slow servers (rust-analyzer cold start).
const INIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Pull-diagnostic retries (covers project discovery + analysis time).
const DIAGNOSTIC_ATTEMPTS: usize = 6;

/// Supported language server flavors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerKind {
    Rust,
    TypeScript,
    Python,
    Go,
    C,
}

impl ServerKind {
    fn extra_args(&self) -> &'static [&'static str] {
        match self {
            Self::Rust | Self::Go | Self::C => &[],
            Self::TypeScript | Self::Python => &["--stdio"],
        }
    }
}

/// Project markers that identify a language project root.
fn project_marker(root: &Path) -> Option<(&'static str, ServerKind)> {
    if root.join("Cargo.toml").exists() {
        Some(("rust-analyzer", ServerKind::Rust))
    } else if root.join("package.json").exists() || root.join("tsconfig.json").exists() {
        Some(("typescript-language-server", ServerKind::TypeScript))
    } else if root.join("pyproject.toml").exists() || root.join("requirements.txt").exists() {
        Some(("pyright-langserver", ServerKind::Python))
    } else if root.join("go.mod").exists() {
        Some(("gopls", ServerKind::Go))
    } else if root.join("CMakeLists.txt").exists()
        || root.join("compile_commands.json").exists()
        || root.join(".clangd").exists()
    {
        Some(("clangd", ServerKind::C))
    } else {
        None
    }
}

/// Candidate (server, kind) pairs for a directory, honouring the primary
/// marker plus alternate binaries for the same project type.
fn candidates_for(root: &Path) -> Vec<(&'static str, ServerKind)> {
    match project_marker(root) {
        Some((_, ServerKind::TypeScript)) => vec![
            ("typescript-language-server", ServerKind::TypeScript),
            ("vscode-typescript-language-server", ServerKind::TypeScript),
        ],
        Some((_, kind)) => vec![marker_binary(kind)],
        None => Vec::new(),
    }
}

fn marker_binary(kind: ServerKind) -> (&'static str, ServerKind) {
    match kind {
        ServerKind::Rust => ("rust-analyzer", ServerKind::Rust),
        ServerKind::TypeScript => ("typescript-language-server", ServerKind::TypeScript),
        ServerKind::Python => ("pyright-langserver", ServerKind::Python),
        ServerKind::Go => ("gopls", ServerKind::Go),
        ServerKind::C => ("clangd", ServerKind::C),
    }
}

/// Fallback when no project marker exists: probe source files by extension.
fn fallback_by_extension(root: &Path) -> Vec<(&'static str, ServerKind)> {
    let mut probe: Vec<(&str, ServerKind)> = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    // Probe the root and its immediate children (bounded walk, no recursion).
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                dirs.push(entry.path());
            }
        }
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
                continue;
            };
            let kind = match ext {
                "rs" => ServerKind::Rust,
                "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => ServerKind::TypeScript,
                "py" => ServerKind::Python,
                "go" => ServerKind::Go,
                "c" | "h" | "cpp" | "hpp" | "cc" => ServerKind::C,
                _ => continue,
            };
            if !probe.iter().any(|(_, k)| *k == kind) {
                probe.push(marker_binary(kind));
            }
        }
        if !probe.is_empty() {
            break;
        }
    }
    probe
}

/// Locate the nearest project root by walking up from `root` (max 6 levels),
/// so subdirectories of a project still resolve to the project root.
fn find_project_root(root: &Path) -> Option<PathBuf> {
    let mut current = Some(root.to_path_buf());
    for _ in 0..6 {
        let dir = current?;
        if project_marker(&dir).is_some() {
            return Some(dir);
        }
        current = dir.parent().map(Path::to_path_buf);
    }
    None
}

/// Detect the language server for a workspace root.
///
/// Returns `(binary_name, kind)` when a project marker and the server
/// binary both exist. Markers are searched upward from the workspace root;
/// when none exist, source files in the root (or its immediate children)
/// provide a fallback, and C/C++ projects are supported via clangd.
pub fn detect_server(root: &Path) -> Option<(String, ServerKind)> {
    let project_root = find_project_root(root);
    let mut candidates = match &project_root {
        Some(pr) => candidates_for(pr),
        None => Vec::new(),
    };
    if candidates.is_empty() {
        candidates = fallback_by_extension(root);
    }

    candidates
        .iter()
        .find_map(|(name, kind)| which(name).map(|p| (p.to_string_lossy().into_owned(), *kind)))
}

/// Find a binary in PATH plus well-known install directories.
///
/// Windows GUI apps launched from the desktop may not inherit the shell
/// PATH, so besides `~/.cargo/bin` (rustup) we also probe the npm global
/// bin directory (`%APPDATA%\npm`) where typescript-language-server and
/// pyright are commonly installed.
///
/// On Windows, npm global packages install as shims — `name.cmd`,
/// `name.ps1` and an extensionless POSIX shim — NOT as `.exe`. Each
/// directory is probed in priority order `.exe` → `.cmd` → `.ps1` →
/// extensionless so `npm i -g typescript-language-server` is actually
/// found (previously the hard-coded `.exe` suffix made every npm-installed
/// server invisible → "No language server found" after a successful install).
fn which(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".cargo").join("bin"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join("npm"));
    }
    if let Some(programfiles) = std::env::var_os("ProgramFiles") {
        dirs.push(PathBuf::from(programfiles).join("nodejs"));
    }

    for dir in dirs {
        if cfg!(windows) {
            // npm shim priority order within each directory.
            for candidate in [
                format!("{name}.exe"),
                format!("{name}.cmd"),
                format!("{name}.ps1"),
                name.to_string(),
            ] {
                let path = dir.join(&candidate);
                if path.is_file() {
                    return Some(path);
                }
            }
        } else {
            let path = dir.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Whether a resolved server path is a Windows script shim that
/// `CreateProcess` cannot launch directly.
fn is_cmd_shim(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    matches!(ext.as_deref(), Some("cmd") | Some("bat"))
}

fn is_ps1_shim(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        == Some("ps1")
}

/// A pending request: response id → oneshot sender.
type PendingRequest = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

/// The LSP client — one per workspace.
pub struct LspClient {
    server_binary: String,
    child: Mutex<Child>,
    /// Shared writer to the server's stdin.
    writer: Mutex<BufWriter<tokio::process::ChildStdin>>,
    /// Pending request ids → response senders.
    pending: PendingRequest,
    next_id: std::sync::atomic::AtomicU64,
    /// Documents currently open in the server: uri → (last synced mtime, version).
    open_documents: Mutex<HashMap<String, (u64, u64)>>,
    /// Negotiated server capabilities.
    capabilities: Mutex<Capabilities>,
}

/// Server capabilities negotiated at initialization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub diagnostics: bool,
    pub definition: bool,
    pub references: bool,
    pub formatting: bool,
    pub document_symbols: bool,
    pub hover: bool,
    pub workspace_symbols: bool,
}

impl LspClient {
    /// Spawn the server process and start the reader loop.
    ///
    /// Does not `initialize` yet — the caller decides when (first use),
    /// keeping cold-start cost out of startup.
    pub async fn start(
        workspace_root: PathBuf,
        server_binary: String,
        server_kind: ServerKind,
    ) -> AppResult<Arc<Self>> {
        let binary_path = Path::new(&server_binary);
        // Windows: npm shims (.cmd/.bat/.ps1) cannot be spawned directly —
        // CreateProcess only runs PE executables. Wrap the shim in its
        // interpreter so `npm i -g typescript-language-server` works.
        let mut cmd = if is_cmd_shim(binary_path) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&server_binary);
            c
        } else if is_ps1_shim(binary_path) {
            let mut c = Command::new("powershell");
            c.args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                &server_binary,
            ]);
            c
        } else {
            Command::new(&server_binary)
        };
        crate::core::proc::no_window_tokio(&mut cmd);
        cmd.args(server_kind.extra_args())
            .current_dir(&workspace_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| {
            AppError::Internal(format!("Failed to spawn LSP server '{server_binary}': {e}"))
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::Internal("LSP server stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::Internal("LSP server stdout unavailable".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AppError::Internal("LSP server stderr unavailable".into()))?;

        // Collect server stderr into tracing for diagnostics.
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => tracing::debug!(target: "lsp_stderr", "{}", line.trim_end()),
                    Err(_) => break,
                }
            }
        });

        let client = Arc::new(Self {
            server_binary,
            child: Mutex::new(child),
            writer: Mutex::new(BufWriter::new(stdin)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: std::sync::atomic::AtomicU64::new(1),
            open_documents: Mutex::new(HashMap::new()),
            capabilities: Mutex::new(Capabilities::default()),
        });

        // Reader loop: parse frames, route to pending requests or ignore
        // server-initiated requests (notification-based flows are out of
        // scope; pull diagnostics avoids the push channel entirely).
        // The task holds only the pending map (not the client), so the
        // client can be dropped — killing the server — when unused.
        let pending = client.pending.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                match protocol::read_frame(&mut reader).await {
                    Ok(Some(body)) => {
                        let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
                            continue;
                        };
                        if let Some(id) = protocol::message_id(&msg) {
                            let sender = pending.lock().await.remove(&id);
                            if let Some(tx) = sender {
                                let result = match protocol::response_result(&msg) {
                                    Some(r) => Ok(r.clone()),
                                    None => Err(protocol::response_error(&msg)
                                        .unwrap_or_else(|| "no result in response".to_string())),
                                };
                                let _ = tx.send(result);
                            }
                        }
                        // Notifications and server requests are ignored.
                    }
                    Ok(None) => break, // EOF — server exited
                    Err(e) => {
                        tracing::warn!(error = %e, "LSP reader loop error");
                        break;
                    }
                }
            }
            // Unblock all pending requests.
            let mut pend = pending.lock().await;
            for (_, tx) in pend.drain() {
                let _ = tx.send(Err("LSP server exited".to_string()));
            }
        });

        Ok(client)
    }

    /// Whether the server is still alive.
    pub async fn is_alive(&self) -> bool {
        self.child.lock().await.try_wait().ok().flatten().is_none()
    }

    /// Negotiate capabilities with the server (`initialize` request).
    pub async fn initialize(&self, root_uri: &str) -> AppResult<()> {
        let params = json!({
            "processId": null,
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "diagnostic": { "dynamicRegistration": false },
                    "definition": { "dynamicRegistration": false },
                    "references": { "dynamicRegistration": false },
                    "formatting": { "dynamicRegistration": false },
                    "hover": { "dynamicRegistration": false }
                }
            },
            "workspace": {
                "symbol": { "dynamicRegistration": false }
            },
            "workspaceFolders": [{ "uri": root_uri, "name": "workspace" }]
        });
        let result = tokio::time::timeout(INIT_TIMEOUT, self.request("initialize", params))
            .await
            .map_err(|_| AppError::Internal("LSP initialize timed out".to_string()))??;

        let caps = result.get("capabilities").cloned().unwrap_or_default();
        let negotiated = Capabilities {
            // All capability probes live at the TOP level of
            // ServerCapabilities (LSP 3.0–3.17). rust-analyzer, tsserver,
            // pyright and gopls all report them there.
            diagnostics: caps
                .get("diagnosticProvider")
                .map(|p| !p.is_null())
                .unwrap_or(false),
            definition: caps
                .get("definitionProvider")
                .map(|p| p.is_boolean() || p.is_object())
                .unwrap_or(false),
            references: caps
                .get("referencesProvider")
                .map(|p| p.is_boolean() || p.is_object())
                .unwrap_or(false),
            formatting: caps
                .get("documentFormattingProvider")
                .map(|p| p.is_boolean() || p.is_object())
                .unwrap_or(false),
            document_symbols: caps
                .get("documentSymbolProvider")
                .map(|p| p.is_boolean() || p.is_object())
                .unwrap_or(false),
            hover: caps
                .get("hoverProvider")
                .map(|p| p.is_boolean() || p.is_object())
                .unwrap_or(false),
            workspace_symbols: caps
                .get("workspaceSymbolProvider")
                .map(|p| p.is_boolean() || p.is_object())
                .unwrap_or(false),
        };

        tracing::info!(
          server = %self.server_binary,
          diagnostics = negotiated.diagnostics,
          definition = negotiated.definition,
            references = negotiated.references,
            formatting = negotiated.formatting,
            document_symbols = negotiated.document_symbols,
            hover = negotiated.hover,
            workspace_symbols = negotiated.workspace_symbols,
            "LSP server initialized"
        );

        *self.capabilities.lock().await = negotiated;
        self.notify("initialized", json!({})).await?;
        Ok(())
    }

    /// Whether the client is initialized.
    pub async fn capabilities(&self) -> Capabilities {
        self.capabilities.lock().await.clone()
    }

    /// Ensure a document is open in the server with its current content.
    ///
    /// Tracks the last-synced file modification time: a document is opened
    /// with `didOpen` on first use, and re-synced with a full-content
    /// `didChange` whenever the file changed on disk since the last sync
    /// (e.g. after an edit tool modified it).
    pub async fn sync_document(&self, uri: &str, language_id: &str) -> AppResult<()> {
        let path = protocol::uri_to_path(uri)
            .ok_or_else(|| AppError::Internal(format!("invalid document URI: {uri}")))?;
        let current_mtime = file_mtime_nanos(&path).await;

        let (known_mtime, known_version) = {
            let open = self.open_documents.lock().await;
            open.get(uri)
                .copied()
                .map(|(m, v)| (Some(m), v))
                .unwrap_or((None, 0))
        };

        if let Some(known) = known_mtime {
            if known == current_mtime {
                return Ok(()); // already in sync
            }
        }

        let text = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        let version = known_version + 1;

        match known_mtime {
            None => {
                self.notify(
                    "textDocument/didOpen",
                    json!({
                        "textDocument": {
                            "uri": uri,
                            "languageId": language_id,
                            "version": version,
                            "text": text
                        }
                    }),
                )
                .await?;
            }
            Some(_) => {
                self.notify(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": uri, "version": version },
                        "contentChanges": [{ "text": text }]
                    }),
                )
                .await?;
            }
        }

        self.open_documents
            .lock()
            .await
            .insert(uri.to_string(), (current_mtime, version));
        Ok(())
    }

    /// Send a request and await its response.
    async fn request(&self, method: &str, params: Value) -> AppResult<Value> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id, tx);
        }

        let frame = serde_json::to_vec(&protocol::request(id, method, params))
            .map_err(|e| AppError::Internal(format!("JSON encode failed: {e}")))?;
        {
            let mut writer = self.writer.lock().await;
            writer.write_all(&protocol::encode_frame(&frame)).await?;
            writer.flush().await?;
        }

        let outcome = tokio::time::timeout(REQUEST_TIMEOUT, rx).await;
        match outcome {
            Ok(Ok(result)) => {
                result.map_err(|e| AppError::Internal(format!("LSP '{method}' failed: {e}")))
            }
            Ok(Err(_)) => {
                // The reader loop already removed the entry before sending —
                // nothing to clean up.
                Err(AppError::Internal(format!(
                    "LSP request '{method}' dropped"
                )))
            }
            Err(_) => {
                // Timed out — the reader loop will never answer this id, so
                // the parked entry would leak forever. Drop it (a late
                // response arriving afterwards is simply ignored).
                self.pending.lock().await.remove(&id);
                Err(AppError::Internal(format!(
                    "LSP request '{method}' timed out"
                )))
            }
        }
    }

    /// Send a fire-and-forget notification.
    async fn notify(&self, method: &str, params: Value) -> AppResult<()> {
        let frame = serde_json::to_vec(&protocol::notification(method, params))
            .map_err(|e| AppError::Internal(format!("JSON encode failed: {e}")))?;
        let mut writer = self.writer.lock().await;
        writer.write_all(&protocol::encode_frame(&frame)).await?;
        writer.flush().await?;
        Ok(())
    }

    // ── Document operations ─────────────────────────────────────────────

    /// Pull diagnostics for a file (LSP 3.17 pull model).
    ///
    /// Servers analyze asynchronously after `didOpen`/`didChange` and a
    /// freshly spawned server needs time to discover the project via
    /// cargo metadata; the pull is retried with a short delay until it
    /// returns items (or the attempts are exhausted).
    pub async fn diagnostics(
        &self,
        file: &Path,
        language_id: &str,
    ) -> AppResult<Vec<super::LspDiagnostic>> {
        if !self.capabilities().await.diagnostics {
            return Ok(Vec::new());
        }
        let uri = protocol::path_to_uri(file);
        self.sync_document(&uri, language_id).await?;

        for attempt in 0..DIAGNOSTIC_ATTEMPTS {
            let result = match self
                .request(
                    "textDocument/diagnostic",
                    json!({ "textDocument": { "uri": uri } }),
                )
                .await
            {
                Ok(r) => r,
                Err(e) => {
                    // "file not found" means the server has not discovered
                    // the project yet (cargo metadata still loading);
                    // "server cancelled the request" happens when the
                    // server re-schedules analysis mid-startup. Retry
                    // both rather than surface them.
                    let retriable = e.to_string().contains("file not found")
                        || e.to_string().contains("cancelled the request");
                    if retriable && attempt + 1 < DIAGNOSTIC_ATTEMPTS {
                        tokio::time::sleep(Duration::from_millis(800)).await;
                        continue;
                    }
                    return Err(e);
                }
            };

            let items = result
                .get("kind")
                .and_then(|k| k.as_str())
                .and_then(|k| (k == "full").then_some(()))
                .and_then(|_| result.get("items"))
                .and_then(|i| i.as_array())
                .cloned()
                .unwrap_or_default();

            if !items.is_empty() || attempt + 1 == DIAGNOSTIC_ATTEMPTS {
                return Ok(parse_diagnostics(file, items));
            }
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
        Ok(Vec::new())
    }

    /// Go to definition at a position.
    pub async fn definition(
        &self,
        file: &Path,
        pos: Position,
    ) -> AppResult<Vec<super::LspLocation>> {
        if !self.capabilities().await.definition {
            return Ok(Vec::new());
        }
        let uri = protocol::path_to_uri(file);
        let result = self
            .request(
                "textDocument/definition",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": pos.line, "character": pos.character }
                }),
            )
            .await?;

        Ok(parse_locations(result))
    }

    /// Find all references to the symbol at a position.
    pub async fn references(
        &self,
        file: &Path,
        pos: Position,
    ) -> AppResult<Vec<super::LspLocation>> {
        if !self.capabilities().await.references {
            return Ok(Vec::new());
        }
        let uri = protocol::path_to_uri(file);
        let result = self
            .request(
                "textDocument/references",
                json!({
                    "textDocument": { "uri": uri },
                    "position": { "line": pos.line, "character": pos.character },
                    "context": { "includeDeclaration": true }
                }),
            )
            .await?;

        Ok(parse_locations(result))
    }

    /// Fetch the document symbol outline (functions/classes/variables…).
    /// Returns the RAW LSP result; the tool parses/flattens it.
    pub async fn document_symbols(&self, file: &Path) -> AppResult<Value> {
        if !self.capabilities().await.document_symbols {
            return Ok(serde_json::Value::Array(Vec::new()));
        }
        let uri = protocol::path_to_uri(file);
        self.request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri } }),
        )
        .await
    }

    /// Fetch hover documentation at a position.
    /// Returns the RAW LSP result; the tool parses it to text.
    pub async fn hover(&self, file: &Path, pos: Position) -> AppResult<Value> {
        if !self.capabilities().await.hover {
            return Ok(serde_json::Value::Null);
        }
        let uri = protocol::path_to_uri(file);
        self.request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": uri },
                "position": { "line": pos.line, "character": pos.character }
            }),
        )
        .await
    }

    /// Search workspace-wide symbols by query (`workspace/symbol`).
    /// Returns the RAW LSP result; the tool parses it to locations.
    pub async fn workspace_symbols(&self, query: &str) -> AppResult<Value> {
        if !self.capabilities().await.workspace_symbols {
            return Ok(serde_json::Value::Array(Vec::new()));
        }
        self.request("workspace/symbol", json!({ "query": query }))
            .await
    }

    /// Format a document; returns the new full text.
    pub async fn format(&self, file: &Path) -> AppResult<Option<String>> {
        if !self.capabilities().await.formatting {
            return Ok(None);
        }
        let uri = protocol::path_to_uri(file);
        self.sync_document(&uri, language_id_for_path(file)).await?;

        let result = self
            .request(
                "textDocument/formatting",
                json!({
                    "textDocument": { "uri": uri },
                    "options": { "tabSize": 4, "insertSpaces": true }
                }),
            )
            .await?;

        let edits = result.as_array().cloned().unwrap_or_default();
        if edits.is_empty() {
            return Ok(None);
        }
        let text = tokio::fs::read_to_string(file).await.unwrap_or_default();
        Ok(Some(apply_edits(&text, &edits)))
    }
}

/// Parse a definition/references response into locations.
///
/// Handles `Location | Location[] | LocationLink[]`.
fn parse_locations(result: Value) -> Vec<super::LspLocation> {
    let mut out = Vec::new();
    match result {
        Value::Array(items) => {
            for item in items {
                push_location(&mut out, &item);
            }
        }
        other => push_location(&mut out, &other),
    }
    out
}

fn push_location(out: &mut Vec<super::LspLocation>, item: &Value) {
    // LocationLink has `targetUri`/`targetRange`; Location has `uri`/`range`.
    let uri = item
        .get("targetUri")
        .or_else(|| item.get("uri"))
        .and_then(|u| u.as_str());
    let range = item
        .get("targetRange")
        .or_else(|| item.get("range"))
        .and_then(|r| {
            let start = r.get("start")?;
            let end = r.get("end")?;
            Some(Range {
                start: Position {
                    line: start.get("line")?.as_u64()? as u32,
                    character: start.get("character")?.as_u64()? as u32,
                },
                end: Position {
                    line: end.get("line")?.as_u64()? as u32,
                    character: end.get("character")?.as_u64()? as u32,
                },
            })
        });

    if let (Some(uri), Some(range)) = (uri, range) {
        if let Some(loc) = location_from_uri(uri, range) {
            out.push(loc);
        }
    }
}

/// Apply a list of text edits (assumed sorted, non-overlapping) to a document.
fn apply_edits(text: &str, edits: &[Value]) -> String {
    let mut positions: Vec<(usize, usize, &str)> = Vec::new();
    for edit in edits {
        let new_text = edit.get("newText").and_then(|t| t.as_str()).unwrap_or("");
        let start = edit
            .pointer("/range/start")
            .and_then(|s| offset_for_position(text, s));
        let end = edit
            .pointer("/range/end")
            .and_then(|e| offset_for_position(text, e));
        if let (Some(start), Some(end)) = (start, end) {
            positions.push((start, end, new_text));
        }
    }
    positions.sort_by_key(|(s, _, _)| *s);

    let mut result = String::with_capacity(text.len() + 256);
    let mut cursor = 0usize;
    for (start, end, replacement) in positions {
        if start < cursor || end < start {
            continue;
        }
        result.push_str(&text[cursor..start]);
        result.push_str(replacement);
        cursor = end;
    }
    result.push_str(&text[cursor..]);
    result
}

/// Convert a JSON position to a byte offset in `text`.
fn offset_for_position(text: &str, pos: &Value) -> Option<usize> {
    let line = pos.get("line")?.as_u64()? as usize;
    let character = pos.get("character")?.as_u64()? as usize;
    let mut current_line = 0usize;
    let mut offset = 0usize;
    for (i, ch) in text.char_indices() {
        if current_line == line {
            let col = text[i..]
                .char_indices()
                .nth(character)
                .map(|(c, _)| c)
                .unwrap_or(text.len() - i);
            return Some(i + col);
        }
        if ch == '\n' {
            current_line += 1;
            offset = i + 1;
        }
    }
    if current_line == line {
        return Some(text.len());
    }
    let _ = offset;
    None
}

/// Map a file path to an LSP language id.
pub fn language_id_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => "rust",
        Some("ts") | Some("tsx") => "typescript",
        Some("js") | Some("jsx") | Some("mjs") | Some("cjs") => "javascript",
        Some("py") => "python",
        Some("go") => "go",
        Some("json") => "json",
        Some("md") => "markdown",
        _ => "plaintext",
    }
}

/// Convert raw LSP diagnostic items into user-facing diagnostics.
fn parse_diagnostics(file: &Path, items: Vec<Value>) -> Vec<super::LspDiagnostic> {
    let mut diags = Vec::new();
    for item in items {
        let line = item
            .pointer("/range/start/line")
            .and_then(|l| l.as_u64())
            .unwrap_or(0) as u32;
        let severity = item.get("severity").and_then(|s| s.as_u64());
        let message = item
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();
        let source = item
            .get("source")
            .and_then(|s| s.as_str())
            .unwrap_or("lsp")
            .to_string();
        if !message.is_empty() {
            diags.push(super::LspDiagnostic {
                file: file.to_string_lossy().into_owned(),
                line,
                severity: protocol::severity_label(severity).to_string(),
                message,
                source,
            });
        }
    }
    diags.sort_by_key(|d| d.line);
    diags
}

/// File modification time in nanoseconds since the epoch (0 on error —
/// treated as "unknown but stable", so sync still happens once).
async fn file_mtime_nanos(path: &Path) -> u64 {
    tokio::fs::metadata(path)
        .await
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate the process-global PATH env var.
    static PATH_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn language_id_mapping() {
        assert_eq!(language_id_for_path(Path::new("a.rs")), "rust");
        assert_eq!(language_id_for_path(Path::new("a.tsx")), "typescript");
        assert_eq!(language_id_for_path(Path::new("a.py")), "python");
        assert_eq!(language_id_for_path(Path::new("a.go")), "go");
        assert_eq!(language_id_for_path(Path::new("a.txt")), "plaintext");
    }

    #[test]
    fn which_finds_cmd_shim_like_npm_install() {
        if !cfg!(windows) {
            return;
        }
        // PATH is process-global — serialize with the sibling which() test.
        let _guard = PATH_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        // Simulate an npm global install: only `name.cmd` + `name.ps1` +
        // extensionless shim exist — NO .exe (npm never creates one). The
        // hard-coded .exe probe previously made every npm-installed server
        // invisible; the cmd shim must now be found.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("typescript-language-server.cmd"),
            "@echo off",
        )
        .unwrap();
        std::fs::write(tmp.path().join("typescript-language-server.ps1"), "").unwrap();
        std::fs::write(tmp.path().join("typescript-language-server"), "#!/bin/sh").unwrap();

        // Inject the temp dir as the APPDATA/npm location via a controlled
        // probe: which() reads env PATH — prepend the temp dir to PATH so the
        // normal resolution order is exercised without touching real dirs.
        let prev = std::env::var_os("PATH");
        let augmented = std::env::join_paths(
            std::iter::once(tmp.path().to_path_buf()).chain(
                prev.as_ref()
                    .map(|p| std::env::split_paths(p))
                    .into_iter()
                    .flatten(),
            ),
        )
        .unwrap();
        std::env::set_var("PATH", &augmented);

        let found = which("typescript-language-server");
        let expected = Some(tmp.path().join("typescript-language-server.cmd"));
        assert_eq!(
            found, expected,
            "npm .cmd shim must be found (exe absent); got {:?}",
            found
        );

        if let Some(prev) = prev {
            std::env::set_var("PATH", prev);
        } else {
            std::env::remove_var("PATH");
        }
    }

    #[test]
    fn which_prefers_exe_over_cmd_shim() {
        if !cfg!(windows) {
            return;
        }
        let _guard = PATH_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        // When BOTH an .exe and a .cmd exist (e.g. pip Scripts dir next to
        // an npm shim), the native executable wins — priority order is
        // exe → cmd → ps1 → extensionless.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pyright-langserver.exe"), "MZ").unwrap();
        std::fs::write(tmp.path().join("pyright-langserver.cmd"), "@echo off").unwrap();

        let prev = std::env::var_os("PATH");
        let augmented = std::env::join_paths(
            std::iter::once(tmp.path().to_path_buf()).chain(
                prev.as_ref()
                    .map(|p| std::env::split_paths(p))
                    .into_iter()
                    .flatten(),
            ),
        )
        .unwrap();
        std::env::set_var("PATH", &augmented);

        assert_eq!(
            which("pyright-langserver"),
            Some(tmp.path().join("pyright-langserver.exe"))
        );

        if let Some(prev) = prev {
            std::env::set_var("PATH", prev);
        } else {
            std::env::remove_var("PATH");
        }
    }

    #[test]
    fn shim_classification() {
        assert!(is_cmd_shim(Path::new("C:\\npm\\tss.cmd")));
        assert!(is_cmd_shim(Path::new("C:\\npm\\tss.bat")));
        assert!(!is_cmd_shim(Path::new("C:\\cargo\\rust-analyzer.exe")));
        assert!(!is_cmd_shim(Path::new("C:\\npm\\tss")));
        assert!(is_ps1_shim(Path::new("C:\\npm\\tss.ps1")));
        assert!(!is_ps1_shim(Path::new("C:\\npm\\tss.cmd")));
    }

    #[tokio::test]
    async fn file_mtime_rounds_up_on_error() {
        // Missing file → 0 (stable fallback, sync happens once).
        assert_eq!(
            file_mtime_nanos(Path::new("C:\\nonexistent\\x.rs")).await,
            0
        );
    }

    #[tokio::test]
    async fn file_mtime_changes_after_write() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("a.txt");
        tokio::fs::write(&file, "v1").await.unwrap();
        let first = file_mtime_nanos(&file).await;
        // Force a distinguishable mtime on coarse-grained filesystems.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        tokio::fs::write(&file, "v2").await.unwrap();
        let second = file_mtime_nanos(&file).await;
        assert_ne!(first, second, "mtime must change after a write");
    }

    #[test]
    fn apply_edits_basic() {
        let edits = vec![json!({
            "range": { "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5} },
            "newText": "hello"
        })];
        let text = "world!";
        let formatted = apply_edits(text, &edits);
        assert_eq!(formatted, "hello!");
    }

    #[test]
    fn apply_edits_multiple_sorted() {
        let edits = vec![
            json!({
                "range": { "start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1} },
                "newText": "A"
            }),
            json!({
                "range": { "start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 1} },
                "newText": "B"
            }),
        ];
        let text = "x\ny";
        assert_eq!(apply_edits(text, &edits), "A\nB");
    }

    #[test]
    fn apply_edits_multiline() {
        let edits = vec![json!({
            "range": { "start": {"line": 1, "character": 0}, "end": {"line": 2, "character": 0} },
            "newText": "  middle\n"
        })];
        let text = "first\noldline\nlast";
        assert_eq!(apply_edits(text, &edits), "first\n  middle\nlast");
    }

    #[test]
    fn offset_for_position_handles_unicode() {
        let text = "héllo\nworld";
        let pos = json!({"line": 0, "character": 1});
        let offset = offset_for_position(text, &pos).unwrap();
        assert_eq!(text[offset..].chars().next(), Some('é'));
    }

    #[test]
    fn parse_locations_handles_links() {
        let result = json!([
            {
                "targetUri": "file:///C:/proj/src/lib.rs",
                "targetRange": {
                    "start": {"line": 3, "character": 4},
                    "end": {"line": 3, "character": 10}
                },
                "targetSelectionRange": {
                    "start": {"line": 3, "character": 4},
                    "end": {"line": 3, "character": 10}
                }
            }
        ]);
        let locations = parse_locations(result);
        assert_eq!(locations.len(), 1);
        assert!(locations[0].file.contains("lib.rs"));
        assert_eq!(locations[0].line, 3);
        assert_eq!(locations[0].character, 4);
    }

    /// Integration smoke test — spawns a real language server when one is
    /// on PATH (skipped otherwise): initialize handshake + pull diagnostics
    /// on a tiny workspace with an intentional type error.
    #[tokio::test]
    async fn real_server_smoke_test() {
        let Some((binary, kind)) = detect_server(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
        else {
            eprintln!("no rust-analyzer on PATH — skipping LSP smoke test");
            return;
        };
        let _ = (binary.clone(), kind);

        let tmp = tempfile::tempdir().expect("tempdir");
        let ws = tmp.path();
        std::fs::write(
            ws.join("Cargo.toml"),
            "[package]\nname = \"smoke\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(ws.join("src")).unwrap();
        std::fs::write(
            ws.join("src/main.rs"),
            "fn main() {\n    let x: i32 = \"oops\";\n    println!(\"{}\", x);\n}\n",
        )
        .unwrap();

        let client = LspClient::start(ws.to_path_buf(), binary, kind)
            .await
            .expect("spawn server");
        let root_uri = super::protocol::path_to_uri(ws);
        if let Err(e) = client.initialize(&root_uri).await {
            // e.g. a rustup shim pointing at a not-installed component —
            // the environment lacks a working server, so skip the smoke
            // assertions (the error path itself is already exercised).
            eprintln!("rust-analyzer unavailable ({e}) — skipping LSP smoke test");
            return;
        }

        // Pull diagnostics — rust-analyzer reports the type mismatch.
        // The server analyzes asynchronously after cold start (cargo
        // metadata + full crate analysis); poll until diagnostics appear.
        let file = ws.join("src/main.rs");
        let mut diags = Vec::new();
        for _ in 0..40 {
            match client.diagnostics(&file, "rust").await {
                Ok(d) if !d.is_empty() => {
                    diags = d;
                    break;
                }
                Ok(_) => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
                Err(e) => {
                    // Capability not supported — nothing to assert.
                    if e.to_string().contains("not supported") {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
        assert!(
            !diags.is_empty(),
            "expected at least one diagnostic for the type error"
        );
        assert!(
            diags.iter().any(|d| d.severity == "error"),
            "expected an error-level diagnostic: {diags:?}"
        );

        // Definition lookup on `main` (line 0, column 0-ish).
        let defs = client
            .definition(
                &file,
                Position {
                    line: 0,
                    character: 0,
                },
            )
            .await
            .expect("definition");
        assert!(
            !defs.is_empty(),
            "expected at least one definition location for main"
        );

        // Hover documentation on `main`.
        let raw_hover = client
            .hover(
                &file,
                Position {
                    line: 0,
                    character: 4,
                },
            )
            .await
            .expect("hover");
        assert!(
            super::super::parse_hover(&raw_hover).is_some(),
            "expected hover content for main"
        );

        // Workspace-wide symbol search.
        let raw_symbols = client
            .workspace_symbols("main")
            .await
            .expect("workspace symbols");
        let symbols = super::super::parse_workspace_symbols(&raw_symbols);
        assert!(
            !symbols.is_empty(),
            "expected at least one workspace symbol matching 'main'"
        );
    }
}
