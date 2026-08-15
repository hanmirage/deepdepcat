/**
 * MCP (Model Context Protocol) type definitions.
 *
 * Mirrors Rust structs from `src-tauri/src/mcp/types.rs` and
 * `src-tauri/src/core/config.rs`.
 *
 * Rust `#[serde(rename = "type")]` on `transport_type` means the
 * JSON key is `type`, not `transport_type` — the TypeScript interface
 * uses `type` to match the wire format exactly.
 */

/** Transport type for an MCP server connection. */
export type McpTransportType = "stdio" | "sse" | "http";

/** Tool annotations from MCP — hints about tool behavior. */
export interface McpToolAnnotations {
  title?: string;
  readOnlyHint?: boolean;
  destructiveHint?: boolean;
  idempotentHint?: boolean;
}

/** A tool exposed by an MCP server. Mirrors Rust `McpTool`. */
export interface McpTool {
  name: string;
  description: string;
  inputSchema: unknown;
  annotations?: McpToolAnnotations;
}

/** A resource exposed by an MCP server. Mirrors Rust `McpResource`. */
export interface McpResource {
  uri: string;
  name: string;
  description?: string;
  mimeType?: string;
}

/** MCP server configuration. Mirrors Rust `McpServerConfig`.
 *
 * Note: Rust uses `#[serde(rename = "type")]` on the `transport_type` field,
 * so the JSON wire format uses `type` as the key. */
export interface McpServerConfig {
  name: string;
  type: McpTransportType;
  command: string | null;
  args: string[];
  env: Record<string, string>;
  url: string | null;
  enabled: boolean;
}

/** Connection status of an MCP server (tracked in frontend only).
 *  `installing` = auto-setup is building the bundled server's Python venv. */
export type McpConnectionStatus =
  | "disconnected"
  | "connecting"
  | "connected"
  | "error"
  | "installing";

/** An MCP server with its live connection status — frontend-only augmentation. */
export interface McpServerWithStatus extends McpServerConfig {
  status: McpConnectionStatus;
  tools: McpTool[];
  errorMessage: string | null;
}

/** OAuth credential input for an MCP server (settings dialog form). */
export interface McpCredentialInput {
  /** OAuth token endpoint used for refresh-grant renewal. */
  tokenEndpoint: string;
  /** OAuth client id sent with the refresh grant. */
  clientId: string;
  accessToken: string;
  refreshToken: string;
  /** Token type, e.g. "Bearer". */
  tokenType: string;
  /** RFC3339 expiry of the access token ("" = unknown). */
  expiresAt: string;
}

/** Backend → frontend connection-status event (`mcp-status-changed`). */
export interface McpStatusEvent {
  name: string;
  status: McpConnectionStatus;
  error?: string | null;
  /** Tool count on a successful connect (informational). */
  tools?: number;
}
