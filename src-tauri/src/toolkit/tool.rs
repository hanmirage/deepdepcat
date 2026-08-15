//! Tool types — the unified type system for all agent tools.
//!
//! This module is the single home for:
//! - The `Tool` trait (async, object-safe via `async_trait`)
//! - `ToolContext` / `ToolResult` / `PermissionDecision`
//! - Streaming primitives: `ToolStream<T>`, `ToolStreamItem<T>`,
//!   `terminal_only()`, `with_progress()`
//! - The 4-variant `ToolProgress` enum (Text / Content / Custom / PartialResult)
//!
//! All built-in tools import from here. The old `tools/trait_def.rs` is a
//! thin re-export shim that will be deleted once migration is complete.

use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures_util::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;

use crate::core::error::AppResult;
use crate::core::types::{ConversationItem, ToolDefinition};

// ── Streaming Types ──────────────────────────────────────────────────────────

/// Progress update emitted during streaming tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolProgress {
    /// Tool-defined progress payload. `subkind` is a stable discriminator.
    Custom { subkind: String, payload: Value },
    /// Incremental output delta — append-only, lossless.
    PartialResult { delta: String, total_bytes: u64 },
}

/// Stream of items a tool produces during a single call.
/// Shape: `[Progress(_)*, Terminal(Result<T, String>)]`.
pub type ToolStream<T> = Pin<Box<dyn Stream<Item = ToolStreamItem<T>> + Send>>;

/// One item in a `ToolStream`.
#[derive(Debug)]
pub enum ToolStreamItem<T> {
    /// Terminal result. Exactly one per stream, always last.
    Terminal(Result<T, String>),
}

/// Build a single-item stream containing only the terminal result.
pub fn terminal_only<T: Send + 'static>(result: Result<T, String>) -> ToolStream<T> {
    Box::pin(stream::iter(std::iter::once(ToolStreamItem::Terminal(
        result,
    ))))
}

// ── Context & Result ─────────────────────────────────────────────────────────

/// The context passed to each tool execution.
#[derive(Clone)]
pub struct ToolContext {
    pub session_id: String,
    /// Stream turn ID — emitted on `StreamEvent` to group by turn.
    pub turn_id: String,
    /// The tool call ID that triggered this execution — lets tools correlate
    /// their async side effects (e.g. subagent spawns) back to the call.
    pub call_id: String,
    /// Product work mode this execution runs in ("code"/"depwork") —
    /// meta-tools (use_tool, agent) filter their targets by this.
    pub work_mode: crate::toolkit::WorkMode,
    /// The main model id for this session — lets tools decide between
    /// native image embedding (vision-capable) and text transcription
    /// (text-only models like DeepSeek).
    pub model: String,
    /// The session's provider hint (e.g. "deepseek", "provider-<ts>") — lets
    /// model-facing meta tools (`agent` decompose / subagent spawn) route
    /// their own LLM calls to the SAME provider as the session. Without it,
    /// a custom-provider model that does not match a known prefix falls back
    /// to the first enabled provider and gets HTTP 400 (the #102 model-
    /// routing bug class).
    pub provider: Option<String>,
    /// The session's usage tracker — lets tools that make their own LLM
    /// calls (`visual_describe`, read_file image transcription) record the
    /// billed tokens into the session stats instead of vanishing from every
    /// accounting surface. `None` for subagent dispatchers (their usage
    /// surfaces through `SubagentResult`).
    pub usage_tracker: Option<crate::observability::usage::SessionUsageTracker>,
    pub workspace: Option<std::path::PathBuf>,
    pub app: AppHandle,
    /// File state tracker for checkpoint/rewind functionality.
    pub file_state_tracker: Option<crate::workspace::checkpoint::FileStateTracker>,
    /// Behavior version pinned for this execution (config-level override).
    pub behavior_version: ToolBehaviorVersion,
    /// Snapshot of the conversation at the moment the tool executes.
    /// Used by context-hungry tools (agent fork mode) — the subagent inherits
    /// the parent's exploration background instead of starting blank.
    pub conversation: Vec<ConversationItem>,
    /// The session that spawned this execution as a SUBAGENT — `None` for a
    /// main agent session. Lets tools share parent-owned caches (e.g. the
    /// vision transcription cache: a subagent asking the same question about
    /// the same image reuses the parent's description instead of re-invoking
    /// the vision API).
    pub parent_session_id: Option<String>,
    /// Subagent nesting depth of the agent executing this call (0 = main
    /// loop). The `agent` tool spawns children at depth+1 and the
    /// coordinator's recursion guard compares against `max_depth` — without
    /// real depth threading the guard could never trigger.
    pub agent_depth: u32,
    /// Deny rules inherited from the parent agent chain (raw `Tool(pattern)`
    /// strings). The `agent` tool forwards these into nested subagent
    /// configs so a parent's hard deny can never be dropped by a child —
    /// deny propagation is an OR over the whole ancestor chain.
    pub agent_deny_rules: Vec<String>,
    /// `(name, path)` notes for images attached to the CURRENT user message
    /// by a TEXT-ONLY main model. Empty for multimodal parents (their
    /// pictures travel as image parts). Non-fork subagents get these injected
    /// into their task context so they can `visual_describe` by path.
    pub attached_images: Vec<(String, String)>,
}

/// The result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, Value>>,
    /// An optional image embedded in this tool result (read_file multimodal
    /// for vision-capable main models). `data` is base64 — the request layers
    /// never re-encode it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<ToolImage>,
}

/// An image attached to a tool result (read_file reading a picture).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolImage {
    pub media_type: String,
    pub data: String,
}

impl ToolResult {
    pub fn success(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            metadata: None,
            image: None,
        }
    }
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            metadata: None,
            image: None,
        }
    }
    pub fn with_metadata(mut self, key: impl Into<String>, value: Value) -> Self {
        self.metadata
            .get_or_insert_with(HashMap::new)
            .insert(key.into(), value);
        self
    }
    /// Attach an image to this result. The content text remains the
    /// model-visible summary; the image travels separately.
    pub fn with_image(mut self, image: ToolImage) -> Self {
        self.image = Some(image);
        self
    }

    /// The MCP Apps UI payload attached under `mcp_app`, if any (MCP server
    /// tool results declare an interactive HTML app — surfaced to the
    /// frontend, never to the model's text context).
    pub fn mcp_app(&self) -> Option<Value> {
        self.metadata
            .as_ref()
            .and_then(|m| m.get("mcp_app"))
            .cloned()
    }
}

impl From<String> for ToolResult {
    fn from(s: String) -> Self {
        Self::success(s)
    }
}
impl From<&str> for ToolResult {
    fn from(s: &str) -> Self {
        Self::success(s.to_string())
    }
}

// ── Permission ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionDecision {
    Allow,
    Deny(String),
    Ask,
}

// ── Tool Behavior Versioning ────────────────────────────────────────────────

/// Behavior version of a tool.
///
/// Tools read the version from `ToolContext` to decide between current and
/// legacy behavior. Users can pin tools to a legacy version to preserve
/// output formats across upgrades — the tool's description/parameters stay
/// the same, only its runtime behavior branches on the version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolBehaviorVersion {
    /// Current behavior (default).
    #[default]
    Current,
    /// Legacy 0.1.0 behavior — matches the first shipped tool semantics.
    Legacy0_1_0,
}

impl ToolBehaviorVersion {
    /// Parse from a config string, falling back to `Current`.
    pub fn parse(s: &str) -> Self {
        match s {
            "legacy-0.1.0" => Self::Legacy0_1_0,
            _ => Self::Current,
        }
    }
}

// ── Tool Trait ───────────────────────────────────────────────────────────────

/// The trait that every tool implements.
///
/// Tools can override `execute_stream` to emit `ToolProgress` events during
/// long-running operations. The default implementation wraps `execute()` in
/// a single-item stream (no progress), so tools that don't need streaming
/// just implement `execute()`.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;

    fn is_read_only(&self) -> bool {
        false
    }
    /// Per-call read classification — the value the permission pipeline
    /// feeds `read_only` for THIS invocation. Defaults to `is_read_only()`;
    /// tools with mixed read/write actions (browser_control, office_automate)
    /// override it to approve their read actions without prompting.
    fn is_read_only_call(&self, _args: &Value) -> bool {
        self.is_read_only()
    }
    /// Self-approval hook: after the unified permission pipeline answers
    /// `Ask` for a write call, the tool gets one last chance to prove THIS
    /// invocation is safe without a prompt (e.g. Depwork generate tools
    /// writing to a NEW path, or editing the session's own output). Deny
    /// rules from the pipeline already ran and still win — this hook can
    /// only convert Ask → Allow, never override a Deny.
    fn self_approve(&self, _args: &Value, _context: &ToolContext) -> PermissionDecision {
        PermissionDecision::Ask
    }
    fn is_concurrency_safe(&self) -> bool {
        true
    }
    fn is_enabled(&self) -> bool {
        true
    }
    /// The work mode scope this tool is available in (default: all modes).
    ///
    /// Code-only tools (bash, code editing, LSP, code intelligence) override
    /// this to `ToolScope::Code`; Depwork filters them out at agent build
    /// time via `ToolRegistry::for_mode`.
    fn scope(&self) -> crate::toolkit::ToolScope {
        crate::toolkit::ToolScope::All
    }

    fn check_permissions(&self, _args: &Value, _context: &ToolContext) -> PermissionDecision {
        if self.is_read_only() {
            PermissionDecision::Allow
        } else {
            PermissionDecision::Ask
        }
    }

    /// Execute the tool and return a single result.
    ///
    /// Tools that don't need streaming progress should implement this.
    /// Tools that DO need streaming should override `execute_stream` instead.
    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult>;

    /// Validate tool arguments against the tool's own `parameters()` schema
    /// BEFORE execution.
    ///
    /// The default implementation checks required fields and top-level
    /// value types (string/integer/boolean/array/object). Tools with
    /// richer constraints (enums, nested schemas, cross-field rules) can
    /// override this. Called by the dispatcher right after JSON parsing —
    /// a failed validation returns a typed error instead of executing the
    /// tool with garbage input.
    fn validate_args(&self, args: &Value) -> Result<(), String> {
        validate_against_schema(&self.parameters(), args)
    }

    /// Execute the tool with streaming progress.
    ///
    /// Returns a `ToolStream` that emits zero or more `Progress` items
    /// followed by exactly one `Terminal` item. The default implementation
    /// wraps `execute()` in a single-item stream with no progress events.
    async fn execute_stream(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> AppResult<ToolStream<ToolResult>> {
        let result = self.execute(args, context).await;
        Ok(terminal_only(result.map_err(|e| e.to_string())))
    }

    /// Whether a failed execution may be retried automatically.
    ///
    /// Only consulted for READ-ONLY tools (the dispatcher never auto-retries
    /// side-effecting tools). Network-dependent read tools (web_fetch,
    /// web_search, ...) override this to tolerate transient errors — one
    /// retry with a short backoff, then the error is reported as-is.
    fn is_retryable(&self, _error: &crate::core::error::AppError) -> bool {
        false
    }
}

// ── Tool Spec & Definition ───────────────────────────────────────────────────

/// Validate a value against a JSON-schema-like parameter definition.
///
/// Lightweight, schema-light validation used by the default
/// `Tool::validate_args`:
/// - `required` fields must be present (and non-null).
/// - Declared `type` of each provided property is checked at the top level
///   (`string`, `integer`, `number`, `boolean`, `array`, `object`).
/// - `enum` constraints are honored when present.
///
/// Nested validation is intentionally out of scope — tools with complex
/// nested schemas override `validate_args`.
pub fn validate_against_schema(schema: &Value, args: &Value) -> Result<(), String> {
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return Ok(()); // no declared properties — nothing to validate
    };

    // Required fields.
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        for name in required {
            let name = name.as_str().unwrap_or_default();
            match args.get(name) {
                Some(v) if !v.is_null() => {}
                _ => return Err(format!("missing required argument '{name}'")),
            }
        }
    }

    // Type checks for provided properties.
    for (name, prop) in props {
        let Some(value) = args.get(name) else {
            continue;
        };
        if value.is_null() {
            continue;
        }
        if let Some(expected) = prop.get("type").and_then(|t| t.as_str()) {
            let ok = match expected {
                "string" => value.is_string(),
                "integer" => value.is_i64() || value.is_u64(),
                "number" => value.is_number(),
                "boolean" => value.is_boolean(),
                "array" => value.is_array(),
                "object" => value.is_object(),
                _ => true,
            };
            if !ok {
                return Err(format!(
                    "argument '{name}' must be {expected}, got {}",
                    json_type_name(value)
                ));
            }
        }
        if let Some(variants) = prop.get("enum").and_then(|e| e.as_array()) {
            let matched = variants.iter().any(|v| v == value);
            if !matched {
                return Err(format!(
                    "argument '{name}' is not one of the allowed values"
                ));
            }
        }
    }

    Ok(())
}

/// Human-readable JSON value type name for error messages.
fn json_type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub fn tool_to_definition(tool: &dyn Tool) -> ToolDefinition {
    ToolDefinition::function(tool.name(), Some(tool.description()), tool.parameters())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "max_results": { "type": "integer" },
                "follow": { "type": "boolean" },
                "tags": { "type": "array" },
                "mode": { "type": "string", "enum": ["fast", "safe"] }
            },
            "required": ["path"]
        })
    }

    #[test]
    fn valid_args_pass() {
        assert!(validate_against_schema(&schema(), &json!({"path": "/a/b"})).is_ok());
        assert!(validate_against_schema(
            &schema(),
            &json!({"path": "/a/b", "max_results": 3, "follow": true, "tags": ["x"], "mode": "fast"})
        )
        .is_ok());
    }

    #[test]
    fn missing_required_rejected() {
        let err = validate_against_schema(&schema(), &json!({"max_results": 3})).unwrap_err();
        assert!(err.contains("path"));
        assert!(err.contains("required"));
    }

    #[test]
    fn wrong_type_rejected() {
        let err = validate_against_schema(&schema(), &json!({"path": 42})).unwrap_err();
        assert!(err.contains("path"));
        assert!(err.contains("string"));
        let err2 =
            validate_against_schema(&schema(), &json!({"path": "/x", "max_results": "many"}))
                .unwrap_err();
        assert!(err2.contains("max_results"));
    }

    #[test]
    fn enum_violation_rejected() {
        let err = validate_against_schema(&schema(), &json!({"path": "/x", "mode": "turbo"}))
            .unwrap_err();
        assert!(err.contains("mode"));
    }

    #[test]
    fn null_required_rejected() {
        let err = validate_against_schema(&schema(), &json!({"path": null})).unwrap_err();
        assert!(err.contains("required"));
    }

    #[test]
    fn schema_without_properties_passes() {
        assert!(validate_against_schema(&json!({"type": "object"}), &json!({"a": 1})).is_ok());
        assert!(validate_against_schema(&json!({}), &json!({})).is_ok());
    }

    #[test]
    fn tool_result_image_serde() {
        // Without an image the field is skipped entirely (no wire noise).
        let plain = ToolResult::success("ok");
        let serialized = serde_json::to_value(&plain).unwrap();
        assert!(serialized.get("image").is_none());

        // With an image it round-trips with media_type + base64 data.
        let with_img = ToolResult::success("summary").with_image(ToolImage {
            media_type: "image/png".into(),
            data: "aGVsbG8=".into(),
        });
        let v = serde_json::to_value(&with_img).unwrap();
        assert_eq!(v["image"]["media_type"], "image/png");
        assert_eq!(v["image"]["data"], "aGVsbG8=");
        let back: ToolResult = serde_json::from_value(v).unwrap();
        assert_eq!(
            back.image.unwrap().media_type,
            "image/png",
            "image must round-trip through serialization"
        );
    }
}
