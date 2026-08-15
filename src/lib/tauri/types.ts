/**
 * Tauri API bridge — split by domain (see index.ts for the barrel).
 * Types mirror Rust structs in src-tauri; every invoke is typed.
 */

/** Types owned by src/types (shared with the rest of the app). */
export type {
  Session,
  AgentMode,
  PermissionDecision,
  PermissionMode,
  ContextChip,
  McpServerConfig,
  McpTool,
  Skill,
  Memory,
  DreamResult,
  MemorySearchResult,
} from "@/types";

// ── Types (mirrors Rust structs) ──────────────────────────────

export interface SystemInfo {
  os: string;
  arch: string;
  cpu_count: number;
  total_memory_mb: number;
  app_version: string;
  app_data_dir?: string;
}

/** One todo item from the backend todo_write tool (mirrors Rust TodoItem). */
export interface TodoItem {
  id: string;
  content: string;
  status: "pending" | "in_progress" | "completed";
  priority?: string;
  /** Parent todo id — makes this a child step of a phase item. */
  parent_id?: string;
  /** Step ids this step must wait on — may only leave pending once all completed. */
  depends_on?: string[];
  /** Concrete command/check proving this step is done (test/lint/typecheck/run). */
  verify?: string;
}

export type AgentStatus = "idle" | "thinking" | "tool_running" | "connecting" | "error" | "paused";

export interface ModelInfo {
  id: string;
  name: string;
  provider: string;
  description: string;
  context_window: number;
  /** Internal-workhorse models (compaction/dream) excluded from the picker. */
  hidden?: boolean;
}

export interface ChatMessage {
  role: "user" | "assistant" | "system";
  content: string;
  model: string;
}

/** Rich content block for ToolProgress::Content. Mirrors Rust ContentBlock. */
export interface ProgressContentBlock {
  type: "text" | "image" | "resource";
  text?: string;
  mime_type?: string;
  data?: string;
  uri?: string;
}

/** Token usage event payload (deepseek-native: includes KV cache + reasoning fields). */
export interface TokenUsageEvent {
  prompt_tokens: number;
  completion_tokens: number;
  cached_read_tokens?: number;
  reasoning_tokens?: number;
  prompt_cache_hit_tokens?: number;
  prompt_cache_miss_tokens?: number;
}

/** One persisted replay-exact agent event (mirrors Rust AgentEvent). */
export interface AgentEvent {
  id: number;
  session_id: string;
  turn_id: string | null;
  seq: number;
  kind: "model_call" | "tool_run" | "approval" | "edit" | string;
  payload: Record<string, unknown>;
  created_at: string;
}

/** One durable "always allow" grant (mirrors Rust PermissionGrant). */
export interface PermissionGrant {
  tool_name: string;
  pattern: string;
  created_at: string;
}

/** Settings rule snapshot for the governance UI. */
export interface PermissionRulesView {
  mode: string;
  allow: string[];
  deny: string[];
  ask: string[];
}

/** Terminal turn outcome — mirrors Rust `TurnOutcome` (renamed from
 *  `TurnStatus` so the live-phase `turn_status` event keeps the name). */
export type TurnOutcome =
  | "done"
  | "needs_input"
  | "failed"
  | "limit"
  | "cancelled"
  | "denied";

/** Live phase of a running turn — mirrors Rust `TurnPhase`. */
export type TurnPhase = "verifying";

/** One tool call's authoritative terminal state (snapshot repair payload). */
export interface ToolCallSnapshot {
  call_id: string;
  name: string;
  arguments: string;
  result?: string | null;
  is_error: boolean;
}

/** An interactive MCP app attached to a tool result (snapshot repair). */
export interface McpAppSnapshot {
  call_id: string;
  name: string;
  server: string;
  resource_uri: string;
  html: string;
  is_error: boolean;
  csp?: unknown;
}

/** Authoritative terminal state of one turn — mirrors Rust `TurnSnapshot`. */
export interface TurnSnapshot {
  turn_id: string;
  session_id: string;
  status: TurnOutcome;
  reason: string;
  text: string;
  reasoning: string;
  tool_calls: ToolCallSnapshot[];
  mcp_apps: McpAppSnapshot[];
  usage?: TokenUsageEvent | null;
  trace_id?: string | null;
}

/** Raw `chat-stream` event body (wire shape minus the seq envelope). */
export type StreamEventShape =
  | { type: "turn_start"; turn_id: string; session_id: string; model: string; trace_id?: string | null }
  | { type: "text_delta"; turn_id: string; text: string }
  | { type: "reasoning_delta"; turn_id: string; text: string }
  | { type: "tool_call_start"; turn_id: string; call_id: string; name: string }
  | { type: "tool_call_delta"; turn_id: string; call_id: string; arguments: string }
  | { type: "tool_call_progress"; turn_id: string; call_id: string; name: string; kind: string; delta?: string | null; total_bytes?: number | null }
  | { type: "tool_call_result"; turn_id: string; call_id: string; name: string; result: string; is_error: boolean }
  | { type: "usage"; turn_id: string; usage: TokenUsageEvent }
  | { type: "turn_end"; turn_id: string; session_id: string; reason: string; status?: TurnOutcome; trace_id?: string | null }
  | { type: "turn_status"; turn_id: string; session_id: string; phase: TurnPhase; reason: string; trace_id?: string | null }
  | { type: "snapshot"; snapshot: TurnSnapshot }
  | { type: "error"; turn_id: string; session_id: string; message: string; trace_id?: string | null }
  | { type: "compaction"; session_id: string; compacted_tokens: number; summary: string }
  | { type: "memory_injected"; session_id: string; count: number; snippet: string }
  | { type: "subagent_start"; subagent_id: string; task: string; agent_type: string; tool_call_id?: string | null; session_id?: string | null }
  | { type: "subagent_progress"; subagent_id: string; message: string; turn: number; total_turns: number; tool_call_id?: string | null; session_id?: string | null }
  | { type: "subagent_result"; subagent_id: string; result: string; success: boolean; tool_call_id?: string | null; session_id?: string | null }
  | { type: "elicitation"; elicitation_id: string; server_name: string; message: string; requested_schema?: unknown }
  | { type: "mcp_app"; turn_id: string; call_id: string; name: string; server: string; resource_uri: string; html: string; is_error: boolean; csp?: unknown };

/** Wire event — every backend event rides in a `{ seq }` envelope; the
 *  monotonic seq per turn lets the listener detect lost deltas and pull a
 *  terminal `snapshot` for repair. */
export type ChatStreamEvent = { seq: number } & StreamEventShape;
export interface DepworkTask {
  id: string;
  description: string;
  status: "pending" | "running" | "completed" | "failed";
  context_paths: string[];
  created_at: string;
}

export interface Connector {
  id: string;
  name: string;
  service: string;
  status: string;
  icon: string | null;
  permissions: Permission[];
}

export interface Permission {
  resource: string;
  resource_type: string;
  access: string;
  enabled: boolean;
}

export interface Plugin {
  id: string;
  name: string;
  description: string;
  version: string;
  author: string;
  installed: boolean;
  enabled: boolean;
}


/** Snapshot of the agent browser takeover session (mirrors Rust BrowserStatus). */
export interface BrowserStatus {
    running: boolean;
    url: string | null;
    title: string | null;
    awaiting_takeover: boolean;
    takeover_reason: string | null;
    profile: string | null;
    headless: boolean;
    download_dir: string | null;
  }

/** Browser-dev-mode fallback status (never running). */

/** Event payload of "browser-takeover-requested" (agent paused for user). */
export interface BrowserTakeoverRequest {
  reason: string;
}

/**
 * Browser takeover API — drive the real agent browser from the DevBrowser
 * "接管浏览器" mode. The agent pauses on `handoff` until the user clicks
 * "我已接管完成，继续" (browser_takeover_resume).
 */
