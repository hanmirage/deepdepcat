//! code_search tools — symbol and dependency lookup for the agent.
//!
//! Wires the codebase indexing subsystem (SymbolIndex / DependencyGraph)
//! into the agent toolset. Previously those indexes were only reachable via
//! frontend commands; now the model can answer "where is X defined?" and
//! "what depends on this file?" directly, without grep-scraping whole files.
//!
//! Both tools are read-only and lazily build their index on first use
//! (a one-time cost — the first call may take a second or two).

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::codebase::dependency::DependencyGraph;
use crate::codebase::symbols::SymbolKind;
use crate::core::error::AppResult;
use crate::bootstrap::AppState;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::Path;
use tauri::Manager;

/// Maximum symbols returned by one search_symbols call.
const MAX_SYMBOLS: usize = 30;
/// Maximum files returned by one file_dependencies call.
const MAX_FILES: usize = 20;

/// Whether the cached symbol index must be rebuilt for `workspace`.
///
/// Three staleness conditions: never built, built for a DIFFERENT workspace
/// (serving another project's symbols would be silently wrong), or marked
/// stale after a file write during this agent session (pre-edit answers are
/// wrong answers). An empty-but-current index (a symbol-less workspace) is
/// NOT stale — checking `file_count() == 0` alone would rebuild forever.
fn symbol_index_stale(state: &AppState, workspace: &Path) -> bool {
    let index = state.symbol_index.read().unwrap_or_else(|e| e.into_inner());
    index.indexed_root.as_deref() != Some(workspace) || index.stale
}

/// Lazily ensure the symbol index is populated and FRESH for the workspace.
///
/// The directory walk is CPU/disk heavy, so it runs on a blocking thread
/// (never stalling the async executor) and only the final swap takes the
/// std RwLock.
async fn ensure_symbol_index(state: &AppState, workspace: &Path) {
    let stale = symbol_index_stale(state, workspace);
    if !stale {
        return;
    }
    tracing::info!("Building symbol index on demand");
    let workspace = workspace.to_path_buf();
    let index_workspace = workspace.clone();
    let built = tokio::task::spawn_blocking(move || {
        let mut fresh = crate::codebase::symbols::SymbolIndex::new();
        fresh.index_directory(&index_workspace);
        fresh
    })
    .await;
    let fresh = match built {
        Ok(fresh) => fresh,
        Err(e) => {
            tracing::warn!(error = %e, "Symbol index build task panicked");
            return;
        }
    };
    let mut index = state
        .symbol_index
        .write()
        .unwrap_or_else(|e| e.into_inner());
    if index.indexed_root.as_deref() != Some(workspace.as_path()) || index.stale {
        *index = fresh;
    }
}

/// Lazily ensure the dependency graph is available for the workspace.
///
/// Rebuilt when it was built for a different workspace root or marked
/// stale after a file write / external change — the cached graph must
/// never answer import questions about another project or pre-edit
/// imports.
async fn ensure_dependency_graph(state: &AppState, workspace: &Path) {
    let stale = {
        let graph = state
            .dependency_graph
            .read()
            .unwrap_or_else(|e| e.into_inner());
        match graph.as_ref() {
            Some(g) => g.root() != workspace || g.is_stale(),
            None => true,
        }
    };
    if stale {
        tracing::info!("Building dependency graph on demand");
        let workspace = workspace.to_path_buf();
        let graph_workspace = workspace.clone();
        let built = tokio::task::spawn_blocking(move || {
            let mut graph = DependencyGraph::new(&graph_workspace);
            graph.build();
            graph
        })
        .await;
        let graph = match built {
            Ok(graph) => graph,
            Err(e) => {
                tracing::warn!(error = %e, "Dependency graph build task panicked");
                return;
            }
        };
        let mut cache = state
            .dependency_graph
            .write()
            .unwrap_or_else(|e| e.into_inner());
        let still_stale = cache
            .as_ref()
            .map(|g| g.root() != workspace.as_path() || g.is_stale())
            .unwrap_or(true);
        if still_stale {
            *cache = Some(graph);
        }
    }
}

/// Render a path relative to the workspace to keep tool output compact.
fn relative_to<'a>(workspace: &'a Path, path: &'a Path) -> &'a Path {
    path.strip_prefix(workspace).unwrap_or(path)
}

/// search_symbols — find symbol definitions by name.
///
/// Matching order: exact name → name prefix → name substring (case-insensitive).
/// Results include the file path and line so the agent can read the file.
pub struct SearchSymbolsTool;

impl SearchSymbolsTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for SearchSymbolsTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Code
    }
    fn name(&self) -> &str {
        "search_symbols"
    }

    fn description(&self) -> &str {
        "Find symbol definitions (functions, structs, classes, interfaces, \
         traits, enums, consts) in the codebase by name. Faster and more \
         precise than grep for 'where is X defined'. Returns matching \
         symbols with file path and line. Use when you need to locate a \
         definition or check what symbols exist in the project. The index \
         is built on first use (may take a moment)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Symbol name to search (case-insensitive, partial matches allowed)."
                },
                "kind": {
                    "type": "string",
                    "enum": ["function", "struct", "enum", "interface", "class", "trait", "module", "const", "method"],
                    "description": "Optional — restrict results to one symbol kind."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum results to return (default 20, max 30)."
                }
            },
            "required": ["query"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn check_permissions(&self, _args: &Value, _context: &ToolContext) -> PermissionDecision {
        PermissionDecision::Allow
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> AppResult<ToolResult> {
        let query = args
            .get("query")
            .and_then(|q| q.as_str())
            .map(str::trim)
            .filter(|q| !q.is_empty())
            .ok_or_else(|| crate::core::error::AppError::Parse("Missing 'query'".into()))?;
        let max_results = args
            .get("max_results")
            .and_then(|m| m.as_u64())
            .unwrap_or(20)
            .min(MAX_SYMBOLS as u64) as usize;
        let kind_filter = args
            .get("kind")
            .and_then(|k| k.as_str())
            .and_then(parse_symbol_kind);

        let workspace = ctx.workspace.as_deref().ok_or_else(|| {
            crate::core::error::AppError::Parse(
                "No workspace set — codebase search unavailable".into(),
            )
        })?;
        if !workspace.exists() {
            return Ok(ToolResult::error(format!(
                "Workspace '{}' does not exist",
                workspace.display()
            )));
        }

        let state = ctx.app.state::<AppState>();
        ensure_symbol_index(&state, workspace).await;

        let index = state.symbol_index.read().unwrap_or_else(|e| e.into_inner());
        let query_lower = query.to_lowercase();

        // Priority: exact name → prefix → substring (bounded scan).
        let exact = index
            .find_by_name(query)
            .into_iter()
            .filter(|s| kind_matches(s.kind, kind_filter))
            .collect::<Vec<_>>();
        let prefix = if exact.len() < max_results {
            index
                .find_by_prefix(query)
                .into_iter()
                .filter(|s| s.name.to_lowercase() != query_lower)
                .filter(|s| kind_matches(s.kind, kind_filter))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let substring = if exact.len() + prefix.len() < max_results {
            index
                .all()
                .iter()
                .filter(|s| {
                    s.name.to_lowercase().contains(&query_lower)
                        && !s.name.to_lowercase().starts_with(&query_lower)
                })
                .filter(|s| kind_matches(s.kind, kind_filter))
                .take(max_results - exact.len() - prefix.len())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };

        let mut results: Vec<&crate::codebase::symbols::Symbol> = Vec::new();
        results.extend(exact);
        results.extend(prefix);
        results.extend(substring);
        results.truncate(max_results);

        if results.is_empty() {
            return Ok(ToolResult::success(format!(
                "No symbols matching '{query}' found in the index."
            )));
        }

        let lines: Vec<String> = results
            .iter()
            .map(|s| {
                let vis = if s.is_public { "pub" } else { "priv" };
                format!(
                    "{} ({} {}) — {}:{} — {}",
                    s.name,
                    s.kind.as_str(),
                    vis,
                    relative_to(workspace, &s.file_path).display(),
                    s.line,
                    s.signature
                )
            })
            .collect();
        Ok(ToolResult::success(format!(
            "{} matching symbol(s) for '{}':\n{}",
            lines.len(),
            query,
            lines.join("\n")
        )))
    }
}

/// file_dependencies — files a file imports, and files that import it.
pub struct FileDependenciesTool;

impl FileDependenciesTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for FileDependenciesTool {
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::Code
    }
    fn name(&self) -> &str {
        "file_dependencies"
    }

    fn description(&self) -> &str {
        "Show the import dependency relationships of a file: what it \
         imports (dependencies) and what imports it (dependents). Use to \
         understand change impact before editing — 'who else is affected \
         by this file?' The graph is built on first use (may take a moment)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "file": {
                    "type": "string",
                    "description": "File path (absolute or relative to workspace)."
                },
                "direction": {
                    "type": "string",
                    "enum": ["both", "imports", "imported_by"],
                    "description": "Which direction to show (default both)."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum files per direction (default 10, max 20)."
                }
            },
            "required": ["file"]
        })
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn check_permissions(&self, _args: &Value, _context: &ToolContext) -> PermissionDecision {
        PermissionDecision::Allow
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> AppResult<ToolResult> {
        let file = args
            .get("file")
            .and_then(|f| f.as_str())
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .ok_or_else(|| crate::core::error::AppError::Parse("Missing 'file'".into()))?;
        let direction = args
            .get("direction")
            .and_then(|d| d.as_str())
            .unwrap_or("both");
        let max_results = args
            .get("max_results")
            .and_then(|m| m.as_u64())
            .unwrap_or(10)
            .min(MAX_FILES as u64) as usize;

        let workspace = ctx.workspace.as_deref().ok_or_else(|| {
            crate::core::error::AppError::Parse(
                "No workspace set — codebase search unavailable".into(),
            )
        })?;
        let file_path = crate::tools::builtin::resolve_path(Some(workspace), file);

        let state = ctx.app.state::<AppState>();
        ensure_dependency_graph(&state, workspace).await;

        let graph = state
            .dependency_graph
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let graph = graph.as_ref().ok_or_else(|| {
            crate::core::error::AppError::Internal("Dependency graph unavailable".into())
        })?;

        let mut sections: Vec<String> = Vec::new();

        if direction == "both" || direction == "imports" {
            let deps: Vec<_> = graph
                .dependencies_of(&file_path)
                .into_iter()
                .take(max_results)
                .map(|node| relative_to(workspace, &node.path).display().to_string())
                .collect();
            if deps.is_empty() {
                sections.push("imports: (none)".to_string());
            } else {
                sections.push(format!("imports ({}):\n{}", deps.len(), deps.join("\n")));
            }
        }

        if direction == "both" || direction == "imported_by" {
            let dependents: Vec<_> = graph
                .dependents_of(&file_path)
                .into_iter()
                .take(max_results)
                .map(|node| relative_to(workspace, &node.path).display().to_string())
                .collect();
            if dependents.is_empty() {
                sections.push("imported_by: (none)".to_string());
            } else {
                sections.push(format!(
                    "imported_by ({}):\n{}",
                    dependents.len(),
                    dependents.join("\n")
                ));
            }
        }

        Ok(ToolResult::success(format!(
            "Dependencies for {}:\n{}",
            relative_to(workspace, &file_path).display(),
            sections.join("\n\n")
        )))
    }
}

fn parse_symbol_kind(s: &str) -> Option<SymbolKind> {
    match s {
        "function" => Some(SymbolKind::Function),
        "struct" => Some(SymbolKind::Struct),
        "enum" => Some(SymbolKind::Enum),
        "interface" => Some(SymbolKind::Interface),
        "class" => Some(SymbolKind::Class),
        "trait" => Some(SymbolKind::Trait),
        "module" => Some(SymbolKind::Module),
        "const" => Some(SymbolKind::Const),
        "method" => Some(SymbolKind::Method),
        _ => None,
    }
}

fn kind_matches(kind: SymbolKind, filter: Option<SymbolKind>) -> bool {
    match filter {
        Some(f) => kind == f,
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_symbols_tool_contract() {
        let tool = SearchSymbolsTool::new();
        assert_eq!(tool.name(), "search_symbols");
        // Read-only tools are auto-allowed by the dispatcher (no permission
        // dialog), safe to run in parallel with other read tools.
        assert!(tool.is_read_only());
        assert!(tool.is_concurrency_safe());
        let params = tool.parameters();
        let props = params["properties"].as_object().unwrap();
        assert!(props.contains_key("query"));
        assert!(props.contains_key("kind"));
        assert!(props.contains_key("max_results"));
        let required = params["required"].as_array().unwrap();
        assert_eq!(required[0], "query");
    }

    #[test]
    fn file_dependencies_tool_contract() {
        let tool = FileDependenciesTool::new();
        assert_eq!(tool.name(), "file_dependencies");
        assert!(tool.is_read_only());
        assert!(tool.is_concurrency_safe());
        let params = tool.parameters();
        let props = params["properties"].as_object().unwrap();
        assert!(props.contains_key("file"));
        assert!(props.contains_key("direction"));
        let direction = &props["direction"]["enum"];
        assert!(direction.as_array().unwrap().contains(&json!("imports")));
        assert!(direction
            .as_array()
            .unwrap()
            .contains(&json!("imported_by")));
    }

    #[test]
    fn kind_filter_parsing() {
        assert_eq!(parse_symbol_kind("function"), Some(SymbolKind::Function));
        assert_eq!(parse_symbol_kind("struct"), Some(SymbolKind::Struct));
        assert_eq!(parse_symbol_kind("bogus"), None);
        assert!(kind_matches(SymbolKind::Function, None));
        assert!(kind_matches(
            SymbolKind::Function,
            Some(SymbolKind::Function)
        ));
        assert!(!kind_matches(
            SymbolKind::Struct,
            Some(SymbolKind::Function)
        ));
    }

    #[test]
    fn relative_to_strips_workspace() {
        let ws = std::path::Path::new("C:\\proj");
        let f = std::path::Path::new("C:\\proj\\src\\main.rs");
        assert_eq!(relative_to(ws, f), std::path::Path::new("src\\main.rs"));
        let outside = std::path::Path::new("C:\\other\\x.rs");
        assert_eq!(relative_to(ws, outside), outside);
    }
}
