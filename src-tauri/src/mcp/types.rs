//! MCP types — protocol data structures.
//!
//! Based on the MCP specification (JSON-RPC 2.0 over stdio/SSE/HTTP).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A tool exposed by an MCP server.
///
/// `rename_all = "camelCase"` matches the MCP spec wire format
/// (`inputSchema`); without it every real server's tool fails to
/// deserialize and discovery silently yields nothing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
    /// `_meta` from the tool definition — the MCP Apps extension declares
    /// `_meta.ui.resourceUri` here so hosts can preload the interactive UI
    /// for a tool before it is even called. Explicit `rename` so the
    /// camelCase rename_all never touches the leading underscore.
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub _meta: Option<Value>,
}

/// Tool annotations from MCP (spec wire format: `readOnlyHint`, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
}

/// A single content block from an MCP `tools/call` result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpCallContentBlock {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<Value>,
}

/// A `tools/call` result — the server's response to a tool invocation.
///
/// Per the MCP spec, the result may carry text content blocks, a
/// `structuredContent` (the tool's own structured return value, when the
/// server declares `outputSchema`), and an `isError` flag that marks a
/// tool-level execution failure (distinct from a transport/JSON-RPC error).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallToolResult {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub content: Vec<McpCallContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// A resource exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
}

/// A prompt template exposed by an MCP server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub arguments: Vec<PromptArgument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

// ── JSON-RPC Types ────────────────────────────────────────────────────────────

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.into(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC notification (no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

/// A message that can be sent or received.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcMessage {
    Request(JsonRpcRequest),
    Response(JsonRpcResponse),
    Notification(JsonRpcNotification),
}

// ── Elicitation Types ─────────────────────────────────────────────────────

/// The user's response to an elicitation request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationResult {
    /// "accept" = user provided a value, "decline" = user refused,
    /// "cancel" = user dismissed the dialog.
    pub action: String,
    /// The user's input (only present when action = "accept").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_tool_deserializes_spec_wire_format() {
        // A real MCP server sends camelCase (`inputSchema`) plus the MCP Apps
        // `_meta.ui` declaration — the discovery path must not drop it.
        let json = serde_json::json!({
            "name": "make_dashboard",
            "description": "Render a dashboard.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": {
                "title": "Dashboard",
                "readOnlyHint": true,
                "destructiveHint": false,
                "idempotentHint": true
            },
            "_meta": { "ui": { "resourceUri": "ui://app/dashboard" } }
        });

        let tool: McpTool = serde_json::from_value(json).expect("spec-shaped tool parses");
        assert_eq!(tool.name, "make_dashboard");
        assert!(tool.input_schema.get("type").is_some());
        let annotations = tool.annotations.expect("annotations parsed");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
    }
}
