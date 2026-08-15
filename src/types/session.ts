/**
 * Session-related types — mirrors Rust `core::types::Session`.
 */

/** Session status (Rust enum, snake_case via serde). */
export type SessionStatus = "active" | "idle" | "archived" | "error";

/** Token usage accumulated across all turns in a session. */
export interface SessionTokenUsage {
  prompt_tokens: number;
  completion_tokens: number;
  cached_read_tokens?: number;
  reasoning_tokens?: number;
  // deepseek-native: KV cache hit/miss tokens
  prompt_cache_hit_tokens?: number;
  prompt_cache_miss_tokens?: number;
}

/** A chat session — holds conversation state, model config, and metadata. */
export interface Session {
  id: string;
  title: string;
  model: string;
  provider: string;
  /** Context window captured from the provider's real model metadata (0 = unknown). */
  context_window?: number;
  status: SessionStatus;
  created_at: string;
  updated_at: string;
  workspace_path?: string;
  total_usage: SessionTokenUsage;
  turn_count: number;
  system_prompt: string;
  /** Product mode this session belongs to: "code" | "depwork". */
  work_mode?: string;
  /** Per-session permission mode ("" = inherit the global default). */
  permission_mode?: string;
  /** True when the user pinned this session to the top of the sidebar list. */
  pinned?: boolean;
  /** Short preview of the session's last message (sidebar row subtitle). */
  last_message?: string;
}

/** Agent loop modes — maps to Rust `AgentLoopMode`. */
export type AgentMode =
  | "standard"
  | "plan_execute"
  | "reflexion"
  | "coordinator"
  | "evaluator_qa"
  | "goal";

/** A conversation item loaded from backend (session history). */
export interface ConversationItem {
  role: "system" | "user" | "assistant" | "tool_result" | "reasoning";
  content: string;
  tool_calls?: unknown[];
  model?: string;
  usage?: SessionTokenUsage;
  tool_call_id?: string;
  is_error?: boolean;
}
