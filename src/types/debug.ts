/**
 * Debug tracing types — events emitted by the backend for debugging.
 *
 * The backend emits these via `app.emit("debug-trace", payload)` when
 * `debug_mode` is enabled in the config. The frontend listens and
 * displays them in the DebugPanel.
 *
 * ── Note for backend developer ──────────────────────────────────
 * The Rust `DebugEvent` enum must use `#[serde(tag = "type", rename_all = "snake_case")]`
 * to match this TypeScript discriminated union.
 */

/** A single debug event from the backend. */
export type DebugEvent =
  | { type: "agent_turn_start"; session_id: string; turn: number; mode: string; timestamp: number }
  | { type: "agent_turn_end"; session_id: string; turn: number; duration_ms: number; timestamp: number }
  | { type: "llm_call_start"; session_id: string; model: string; message_count: number; timestamp: number }
  | {
      type: "llm_call_end";
      session_id: string;
      model: string;
      duration_ms: number;
      usage: { prompt_tokens: number; completion_tokens: number };
      timestamp: number;
    }
  | { type: "tool_dispatch"; session_id: string; tool_name: string; arguments: string; timestamp: number }
  | {
      type: "tool_result";
      session_id: string;
      tool_name: string;
      duration_ms: number;
      is_error: boolean;
      timestamp: number;
    }
  | {
      type: "memory_search";
      session_id: string;
      query: string;
      results_count: number;
      duration_ms: number;
      timestamp: number;
    }
  | { type: "memory_inject"; session_id: string; memories_count: number; timestamp: number }
  | {
      type: "permission_check";
      session_id: string;
      resource: string;
      action: string;
      allowed: boolean;
      timestamp: number;
    }
  | { type: "hook_trigger"; session_id: string; event: string; timestamp: number }
  | {
      type: "hook_execute";
      session_id: string;
      event: string;
      hook_id: string;
      duration_ms: number;
      timestamp: number;
    }
  | { type: "compaction"; session_id: string; compacted_tokens: number; summary: string; timestamp: number }
  | { type: "session_create"; session_id: string; model: string; provider: string; timestamp: number };

/** All debug event type strings — used for filtering. */
export type DebugEventType = DebugEvent["type"];

/** Category grouping for UI display. */
export const DEBUG_EVENT_CATEGORIES: { label: string; types: DebugEventType[] }[] = [
  { label: "Agent", types: ["agent_turn_start", "agent_turn_end"] },
  { label: "LLM", types: ["llm_call_start", "llm_call_end"] },
  { label: "Tool", types: ["tool_dispatch", "tool_result"] },
  { label: "Memory", types: ["memory_search", "memory_inject"] },
  { label: "Permission", types: ["permission_check"] },
  { label: "Hook", types: ["hook_trigger", "hook_execute"] },
  { label: "Session", types: ["compaction", "session_create"] },
];
