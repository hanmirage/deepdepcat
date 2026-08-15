//! MCP tool bridge — wraps MCP server tools as native `dyn Tool` implementations.
//!
//! Each MCP tool is wrapped in an `McpToolWrapper` that implements the `Tool` trait.
//! The tool name is namespaced as `server_name__tool_name` to avoid collisions
//! with built-in tools.

use crate::toolkit::{PermissionDecision, Tool, ToolContext, ToolResult};
use crate::core::error::AppResult;
use crate::mcp::client::McpClient;
use crate::mcp::types::McpTool;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;

/// Wraps an MCP server tool as a native DeepDepCat tool.
///
/// The tool name is namespaced as `server_name__tool_name`.
pub struct McpToolWrapper {
    /// Cached namespaced name (avoids re-computing and memory leaks).
    namespaced_name: String,
    /// The MCP server this tool belongs to (surfaced on MCP App payloads).
    server_name: String,
    /// The MCP tool definition.
    tool: McpTool,
    /// The MCP client used to execute the tool.
    client: Arc<McpClient>,
}

impl McpToolWrapper {
    pub fn new(server_name: &str, tool: McpTool, client: Arc<McpClient>) -> Self {
        Self {
            namespaced_name: format!("{}__{}", server_name, tool.name),
            server_name: server_name.to_string(),
            tool,
            client,
        }
    }
}

#[async_trait]
impl Tool for McpToolWrapper {
    fn name(&self) -> &str {
        &self.namespaced_name
    }

    fn description(&self) -> &str {
        &self.tool.description
    }

    fn parameters(&self) -> Value {
        self.tool.input_schema.clone()
    }

    fn is_read_only(&self) -> bool {
        // Display classification only — surfaced in tool docs/UI. NOT used
        // for permission decisions (see is_read_only_call).
        self.tool
            .annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false)
    }

    /// MCP tools NEVER self-classify as read-only for the permission
    /// pipeline. `readOnlyHint` is the server's self-declaration — trusting
    /// it lets a mislabelled or compromised server run write tools with zero
    /// prompts and bypasses the sensitive-file red line. Every MCP call goes
    /// through Ask; the user's durable grant ("always allow") makes repeated
    /// use of trusted tools frictionless.
    fn is_read_only_call(&self, _args: &Value) -> bool {
        false
    }

    fn check_permissions(&self, _args: &Value, _context: &ToolContext) -> PermissionDecision {
        // Never self-approve — the unified permission pipeline decides every
        // MCP call (Ask, then grants / user confirmation).
        PermissionDecision::Ask
    }

    /// Side-effecting MCP tools must run SERIALLY: the parallel tool batch
    /// would race two writes to the same remote server state (and a read
    /// issued after a write could observe pre-write state). Only tools the
    /// server declares read-only (`readOnlyHint`) may parallelize — matching
    /// every built-in write tool (write_file/edit_file/bash/…) which
    /// overrides this to false.
    fn is_concurrency_safe(&self) -> bool {
        self.tool
            .annotations
            .as_ref()
            .and_then(|a| a.read_only_hint)
            .unwrap_or(false)
    }

    async fn execute(&self, args: Value, context: &ToolContext) -> AppResult<ToolResult> {
        match self
            .client
            .call_tool_detailed_with_meta(
                &self.tool.name,
                args,
                Some(&self.tool),
                Some(&context.session_id),
            )
            .await
        {
            Ok(outcome) => {
                let mut result = if outcome.is_error {
                    ToolResult::error(outcome.content)
                } else {
                    ToolResult::success(outcome.content)
                };
                // MCP Apps: surface the interactive UI payload so the
                // dispatcher can emit it to the frontend. The text content
                // (what the model reads) stays untouched.
                if let Some(app) = outcome.app {
                    let mut payload = serde_json::to_value(app).unwrap_or_default();
                    if let Some(obj) = payload.as_object_mut() {
                        obj.insert(
                            "server".to_string(),
                            serde_json::Value::String(self.server_name.clone()),
                        );
                    }
                    result = result.with_metadata("mcp_app", payload);
                }
                Ok(result)
            }
            Err(e) => Ok(ToolResult::error(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::types::{McpTool, ToolAnnotations};

    fn tool_with_read_only_hint(hint: bool) -> McpTool {
        McpTool {
            name: "write_data".to_string(),
            description: "test tool".to_string(),
            input_schema: serde_json::json!({ "type": "object" }),
            annotations: Some(ToolAnnotations {
                title: None,
                read_only_hint: Some(hint),
                destructive_hint: None,
                idempotent_hint: None,
            }),
            _meta: None,
        }
    }

    #[test]
    fn read_only_hint_surfaces_for_display_but_never_self_approves() {
        let wrapper = McpToolWrapper::new(
            "server",
            tool_with_read_only_hint(true),
            Arc::new(McpClient::default()),
        );
        // The hint is kept for display classification…
        assert!(wrapper.is_read_only());
        // …but the permission pipeline must NOT trust the server's
        // self-declaration: a mislabelled write tool would otherwise run
        // with zero prompts in AcceptEdits mode (and skip the sensitive
        // file red line).
        assert!(!wrapper.is_read_only_call(&serde_json::json!({})));
    }

    #[test]
    fn tool_without_read_only_hint_is_also_never_self_approved() {
        let wrapper = McpToolWrapper::new(
            "server",
            tool_with_read_only_hint(false),
            Arc::new(McpClient::default()),
        );
        assert!(!wrapper.is_read_only_call(&serde_json::json!({})));
    }

    #[test]
    fn only_read_only_hinted_tools_are_concurrency_safe() {
        // A side-effecting MCP tool (no readOnlyHint, or hint=false) must run
        // SERIALLY — the parallel batch would race two writes to the same
        // remote state. Only a declared read-only tool may parallelize.
        let side_effecting = McpToolWrapper::new(
            "server",
            tool_with_read_only_hint(false),
            Arc::new(McpClient::default()),
        );
        assert!(!side_effecting.is_concurrency_safe());

        let unknown = McpToolWrapper::new(
            "server",
            McpTool {
                name: "no_hint".to_string(),
                description: "t".to_string(),
                input_schema: serde_json::json!({ "type": "object" }),
                annotations: None,
                _meta: None,
            },
            Arc::new(McpClient::default()),
        );
        assert!(!unknown.is_concurrency_safe(), "unknown must default to serial");

        let read_only = McpToolWrapper::new(
            "server",
            tool_with_read_only_hint(true),
            Arc::new(McpClient::default()),
        );
        assert!(read_only.is_concurrency_safe());
    }
}
