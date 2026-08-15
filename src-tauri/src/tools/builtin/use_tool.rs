//! Use-tool (meta-tool) — dynamic tool dispatch by name.
//!
//! Allows the LLM to invoke another tool by name with custom arguments,
//! acting as a meta-tool for indirect dispatch. This is useful when the
//! agent needs to dynamically select which tool to call based on runtime
//! conditions.

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult, ToolStreamItem};
use crate::core::error::{AppError, AppResult};
use crate::tools::registry::ToolRegistry;
use async_trait::async_trait;
use futures_util::StreamExt;
use serde_json::{json, Value};

/// Validate the target tool's arguments against its schema before forwarding.
///
/// Mirrors the dispatcher's `validate_args` call so a meta-tool forwards only
/// well-formed arguments. Extracted for unit-testability (pure, no `ToolContext`).
fn validate_target_args(tool: &dyn Tool, args: &Value) -> Result<(), String> {
    tool.validate_args(args)
}

/// Meta-tool that dispatches to another registered tool by name.
///
/// Holds a shared clone of the registry (`ToolRegistry::clone` shares the
/// underlying map), so tools registered later — including MCP tools —
/// are dispatchable.
pub struct UseTool {
    registry: ToolRegistry,
}

impl UseTool {
    /// Create a new use_tool with the given registry (snapshot clone).
    pub fn new(registry: ToolRegistry) -> Self {
        Self { registry }
    }
}

#[async_trait]
impl Tool for UseTool {
    fn name(&self) -> &str {
        "use_tool"
    }

    fn description(&self) -> &str {
        "Dynamically invoke another tool by name with custom arguments. \
        Use this when you need to call a tool whose name is determined at runtime. \
        The target tool's permissions and safety checks still apply."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tool_name": {
                    "type": "string",
                    "description": "The name of the tool to invoke"
                },
                "arguments": {
                    "type": "object",
                    "description": "The arguments to pass to the target tool"
                }
            },
            "required": ["tool_name", "arguments"]
        })
    }

    fn scope(&self) -> crate::toolkit::ToolScope {
        // Meta-tool: can reach ANY registered tool (bash, edit_file, LSP,
        // code intelligence) — Depwork's "NO shell / office tools only"
        // boundary must not be bypassable through a wrapper. Code-only.
        crate::toolkit::ToolScope::Code
    }

    fn is_read_only(&self) -> bool {
        // This meta-tool's read-only status depends on the target tool.
        // We err on the side of asking for permission.
        false
    }

    /// The meta-tool can reach WRITE tools (edit_file, bash, ...) whose
    /// target files a parallel read could race — never batch it.
    fn is_concurrency_safe(&self) -> bool {
        false
    }

    fn check_permissions(&self, args: &Value, ctx: &ToolContext) -> PermissionDecision {
        // Defer to the target tool's permission check if possible.
        let tool_name = args.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");

        // Worker boundary: a subagent (any depth > 0) must not smuggle
        // `ask_user` through the meta-tool — its toolset excludes it by
        // design, and the worker reports ambiguity to the parent instead.
        if ctx.agent_depth > 0 && tool_name == "ask_user" {
            return PermissionDecision::Deny(
                "ask_user is not available to subagents — report the ambiguity in your final result instead".to_string(),
            );
        }

        if let Some(tool) = self.registry.get(tool_name) {
            // Mode boundary: a meta-tool must not smuggle tools from the
            // other product mode into this execution.
            if !ctx.work_mode.allows(tool.scope()) {
                return PermissionDecision::Deny(format!(
                    "Tool '{tool_name}' is not available in {} mode",
                    ctx.work_mode.as_str()
                ));
            }
            let target_args = args.get("arguments").cloned().unwrap_or(json!({}));
            return tool.check_permissions(&target_args, ctx);
        }

        PermissionDecision::Ask
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> AppResult<ToolResult> {
        let tool_name = args
            .get("tool_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AppError::ToolNotFound("missing 'tool_name'".into()))?;

        let target_args = args.get("arguments").cloned().unwrap_or(json!({}));

        let tool = self
            .registry
            .get(tool_name)
            .ok_or_else(|| AppError::ToolNotFound(format!("Tool '{tool_name}' not registered")))?;

        // Worker boundary — same rule as check_permissions, enforced at
        // execution time for every dispatch path.
        if ctx.agent_depth > 0 && tool_name == "ask_user" {
            return Err(AppError::ToolNotFound(
                "ask_user is not available to subagents — report the ambiguity in your final result instead".to_string(),
            ));
        }

        // Mode boundary — same check as check_permissions, enforced at
        // execution time for every dispatch path.
        if !ctx.work_mode.allows(tool.scope()) {
            return Err(AppError::ToolNotFound(format!(
                "Tool '{tool_name}' is not available in {} mode",
                ctx.work_mode.as_str()
            )));
        }

        // Schema validation before anything executes — mirror the dispatcher
        // (a meta-tool must not let malformed arguments reach the target tool
        // without the same fail-fast check).
        if let Err(reason) = validate_target_args(tool.as_ref(), &target_args) {
            return Err(AppError::ToolExecution {
                tool_name: tool_name.to_string(),
                message: format!("Invalid arguments: {reason}"),
            });
        }

        // Forward through the streaming path so progress events from
        // streaming tools (e.g. bash) still reach the frontend. The same
        // wall-clock timeout as the dispatcher guarantees a target tool can
        // never hang the loop through the meta-tool path.
        let stream = tool.execute_stream(target_args, ctx).await?;
        let stream = tokio::time::timeout(
            std::time::Duration::from_secs(crate::tools::dispatch::TOOL_EXECUTION_TIMEOUT_SECS),
            async {
                let mut stream = stream;
                if let Some(item) = stream.next().await {
                    let ToolStreamItem::Terminal(result) = item;
                    return result.map_err(|e| AppError::ToolExecution {
                        tool_name: tool_name.to_string(),
                        message: e,
                    });
                }
                Err(AppError::ToolExecution {
                    tool_name: tool_name.to_string(),
                    message: "Tool stream ended without a terminal result".to_string(),
                })
            },
        )
        .await;

        match stream {
            Ok(result) => result,
            Err(_) => Err(AppError::ToolExecution {
                tool_name: tool_name.to_string(),
                message: format!(
                    "Tool execution timed out after {}s",
                    crate::tools::dispatch::TOOL_EXECUTION_TIMEOUT_SECS
                ),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_tool_name_and_description() {
        let registry = ToolRegistry::new();
        let tool = UseTool::new(registry);
        assert_eq!(tool.name(), "use_tool");
        assert!(tool.description().contains("invoke"));
    }

    #[test]
    fn use_tool_not_read_only() {
        let registry = ToolRegistry::new();
        let tool = UseTool::new(registry);
        assert!(!tool.is_read_only());
    }

    #[test]
    fn validate_target_args_rejects_malformed_args() {
        // The read_file tool requires a string `path`. Through use_tool, that
        // schema must be enforced before dispatch — mirroring the dispatcher.
        let read_file = super::super::read_file::ReadFileTool::new(1000);
        // Malformed: path missing entirely.
        assert!(validate_target_args(&read_file, &json!({})).is_err());
        // Malformed: path is the wrong type.
        assert!(validate_target_args(&read_file, &json!({"path": 42})).is_err());
        // Well-formed.
        assert!(validate_target_args(&read_file, &json!({"path": "/a/b.rs"})).is_ok());
    }
}
