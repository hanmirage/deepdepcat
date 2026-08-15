//! LSP tool — Language Server Protocol integration for code intelligence.
//!
//! Provides the agent with the ability to query a language server for:
//! - Go-to-definition
//! - Find references
//! - Format document
//! - Get diagnostics (pull model, LSP 3.17)
//!
//! Architecture:
//! - [`protocol`] — JSON-RPC framing over stdio + path↔URI conversion
//! - [`client`] — per-workspace server process, request/response matching
//! - [`LspManager`] — shared registry of per-workspace clients
//! - [`LspTool`] — the `lsp` tool exposed to the agent

pub mod client;
pub mod protocol;

use crate::toolkit::{Tool, ToolContext, ToolResult};
use crate::core::error::{AppError, AppResult};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;
use tracing::info;

use client::{detect_server, language_id_for_path, LspClient};
use protocol::{Position, Range};

/// A location in a source file (converted from an LSP URI + range).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspLocation {
    pub file: String,
    pub line: u32,
    pub character: u32,
}

/// Convert an LSP URI + range into a user-facing location.
fn location_from_uri(uri: &str, range: Range) -> Option<LspLocation> {
    let path = protocol::uri_to_path(uri)?;
    Some(LspLocation {
        file: path.to_string_lossy().into_owned(),
        line: range.start.line,
        character: range.start.character,
    })
}

/// A diagnostic message from the language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnostic {
    pub file: String,
    pub line: u32,
    pub severity: String,
    pub message: String,
    pub source: String,
}

/// One document-symbol outline entry (flattened, hierarchical children
/// prefixed with dots).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSymbol {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub character: u32,
}

/// One workspace-symbol result (`workspace/symbol`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspWorkspaceSymbol {
    pub name: String,
    pub kind: String,
    pub container: Option<String>,
    pub file: String,
    pub line: u32,
    pub character: u32,
}

/// LSP SymbolKind number → human label.
pub fn symbol_kind_label(kind: u64) -> &'static str {
    match kind {
        1 => "File",
        2 => "Module",
        3 => "Namespace",
        4 => "Package",
        5 => "Class",
        6 => "Method",
        7 => "Property",
        8 => "Field",
        9 => "Constructor",
        10 => "Enum",
        11 => "Interface",
        12 => "Function",
        13 => "Variable",
        14 => "Constant",
        15 => "String",
        16 => "Number",
        17 => "Boolean",
        18 => "Array",
        19 => "Object",
        20 => "Key",
        21 => "Null",
        22 => "EnumMember",
        23 => "Struct",
        24 => "Event",
        25 => "Operator",
        26 => "TypeParameter",
        _ => "Symbol",
    }
}

/// Parse a `textDocument/documentSymbol` result (both SymbolInformation and
/// hierarchical DocumentSymbol shapes) into a flat outline.
pub fn parse_document_symbols(value: &Value) -> Vec<LspSymbol> {
    fn walk(item: &Value, prefix: &str, out: &mut Vec<LspSymbol>) {
        let Some(name) = item.get("name").and_then(|v| v.as_str()) else {
            return;
        };
        let kind = item.get("kind").and_then(|v| v.as_u64()).unwrap_or(0);
        let start = item
            .get("range")
            .and_then(|r| r.get("start"))
            .or_else(|| item.get("selectionRange").and_then(|r| r.get("start")))
            .or_else(|| {
                item.get("location")
                    .and_then(|l| l.get("range"))
                    .and_then(|r| r.get("start"))
            });
        let (line, character) = start
            .map(|s| {
                (
                    s.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    s.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                )
            })
            .unwrap_or((0, 0));
        let full = if prefix.is_empty() {
            name.to_string()
        } else {
            format!("{prefix}.{name}")
        };
        out.push(LspSymbol {
            name: full.clone(),
            kind: symbol_kind_label(kind).to_string(),
            line,
            character,
        });
        if let Some(children) = item.get("children").and_then(|c| c.as_array()) {
            for child in children {
                walk(child, &full, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(arr) = value.as_array() {
        for item in arr {
            walk(item, "", &mut out);
        }
    } else {
        walk(value, "", &mut out);
    }
    out
}

/// Extract the text from one hover content item (string, MarkupContent or
/// MarkedString). Empty values are treated as absent.
fn hover_part_text(part: &Value) -> Option<String> {
    if let Some(text) = part.as_str() {
        return (!text.is_empty()).then(|| text.to_string());
    }
    let text = part.get("value").and_then(|v| v.as_str())?;
    (!text.is_empty()).then(|| text.to_string())
}

/// Parse a `textDocument/hover` result into plain text.
///
/// Accepts `MarkupContent`, `MarkedString`, a string, or an array mixing
/// those shapes. Multiple parts are joined with a blank line.
pub fn parse_hover(value: &Value) -> Option<String> {
    let contents = value.get("contents")?;
    let mut parts: Vec<String> = Vec::new();
    if let Some(items) = contents.as_array() {
        for item in items {
            if let Some(text) = hover_part_text(item) {
                parts.push(text);
            }
        }
    } else if let Some(text) = hover_part_text(contents) {
        parts.push(text);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// Parse a `workspace/symbol` result into user-facing locations.
///
/// Handles both `SymbolInformation` (`location.range`) and LSP 3.17
/// `WorkspaceSymbol` (full `location` or URI-only `location`).
pub fn parse_workspace_symbols(value: &Value) -> Vec<LspWorkspaceSymbol> {
    let mut out = Vec::new();
    let Some(items) = value.as_array() else {
        return out;
    };
    for item in items {
        let Some(name) = item.get("name").and_then(|v| v.as_str()) else {
            continue;
        };
        let kind = symbol_kind_label(item.get("kind").and_then(|v| v.as_u64()).unwrap_or(0));
        let container = item
            .get("containerName")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let location = item.get("location");
        let uri = location
            .and_then(|l| l.get("uri"))
            .or_else(|| item.get("uri"))
            .and_then(|u| u.as_str());
        let start = location
            .and_then(|l| l.get("range"))
            .or_else(|| item.get("range"))
            .and_then(|r| r.get("start"));
        let (line, character) = start
            .map(|s| {
                (
                    s.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                    s.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
                )
            })
            .unwrap_or((0, 0));

        let Some(uri) = uri else {
            continue;
        };
        let Some(path) = protocol::uri_to_path(uri) else {
            continue;
        };
        out.push(LspWorkspaceSymbol {
            name: name.to_string(),
            kind: kind.to_string(),
            container,
            file: path.to_string_lossy().into_owned(),
            line,
            character,
        });
    }
    out
}

/// Shared LSP client manager — caches one client per workspace root.
#[derive(Clone)]
pub struct LspManager {
    clients: Arc<RwLock<HashMap<PathBuf, Arc<LspClient>>>>,
    init_locks: Arc<Mutex<HashMap<PathBuf, ()>>>,
}

impl LspManager {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            init_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get (or lazily start + initialize) the client for a workspace.
    pub async fn get_or_init(&self, workspace: &Path) -> AppResult<Arc<LspClient>> {
        // Fast path: client already running.
        {
            let client = self
                .clients
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .get(workspace)
                .cloned();
            if let Some(client) = client {
                if client.is_alive().await {
                    return Ok(client.clone());
                }
            }
        }

        // Slow path under a per-workspace lock to avoid duplicate spawns.
        let mut locks = self.init_locks.lock().await;
        if locks.contains_key(workspace) {
            // Another task is starting this workspace's server — wait and retry.
            drop(locks);
            for _ in 0..50 {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let client = self
                    .clients
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(workspace)
                    .cloned();
                if let Some(client) = client {
                    if client.is_alive().await {
                        return Ok(client.clone());
                    }
                }
            }
            return Err(AppError::Internal(format!(
                "LSP server for {} did not start in time",
                workspace.display()
            )));
        }
        locks.insert(workspace.to_path_buf(), ());
        drop(locks);

        let result = self.spawn_and_init(workspace).await;

        self.init_locks.lock().await.remove(workspace);
        result
    }

    async fn spawn_and_init(&self, workspace: &Path) -> AppResult<Arc<LspClient>> {
        let (binary, kind) = detect_server(workspace).ok_or_else(|| {
            AppError::Internal(format!(
                "No language server found for '{}'. Install one and ensure it is on PATH: \
                 rust-analyzer (rustup component add rust-analyzer), typescript-language-server \
                 (npm i -g typescript-language-server), pyright (npm i -g pyright), gopls \
                 (go install golang.org/x/tools/gopls@latest), or clangd (winget install llvm). \
                 On Windows, npm-installed servers (typescript-language-server / pyright) are \
                 detected via their .cmd shim — restart the app after installing.",
                workspace.display()
            ))
        })?;

        info!(workspace = %workspace.display(), server = %binary, "Starting LSP server");
        let client = LspClient::start(workspace.to_path_buf(), binary, kind).await?;
        let root_uri = protocol::path_to_uri(workspace);
        client.initialize(&root_uri).await?;

        {
            let mut guard = self.clients.write().unwrap_or_else(|e| e.into_inner());
            guard.insert(workspace.to_path_buf(), client.clone());
        }
        Ok(client)
    }

    /// Get the client for a workspace if one is already running.
    pub fn get(&self, workspace: &Path) -> Option<Arc<LspClient>> {
        self.clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(workspace)
            .cloned()
    }

    /// Drop the client for `workspace` — the server child process is
    /// killed on drop (`kill_on_drop`). Called on workspace switch so
    /// stale language servers of projects the user left don't keep
    /// consuming memory/CPU (and their file watches don't fire).
    pub async fn drop_workspace(&self, workspace: &Path) {
        {
            let mut guard = self.clients.write().unwrap_or_else(|e| e.into_inner());
            guard.remove(workspace);
        }
        {
            let mut locks = self.init_locks.lock().await;
            locks.remove(workspace);
        }
    }

    /// Workspace roots with a running client.
    pub fn client_workspaces(&self) -> Vec<PathBuf> {
        self.clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .keys()
            .cloned()
            .collect()
    }
}

impl Default for LspManager {
    fn default() -> Self {
        Self::new()
    }
}

/// LSP tool — queries the language server for code intelligence.
pub struct LspTool {
    manager: LspManager,
}

impl LspTool {
    /// Create a new LSP tool with the given manager.
    pub fn new(manager: LspManager) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for LspTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Code
    }
    fn name(&self) -> &str {
        "lsp"
    }

    fn description(&self) -> &str {
        "Query the Language Server Protocol for code intelligence: \
        go-to-definition, find references, hover documentation, search workspace \
        symbols, or get diagnostics. \
        The format operation returns the formatted text for you to write back with \
        edit_file — it is NOT written to disk by this tool. \
        The server (rust-analyzer / typescript-language-server / pyright / gopls / clangd) \
        is started on first use. Works on the current workspace."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": [
                        "definition", "references", "hover", "workspace_symbols",
                        "format", "diagnostics", "symbols"
                    ],
                    "description": "The LSP operation to perform"
                },
                  "file": {
                      "type": "string",
                      "description": "Path to the file (relative to workspace); required except for workspace_symbols; diagnostics accepts comma-separated paths for aggregation"
                  },
                "line": {
                    "type": "integer",
                    "description": "Line number (0-based)"
                },
                "character": {
                    "type": "integer",
                    "description": "Character offset (0-based)"
                },
                "query": {
                    "type": "string",
                    "description": "Symbol name (or fragment) to search for — used by workspace_symbols"
                }
            },
            "required": ["operation"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> AppResult<ToolResult> {
        let operation = args
            .get("operation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                crate::core::error::AppError::ToolNotFound("missing 'operation'".into())
            })?;

        // `workspace_symbols` searches the whole workspace and needs no file;
        // every other operation resolves a project file first.
        let workspace_symbols = operation == "workspace_symbols";
        let file = args
            .get("file")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let workspace = ctx.workspace.as_deref().ok_or_else(|| {
            AppError::Internal("no workspace set — LSP needs a workspace root".into())
        })?;
        let client = match self.manager.get_or_init(workspace).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::error(e.to_string())),
        };

        let file_path = if workspace_symbols {
            None
        } else {
            if file.is_empty() {
                return Ok(ToolResult::error(
                    "missing 'file' — this operation needs a file path",
                ));
            }
            match resolve_file(workspace, &file) {
                Some(p) => Some(p),
                None => {
                    return Ok(ToolResult::error(format!(
                        "File '{file}' is outside the workspace — LSP only operates on project files"
                    )));
                }
            }
        };

        let line = args.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let character = args.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let pos = Position { line, character };
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        match operation {
            "definition" => {
                let Some(file_path) = file_path.as_deref() else {
                    return Ok(ToolResult::error("missing 'file'"));
                };
                match client.definition(file_path, pos).await {
                    Ok(locations) if locations.is_empty() => {
                        Ok(ToolResult::success("No definition found."))
                    }
                    Ok(locations) => {
                        let lines: Vec<String> = locations
                            .iter()
                            .map(|l| format!("{}:{}:{}", l.file, l.line, l.character))
                            .collect();
                        Ok(ToolResult::success(lines.join("\n")))
                    }
                    Err(e) => Ok(ToolResult::error(format!("Definition lookup failed: {e}"))),
                }
            }
            "references" => {
                let Some(file_path) = file_path.as_deref() else {
                    return Ok(ToolResult::error("missing 'file'"));
                };
                match client.references(file_path, pos).await {
                    Ok(locations) if locations.is_empty() => {
                        Ok(ToolResult::success("No references found."))
                    }
                    Ok(locations) => {
                        let lines: Vec<String> = locations
                            .iter()
                            .map(|l| format!("{}:{}:{}", l.file, l.line, l.character))
                            .collect();
                        Ok(ToolResult::success(lines.join("\n")))
                    }
                    Err(e) => Ok(ToolResult::error(format!("Reference lookup failed: {e}"))),
                }
            }
            "hover" => {
                let Some(file_path) = file_path.as_deref() else {
                    return Ok(ToolResult::error("missing 'file'"));
                };
                let uri = protocol::path_to_uri(file_path);
                if let Err(e) = client
                    .sync_document(&uri, language_id_for_path(file_path))
                    .await
                {
                    return Ok(ToolResult::error(format!("Hover failed: {e}")));
                }
                match client.hover(file_path, pos).await {
                    Ok(value) => match parse_hover(&value) {
                        Some(text) => Ok(ToolResult::success(text)),
                        None => Ok(ToolResult::success(
                            "No hover information at this position.",
                        )),
                    },
                    Err(e) => Ok(ToolResult::error(format!("Hover failed: {e}"))),
                }
            }
            "workspace_symbols" => match client.workspace_symbols(&query).await {
                Ok(value) => {
                    let symbols = parse_workspace_symbols(&value);
                    if symbols.is_empty() {
                        let msg = if query.is_empty() {
                            "No workspace symbols found.".to_string()
                        } else {
                            format!("No symbols match '{query}'.")
                        };
                        Ok(ToolResult::success(msg))
                    } else {
                        let lines: Vec<String> = symbols
                            .iter()
                            .map(|s| {
                                let suffix = s
                                    .container
                                    .as_deref()
                                    .map(|c| format!(" ({c})"))
                                    .unwrap_or_default();
                                format!(
                                    "{}:{}:{} [{}] {}{}",
                                    s.file, s.line, s.character, s.kind, s.name, suffix
                                )
                            })
                            .collect();
                        Ok(ToolResult::success(lines.join("\n")))
                    }
                }
                Err(e) => Ok(ToolResult::error(format!(
                    "Workspace symbol search failed: {e}"
                ))),
            },
            "format" => {
                let Some(file_path) = file_path.as_deref() else {
                    return Ok(ToolResult::error("missing 'file'"));
                };
                match client.format(file_path).await {
                    Ok(Some(formatted)) => Ok(ToolResult::success(formatted)),
                    Ok(None) => Ok(ToolResult::error(
                        "Formatting not available (server lacks documentFormattingProvider).",
                    )),
                    Err(e) => Ok(ToolResult::error(format!("Format failed: {e}"))),
                }
            }
            // Aggregated diagnostics: `file` accepts comma-separated paths
            // (one LSP pull per file, merged into one report).
            "diagnostics" => {
                let mut lines: Vec<String> = Vec::new();
                let mut failed = false;
                for part in file.split(',') {
                    let part = part.trim();
                    if part.is_empty() {
                        continue;
                    }
                    let Some(part_path) = resolve_file(workspace, part) else {
                        lines.push(format!("'{part}' is outside the workspace — skipped"));
                        continue;
                    };
                    match client
                        .diagnostics(&part_path, language_id_for_path(&part_path))
                        .await
                    {
                        Ok(diags) if diags.is_empty() => {
                            lines.push(format!("{part}: no diagnostics"))
                        }
                        Ok(diags) => {
                            for d in diags {
                                lines.push(format!(
                                    "[{}] {}:{}: {}",
                                    d.severity, d.file, d.line, d.message
                                ));
                            }
                        }
                        Err(e) => {
                            failed = true;
                            lines.push(format!("{part}: diagnostics failed ({e})"));
                        }
                    }
                }
                if lines.is_empty() {
                    Ok(ToolResult::success("No diagnostics."))
                } else if failed {
                    Ok(ToolResult::error(lines.join("\n")))
                } else {
                    Ok(ToolResult::success(lines.join("\n")))
                }
            }
            "symbols" => {
                let Some(file_path) = file_path.as_deref() else {
                    return Ok(ToolResult::error("missing 'file'"));
                };
                let uri = protocol::path_to_uri(file_path);
                client
                    .sync_document(&uri, language_id_for_path(file_path))
                    .await?;
                let raw = client.document_symbols(file_path).await?;
                let symbols = parse_document_symbols(&raw);
                if symbols.is_empty() {
                    Ok(ToolResult::success(
                        "No symbols found (or the server lacks documentSymbolProvider).",
                    ))
                } else {
                    let lines: Vec<String> = symbols
                        .iter()
                        .map(|s| format!("{}:{} [{}] {}", s.line, s.character, s.kind, s.name))
                        .collect();
                    Ok(ToolResult::success(lines.join("\n")))
                }
            }
            _ => Ok(ToolResult::error(format!("Unknown operation: {operation}"))),
        }
    }
}

/// Resolve a file path relative to the workspace root. Absolute paths are
/// accepted only when they live INSIDE the workspace — LSP must never open
/// arbitrary locations outside the project it was started for (and a
/// nonexistent absolute path can't be verified, so it is rejected too).
fn resolve_file(workspace: &Path, file: &str) -> Option<PathBuf> {
    let p = Path::new(file);
    if !p.is_absolute() {
        return Some(workspace.join(p));
    }
    let ws_canon = workspace.canonicalize().ok()?;
    let file_canon = p.canonicalize().ok()?;
    if file_canon.starts_with(&ws_canon) {
        Some(file_canon)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsp_location_serializes() {
        let loc = LspLocation {
            file: "src/main.rs".into(),
            line: 10,
            character: 5,
        };
        let json = serde_json::to_string(&loc).unwrap();
        assert!(json.contains("main.rs"));
    }

    #[test]
    fn lsp_diagnostic_serializes() {
        let diag = LspDiagnostic {
            file: "src/lib.rs".into(),
            line: 42,
            severity: "error".into(),
            message: "expected `;`".into(),
            source: "rustc".into(),
        };
        let json = serde_json::to_string(&diag).unwrap();
        assert!(json.contains("error"));
        assert!(json.contains("expected"));
    }

    #[test]
    fn location_from_uri_windows() {
        let range = Range {
            start: Position {
                line: 7,
                character: 3,
            },
            end: Position {
                line: 7,
                character: 8,
            },
        };
        let loc = location_from_uri("file:///C:/proj/src/lib.rs", range);
        assert!(loc.is_some());
        let loc = loc.unwrap();
        assert!(loc.file.ends_with("lib.rs"));
        assert_eq!(loc.line, 7);
        assert_eq!(loc.character, 3);
    }

    #[test]
    fn resolve_file_relative_and_workspace_absolute() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        assert_eq!(
            resolve_file(ws, "src/main.rs"),
            Some(ws.join("src/main.rs"))
        );
        // An absolute path inside the workspace resolves (canonicalized).
        let inner = ws.join("x.rs");
        std::fs::write(&inner, "").unwrap();
        assert_eq!(
            resolve_file(ws, inner.to_str().unwrap()),
            Some(inner.canonicalize().unwrap())
        );
    }

    #[test]
    fn resolve_file_rejects_paths_outside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let other = outside.path().join("x.rs");
        std::fs::write(&other, "").unwrap();
        // A real file outside the workspace is rejected…
        assert_eq!(resolve_file(ws, other.to_str().unwrap()), None);
        // …and so is a nonexistent absolute path (unverifiable).
        assert_eq!(resolve_file(ws, "C:\\does\\not\\exist.rs"), None);
    }

    #[test]
    fn parses_hierarchical_document_symbols() {
        let value = serde_json::json!([
            {
                "name": "main",
                "kind": 12,
                "range": { "start": { "line": 1, "character": 0 }, "end": { "line": 5, "character": 1 } },
                "children": [
                    { "name": "helper", "kind": 12, "range": { "start": { "line": 2, "character": 4 }, "end": { "line": 4, "character": 5 } } }
                ]
            }
        ]);
        let symbols = parse_document_symbols(&value);
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "main");
        assert_eq!(symbols[0].kind, "Function");
        assert_eq!(symbols[1].name, "main.helper");
        assert_eq!(symbols[1].line, 2);
        assert_eq!(symbol_kind_label(6), "Method");
        assert_eq!(symbol_kind_label(999), "Symbol");
    }

    #[test]
    fn parses_flat_symbol_information_shape() {
        let value = serde_json::json!([
            {
                "name": "LoginForm",
                "kind": 5,
                "location": {
                    "uri": "file:///C:/proj/src/ui.rs",
                    "range": { "start": { "line": 9, "character": 4 }, "end": { "line": 9, "character": 20 } }
                }
            }
        ]);
        let symbols = parse_document_symbols(&value);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].kind, "Class");
        assert_eq!(symbols[0].line, 9);
    }

    #[test]
    fn parses_hover_markup_and_marked_string() {
        let markup = serde_json::json!({
            "contents": { "kind": "markdown", "value": "**doc** for `foo`" }
        });
        assert_eq!(parse_hover(&markup).as_deref(), Some("**doc** for `foo`"));

        let marked = serde_json::json!({
            "contents": { "language": "rust", "value": "pub fn foo()" }
        });
        assert_eq!(parse_hover(&marked).as_deref(), Some("pub fn foo()"));

        let plain = serde_json::json!({ "contents": "plain text" });
        assert_eq!(parse_hover(&plain).as_deref(), Some("plain text"));
    }

    #[test]
    fn parses_hover_array_and_missing_contents() {
        let mixed = serde_json::json!({
            "contents": [
                "signature line",
                { "kind": "markdown", "value": "details" },
                { "language": "rust", "value": "" }
            ]
        });
        assert_eq!(
            parse_hover(&mixed).as_deref(),
            Some("signature line\n\ndetails")
        );

        assert_eq!(parse_hover(&serde_json::json!({})), None);
        assert_eq!(parse_hover(&serde_json::json!({ "contents": null })), None);
        assert_eq!(
            parse_hover(&serde_json::json!({ "contents": { "value": "" } })),
            None
        );
    }

    #[test]
    fn parses_workspace_symbols_location_shape() {
        let value = serde_json::json!([
            {
                "name": "run_loop",
                "kind": 12,
                "containerName": "core",
                "location": {
                    "uri": "file:///C:/proj/src/core.rs",
                    "range": {
                        "start": { "line": 40, "character": 2 },
                        "end": { "line": 90, "character": 1 }
                    }
                }
            }
        ]);
        let symbols = parse_workspace_symbols(&value);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "run_loop");
        assert_eq!(symbols[0].kind, "Function");
        assert_eq!(symbols[0].container.as_deref(), Some("core"));
        assert!(symbols[0].file.ends_with("core.rs"));
        assert_eq!(symbols[0].line, 40);
        assert_eq!(symbols[0].character, 2);
    }

    #[test]
    fn parses_workspace_symbols_uri_only_shape() {
        let value = serde_json::json!([
            {
                "name": "render",
                "kind": 12,
                "location": { "uri": "file:///C:/proj/src/ui.rs" }
            },
            { "name": "no_uri", "kind": 5, "location": { "uri": "not-a-uri" } }
        ]);
        let symbols = parse_workspace_symbols(&value);
        assert_eq!(symbols.len(), 1);
        assert!(symbols[0].file.ends_with("ui.rs"));
        assert_eq!(symbols[0].line, 0);
    }
}
