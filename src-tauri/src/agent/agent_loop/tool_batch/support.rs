//! Shared tool-batch helpers — results, guards, monitoring.

use tauri::{AppHandle, Manager};

pub(crate) fn record_monitor_event(
    app: &AppHandle,
    session_id: &str,
    event_type: &str,
    payload: serde_json::Value,
) {
    use crate::tools::builtin::monitor::MonitoredEvent;
    let state = app.state::<crate::bootstrap::AppState>();
    state.monitor_events.push(
        session_id,
        MonitoredEvent {
            event_type: event_type.to_string(),
            payload,
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        },
    );
}

/// Classify an error into a coarse diagnostic kind for the anonymous
/// reporter. Only the kind is uploaded (never the message), so this is safe
/// to share with the telemetry backend.
pub(crate) fn classify_tool_error(e: &crate::core::error::AppError) -> &'static str {
    use crate::core::error::AppError;
    match e {
        AppError::PermissionDenied { .. } => "permission_denied",
        AppError::ToolExecution { .. } | AppError::ToolNotFound(_) => "tool_error",
        AppError::Timeout(_) => "timeout",
        AppError::NetworkError(_) | AppError::Http(_) | AppError::LlmApi { .. } => "network",
        AppError::Parse(_) | AppError::Serialization(_) => "parse",
        AppError::Io(_) | AppError::Path(_) => "io",
        AppError::Sandbox(_) => "sandbox",
        AppError::Cancelled => "cancelled",
        AppError::Mcp(_) => "mcp",
        AppError::Hook(_) => "hook",
        _ => "other",
    }
}

/// Record one tool error to the anonymous diagnostics reporter (if enabled).
/// Best-effort — never breaks tool execution.
pub(crate) fn record_tool_diagnostic(app: &AppHandle, tool_name: &str, e: &crate::core::error::AppError) {
    record_tool_diagnostic_kind(app, tool_name, classify_tool_error(e));
}

/// Record a tool failure with an explicit kind string (for content-level
/// failures that never produced an `AppError`).
pub(crate) fn record_tool_diagnostic_kind(app: &AppHandle, tool_name: &str, kind: &str) {
    let state = app.state::<crate::bootstrap::AppState>();
    state.diagnostics.record(tool_name, kind);
}

/// Record a tool execution into the session tracker + durable global
/// aggregate (best-effort — accounting must never break tool execution).
/// Result tokens are estimated as chars/4.
pub(crate) async fn record_tool_usage(
    app: &AppHandle,
    session_id: &str,
    turn: u32,
    tool_name: &str,
    content: &str,
) {
    let state = app.state::<crate::bootstrap::AppState>();
    state.usage_tracker(session_id).await.record_tool_call(
        turn,
        tool_name,
        (content.len() as u64 / 4).max(1),
    );
}

/// Wrap a hook-injected context payload so the model recognizes it as
/// system-provided guidance (not a user message) — same `<system-reminder>`
/// shape the harness uses for other transient injections.
pub(crate) fn hook_context_wrapper(event: &str, tool: &str, context: &str) -> String {
    format!(
        "<system-reminder>\n[hook {event} · {tool}]\n{context}\n</system-reminder>"
    )
}

/// The outcome of executing a single tool call in the batch.
pub(crate) struct BatchToolResult {
    pub(crate) call_id: String,
    pub(crate) name: String,
    pub(crate) content: String,
    pub(crate) is_error: bool,
    pub(crate) permission_denied: bool,
    /// Optional image attached to a successful read_file of a picture —
    /// injected into the conversation as a transient user message for
    /// vision-capable main models.
    pub(crate) image: Option<crate::toolkit::ToolImage>,
    /// Optional MCP Apps UI payload (raw JSON from the tool's metadata) —
    /// emitted as `StreamEvent::McpApp` so the frontend can render the
    /// interactive HTML.
    pub(crate) app: Option<serde_json::Value>,
    /// Parsed arguments (for PostToolUse hook and skill tracking).
    pub(crate) args: serde_json::Value,
    /// Raw arguments JSON string — used to recompute the repeat-failure guard
    /// key in the sequential results loop (the concurrent executor itself
    /// never touches `chat_state`).
    pub(crate) arguments: String,
    /// Whether execution was skipped due to a PreToolUse hook deny.
    pub(crate) hook_blocked: bool,
    /// `additionalContext` payloads injected by PostToolUse-family hooks —
    /// pushed into the conversation by the caller (the concurrent executor
    /// itself never touches `chat_state`).
    pub(crate) hook_contexts: Vec<String>,
}

/// Whether the tool writes file content (as opposed to reading or listing).
pub(crate) fn is_write_tool(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "write_file" | "edit_file" | "search_replace" | "apply_patch"
    )
}

/// Record a successful file edit on the session state so the Evaluator-QA
/// gate (run.rs) and subagent spawns (spawn.rs) can target exactly the files
/// the generator touched instead of falling back to a workspace-wide review.
/// Only SUCCESSFUL writes count — a failed edit must not be presented as a
/// change to review.
pub(crate) fn record_edited_path(
    chat_state: &mut crate::agent::chat_state::ChatState,
    tool_name: &str,
    args: &serde_json::Value,
) {
    if !is_write_tool(tool_name) {
        return;
    }
    if let Some(path) = args.get("path").and_then(|p| p.as_str()) {
        chat_state.record_edited_path(path);
    }
}

/// Mark the cached indexes stale after a successful file write — the
/// next `search_symbols` / `file_dependencies` lookup rebuilds instead of
/// answering from pre-edit content. Both indexes get the same contract
/// (`SymbolIndex::mark_stale` + `DependencyGraph::mark_stale`), so one
/// write event invalidates both caches. Best-effort: accounting
/// bookkeeping must never break tool execution.
pub(crate) fn mark_indexes_stale(app: &AppHandle) {
    use crate::bootstrap::AppState;
    let state = app.state::<AppState>();
    let mut index = state
        .symbol_index
        .write()
        .unwrap_or_else(|e| e.into_inner());
    index.mark_stale();
    let mut graph = state
        .dependency_graph
        .write()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(graph) = graph.as_mut() {
        graph.mark_stale();
    }
}

/// Build a stable identity for the repeat-failure guard: tool name + a
/// normalized hash of the arguments. Valid JSON arguments are re-serialized
/// CANONICALLY, so cosmetically different-but-equivalent calls match
/// (`"path": "a.rs"` vs `"path":"a.rs"`, key order, whitespace) while word
/// boundaries survive ("x y" and "xy" stay distinct — collapsing all
/// whitespace would let unrelated calls falsely block each other).
/// Non-JSON arguments (raw bash commands etc.) fall back to collapsing
/// whitespace runs. The normalized text is hashed so long arguments can
/// never collide on a truncated prefix.
pub(crate) fn failure_guard_key(name: &str, arguments: &str) -> String {
    use std::hash::{Hash, Hasher};
    let normalized: String = match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(value) => value.to_string(),
        Err(_) => arguments.split_whitespace().collect::<Vec<_>>().join(" "),
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    normalized.hash(&mut hasher);
    format!("{name}|{:016x}", hasher.finish())
}

/// Update the repeat-failure guard after a tool call resolves.
///
/// Failures accumulate per (tool, normalized-args) signature; a success
/// clears the signature's count so a fixed call is never blocked. Shared by
/// the serial and parallel execution paths so both get identical guard
/// behavior.
///
/// The map is capped: a long session with many distinct failed signatures
/// (e.g. read_file probing many nonexistent paths) must not grow without
/// bound. When a new key would push the map over the cap, the lowest-count
/// entries are dropped — the guard's job is to stop IDENTICAL retries, and
/// a signature with count 1 that predates many newer ones is the least
/// likely to be retried identically.
pub(crate) fn record_failure_outcome(
    counts: &mut std::collections::HashMap<String, u32>,
    name: &str,
    arguments: &str,
    is_error: bool,
) {
    let key = failure_guard_key(name, arguments);
    if is_error {
        let is_new = !counts.contains_key(&key);
        *counts.entry(key.clone()).or_insert(0) += 1;
        // Bound the map: a fresh failure that pushes it over the cap evicts
        // the single lowest-count OTHER signature (never the just-inserted
        // key — it is the newest and most likely to be retried). Ties break
        // arbitrarily.
        if is_new && counts.len() > MAX_FAILURE_GUARD_KEYS {
            let mut evict: Option<String> = None;
            let mut evict_count = u32::MAX;
            for (k, v) in counts.iter() {
                if *k == key {
                    continue;
                }
                if *v < evict_count {
                    evict_count = *v;
                    evict = Some(k.clone());
                }
            }
            if let Some(k) = evict {
                counts.remove(&k);
            }
        }
    } else {
        counts.remove(&key);
    }
}

/// Track consecutive failures PER TOOL NAME (any arguments) — the
/// strategy-switch signal for the #84 skeleton hardening. `bash` failing
/// with `mvn`, then `javac`, then `java` is three different signatures but
/// ONE doomed approach; the name-level counter catches that. A success on
/// the tool clears its count.
pub(crate) fn record_tool_name_outcome(
    counts: &mut std::collections::HashMap<String, u32>,
    name: &str,
    is_error: bool,
) {
    if is_error {
        *counts.entry(name.to_string()).or_insert(0) += 1;
    } else {
        counts.remove(name);
    }
}

/// Maximum distinct (tool, args) failure signatures tracked per session.
pub(crate) const MAX_FAILURE_GUARD_KEYS: usize = 256;

