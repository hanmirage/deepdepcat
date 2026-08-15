/**
 * Tauri API bridge — split by domain (see index.ts for the barrel).
 * Types mirror Rust structs in src-tauri; every invoke is typed.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauri, mockEmit, setMockCancelStream, clearMockCancelStream } from "../core";
import { MOCK_SESSION, MOCK_SYSTEM_INFO, MOCK_REPLY, MOCK_TASKS } from "../mock";
import type {
  SystemInfo,
  AgentStatus,
  TodoItem,
  DepworkTask,
  Connector,
  Plugin,
  AgentEvent,
  ChatStreamEvent,
  TurnSnapshot,
  StreamEventShape,
} from "../types";
import type { Session, AgentMode, ContextChip, Skill } from "@/types";

/** Browser-dev mock seq — the real wire envelopes every event with a
 *  monotonic seq; the mock mimics that so listener tests match production. */
let mockStreamSeq = 0;

/** Envelope a mock event body with the next seq (mirrors the Rust wire). */
export function mockStreamEvent(body: StreamEventShape): ChatStreamEvent {
  mockStreamSeq += 1;
  return { seq: mockStreamSeq, ...body };
}

// ── Command wrappers ─────────────────────────────────────────

/** System commands */
export const systemApi = {
  getSystemInfo: () =>
    isTauri ? invoke<SystemInfo>("get_system_info") : Promise.resolve(MOCK_SYSTEM_INFO),

  getAgentStatus: () =>
    isTauri ? invoke<AgentStatus>("get_agent_status") : Promise.resolve<AgentStatus>("idle"),

  setAgentStatus: (status: AgentStatus) =>
    isTauri ? invoke<void>("set_agent_status", { status }) : Promise.resolve(),

  cancelOperation: (sessionId: string) => {
    if (isTauri) return invoke<boolean>("cancel_operation", { sessionId });
    // Mock: cancel the active mock stream by clearing the interval and resolving the Promise
    clearMockCancelStream();
    return Promise.resolve(true);
  },

  pauseOperation: (sessionId: string) =>
    isTauri ? invoke<boolean>("pause_operation", { sessionId }) : Promise.resolve(true),

  resumeOperation: (sessionId: string) =>
    isTauri ? invoke<boolean>("resume_operation", { sessionId }) : Promise.resolve(true),

  setDebugMode: (enabled: boolean) =>
    isTauri ? invoke<void>("set_debug_mode", { enabled }) : Promise.resolve(),

  getDebugMode: () =>
    isTauri ? invoke<boolean>("get_debug_mode") : Promise.resolve(false),

  setWorkspace: (path: string | null) =>
    isTauri ? invoke<void>("set_workspace", { path }) : Promise.resolve(),
};

/** Config commands */
export const configApi = {
  getConfig: () =>
    isTauri ? invoke<Record<string, unknown>>("get_config") : Promise.resolve({ llm: { providers: [] } }),

  updateConfig: (config: Record<string, unknown>) =>
    isTauri ? invoke<void>("update_config", { newConfig: config }) : Promise.resolve(),
};

/** Structured `send_chat_message` envelope (see commands/chat.rs) — the
 *  magic-string protocol ("queued:..."/"cancelled"/bare turn id) is gone:
 *  - kind "accepted": turn ran (turn_id of the last turn)
 *  - kind "queued": prompt queued for replay (prompt_id)
 *  - kind "cancelled": invoke aborted by user cancel */
export interface SendChatResult {
  kind: "accepted" | "queued" | "cancelled";
  prompt_id: string | null;
  turn_id: string | null;
}

/** Chat commands */
export const chatApi = {
  /** Send a chat message and stream the response. */
  sendMessage: (sessionId: string, message: string, mode?: AgentMode, workMode?: string, contextChips?: ContextChip[], reasoningMode?: string, agentName?: string) => {
    if (isTauri) {
      return invoke<SendChatResult>("send_chat_message", { sessionId, message, mode, workMode, contextChips, reasoningMode, agentName: agentName ?? null });
    }
    // ── Mock streaming reply via mockEmit ──
    // Returns a Promise that resolves only after all mock events have been emitted,
    // mirroring the real Tauri backend which resolves when streaming completes.
    return new Promise<SendChatResult>((resolve) => {
      const turnId = `mock-turn-${Date.now()}`;
      const text = MOCK_REPLY + message.slice(0, 200);
      const words = text.split(" ");
      // Three sequential tool calls with gaps, so the exec panel visibly
      // streams each step and then auto-collapses.
      const tools: { name: string; args: Record<string, unknown>; result: string }[] = [
        { name: "read_file", args: { path: "src/auth/mod.rs" }, result: "文件内容：trait AuthProvider { ... }" },
        { name: "grep", args: { pattern: "AuthProvider" }, result: "3 处引用" },
        { name: "edit_file", args: { path: "src/auth/service.rs", old_text: "auth", new_text: "auth_v2" }, result: "修改成功" },
      ];
      let phase: "text" | "tools" | "done" = "text";
      let toolIdx = 0;
      let toolDone = true; // gates starting the next tool after the previous result
      let i = 0;

      const startTool = (idx: number) => {
        const t = tools[idx];
      mockEmit("chat-stream", mockStreamEvent({ type: "tool_call_start", turn_id: turnId, call_id: `mock-tc-${idx + 1}`, name: t.name }));
      mockEmit("chat-stream", mockStreamEvent({ type: "tool_call_delta", turn_id: turnId, call_id: `mock-tc-${idx + 1}`, arguments: JSON.stringify(t.args) }));
      };

      mockEmit("chat-stream", mockStreamEvent({ type: "turn_start", turn_id: turnId, session_id: sessionId, model: "deepseek-chat" }));
      const interval = setInterval(() => {
        if (phase === "text") {
          if (i < words.length) {
            const chunk = words.slice(i, i + 3).join(" ") + " ";
            i += 3;
            mockEmit("chat-stream", mockStreamEvent({ type: "text_delta", turn_id: turnId, text: chunk }));
          } else {
            phase = "tools";
            toolDone = true;
          }
        } else if (phase === "tools") {
          if (toolDone && toolIdx < tools.length) {
            startTool(toolIdx);
            toolDone = false;
          } else if (!toolDone) {
            // Finish the current tool, then move to the next (or finish the turn).
            const t = tools[toolIdx];
            mockEmit("chat-stream", mockStreamEvent({ type: "tool_call_result", turn_id: turnId, call_id: `mock-tc-${toolIdx + 1}`, name: t.name, result: t.result, is_error: false }));
            toolIdx++;
            toolDone = true;
            if (toolIdx >= tools.length) phase = "done";
          }
        } else {
          mockEmit("chat-stream", mockStreamEvent({ type: "usage", turn_id: turnId, usage: { prompt_tokens: 50, completion_tokens: words.length * 2 } }));
          mockEmit("chat-stream", mockStreamEvent({ type: "turn_end", turn_id: turnId, session_id: sessionId, reason: "stop" }));
          clearInterval(interval);
          clearMockCancelStream();
          resolve({ kind: "accepted", prompt_id: null, turn_id: turnId });
        }
      }, 300);
      // Store cancel function so stopStreaming can clean up the mock timer
      setMockCancelStream(() => {
        clearInterval(interval);
        clearMockCancelStream();
        resolve({ kind: "accepted", prompt_id: null, turn_id: turnId });
      });
    });
  },
  /** Pull the authoritative terminal snapshot of a turn (gap recovery). */
  getTurnSnapshot: (sessionId: string, turnId: string): Promise<TurnSnapshot | null> =>
    isTauri
      ? invoke<TurnSnapshot | null>("get_turn_snapshot", { sessionId, turnId })
      : Promise.resolve(null),
};

/** Session commands */
export const sessionApi = {
  createSession: (
    model?: string,
    provider?: string,
    workspacePath?: string,
    workMode?: string,
    contextWindow?: number,
    permissionMode?: string,
  ) =>
    isTauri
      ? invoke<Session>("create_session", {
          model,
          provider,
          workspacePath,
          workMode,
          contextWindow,
          permissionMode,
        })
      : Promise.resolve({
          ...MOCK_SESSION,
          model: model ?? "deepseek-chat",
          provider: provider ?? "deepseek",
          work_mode: workMode ?? "code",
          context_window: contextWindow ?? 0,
        }),

  listSessions: (limit?: number) =>
    isTauri ? invoke<Session[]>("list_sessions", { limit }) : Promise.resolve([MOCK_SESSION]),

  getSession: (sessionId: string) =>
    isTauri ? invoke<Session>("get_session", { sessionId }) : Promise.resolve(MOCK_SESSION),

  deleteSession: (sessionId: string) =>
    isTauri ? invoke<void>("delete_session", { sessionId }) : Promise.resolve(),

  getSessionMessages: (sessionId: string) =>
    isTauri ? invoke<unknown[]>("get_session_messages", { sessionId }) : Promise.resolve([]),

  updateSessionTitle: (sessionId: string, title: string) =>
    isTauri ? invoke<void>("update_session_title", { sessionId, title }) : Promise.resolve(),

  /** Pin or unpin a session (sidebar top-of-list placement). */
  setSessionPinned: (sessionId: string, pinned: boolean) =>
    isTauri ? invoke<void>("set_session_pinned", { sessionId, pinned }) : Promise.resolve(),

  updateSessionModel: (sessionId: string, model: string) =>
    isTauri ? invoke<void>("update_session_model", { sessionId, model }) : Promise.resolve(),

  /** Recall a user message and everything after it (backend truncates + persists). */
  deleteMessage: (sessionId: string, userContent: string) =>
    isTauri ? invoke<void>("delete_message", { sessionId, userContent }) : Promise.resolve(),

  /** Get the declared session goal (update_goal tool / goal capsule). */
  getGoal: (sessionId: string): Promise<string | null> =>
    isTauri ? invoke<string | null>("get_session_goal", { sessionId }) : Promise.resolve(null),

  /** Set (or clear with "") the declared session goal. */
  setGoal: (sessionId: string, goal: string): Promise<void> =>
    isTauri ? invoke<void>("set_session_goal", { sessionId, goal }) : Promise.resolve(),

  /** Get the persisted todo list for a session (todo_write tool). */
  getSessionTodos: (sessionId: string): Promise<TodoItem[]> =>
    isTauri ? invoke<TodoItem[]>("get_session_todos", { sessionId }) : Promise.resolve([]),

  /** Fetch token/tool usage summary for a session (usage ring in the input bar). */
  getSessionUsage: (sessionId: string) =>
    isTauri
      ? invoke<SessionUsageSummary>("get_session_usage", { sessionId })
      : Promise.resolve({
          session_id: sessionId,
          total_prompt_tokens: 0,
          total_completion_tokens: 0,
          total_cached_read_tokens: 0,
          total_reasoning_tokens: 0,
          total_tool_calls: 0,
          total_tool_result_tokens: 0,
          turn_count: 0,
          context_window: 0,
          current_context_tokens: 0,
          context_breakdown: {
            system_prompt_tokens: 0,
            skill_tokens: 0,
            tool_definition_tokens: 0,
            conversation_tokens: 0,
            tool_result_tokens: 0,
          },
          total_cache_hit_tokens: 0,
          total_cache_miss_tokens: 0,
          cache_hit_ratio: null,
        }),

  /** Fetch cumulative usage across ALL sessions (settings usage page). */
  getGlobalUsage: () =>
    isTauri
      ? invoke<GlobalUsageSummary>("get_global_usage")
      : Promise.resolve({
          prompt_tokens: 0,
          completion_tokens: 0,
          cached_read_tokens: 0,
          reasoning_tokens: 0,
          cache_hit_tokens: 0,
          cache_miss_tokens: 0,
          tool_calls: 0,
          tool_result_tokens: 0,
          turns: 0,
        }),

  /** List the replay-exact agent event log for a session (newest first).
   *  Pass `turnId` to replay one turn in exact execution order. */
  getSessionEvents: (
    sessionId: string,
    limit?: number,
    turnId?: string,
  ): Promise<AgentEvent[]> =>
    isTauri
      ? invoke<AgentEvent[]>("get_session_events", { sessionId, limit, turnId })
      : Promise.resolve([]),
};

/** One in-flight main-agent turn (mirrors Rust RunningTurnInfo). */
export interface RunningTurnInfo {
  session_id: string;
  turn_id: string;
  started_at_ms: number;
  message_preview: string;
  work_mode: "code" | "depwork";
  status: "running" | "paused";
}

/** Broadcast when a background turn finishes (mirrors Rust payload). */
export interface TurnCompletedPayload {
  session_id: string;
  turn_id: string;
  status: string;
}

/** Persistence view — main-agent turns still running in the background. */
export const runningSessionsApi = {
  list: () =>
    isTauri
      ? invoke<RunningTurnInfo[]>("list_running_sessions")
      : Promise.resolve<RunningTurnInfo[]>([]),
};

/** Per-session usage summary (token counts, tool calls, turns). */
export interface ContextBreakdown {
  system_prompt_tokens: number;
  skill_tokens: number;
  tool_definition_tokens: number;
  conversation_tokens: number;
  tool_result_tokens: number;
}

/** Per-session usage summary (token counts, tool calls, turns). */
export interface SessionUsageSummary {
  session_id: string;
  total_prompt_tokens: number;
  total_completion_tokens: number;
  total_cached_read_tokens: number;
  total_reasoning_tokens: number;
  total_tool_calls: number;
  total_tool_result_tokens: number;
  turn_count: number;
  /** Model context window in tokens; 0 = unknown (UI falls back). */
  context_window: number;
  /** Current context occupancy — last request's input size (what a
   *  Claude-style usage indicator shows). */
  current_context_tokens: number;
  /** Estimated split of the current context by category. */
  context_breakdown: ContextBreakdown;
  /** Cumulative prefix-cache hit tokens (DeepSeek). */
  total_cache_hit_tokens: number;
  /** Cumulative prefix-cache miss tokens (DeepSeek). */
  total_cache_miss_tokens: number;
  /** Live prefix-cache hit ratio over the recent turns (0..1), or
   *  undefined when the provider reports no cache accounting. */
  cache_hit_ratio?: number | null;
  /** Per-request prefix-cache accounting (recent window, oldest first) —
   *  the usage page renders this as the prefix-stability strip. */
  cache_history?: CacheRequest[];
}

/** One request's prefix-cache accounting for the usage-page history strip. */
export interface CacheRequest {
  hit_tokens: number;
  miss_tokens: number;
  /** True when this request missed heavily right after a hit — the
   *  prompt/context changed and the DeepSeek prefix was invalidated. */
  invalidated: boolean;
}

/** Cumulative usage across all sessions, all time (durable SQLite row). */
export interface GlobalUsageSummary {
  prompt_tokens: number;
  completion_tokens: number;
  cached_read_tokens: number;
  reasoning_tokens: number;
  cache_hit_tokens: number;
  cache_miss_tokens: number;
  tool_calls: number;
  tool_result_tokens: number;
  turns: number;
}

/** Depwork commands */
export const depworkApi = {
  createTask: (description: string, contextPaths: string[]) =>
    isTauri ? invoke<DepworkTask>("create_task", { description, contextPaths }) : Promise.resolve({ id: `mock-task-${Date.now()}`, description, status: "pending" as const, context_paths: contextPaths, created_at: new Date().toISOString() } as DepworkTask),

  listTasks: () =>
    isTauri ? invoke<DepworkTask[]>("list_tasks") : Promise.resolve(MOCK_TASKS),
};

/** PDF commands — direct extraction for the preview panel. */
export const pdfApi = {
  extractText: (path: string) =>
    isTauri ? invoke<string>("extract_pdf_text", { path }) : Promise.reject(new Error("PDF text extraction is only available in the desktop app")),
};

// ── Cloud Sync API (P0-1) ────────────────────────────────────

/** Summary of one cloud sync run. */
export interface SyncSummary {
  pushed: number;
  pulled: number;
  settings_pushed: boolean;
  /** Whether remote settings were applied back to the local config. */
  settings_applied: boolean;
}

/** Cloud sync API — push/pull sessions + settings to the backend. */
export const syncApi = {
  syncNow: (serverUrl: string, token: string): Promise<SyncSummary> =>
    isTauri
      ? invoke<SyncSummary>("sync_now", { serverUrl, token })
      : Promise.resolve({
          pushed: 0,
          pulled: 0,
          settings_pushed: false,
          settings_applied: false,
        }),
};

/** Connector commands */
export const connectorApi = {
  getConnectors: () =>
    isTauri ? invoke<Connector[]>("list_connectors") : Promise.resolve([]),

  connect: (connectorId: string) =>
    isTauri
      ? invoke<boolean>("connect_connector", { connectorId })
      : new Promise<boolean>((resolve) => setTimeout(() => resolve(true), 500)),

  installPlugin: (pluginId: string) =>
    isTauri ? invoke<void>("install_plugin", { pluginId }) : Promise.resolve(),

  togglePlugin: (pluginId: string, enabled: boolean) =>
    isTauri
      ? invoke<void>("toggle_plugin", { pluginId, enabled })
      : Promise.resolve(),

  getPluginList: () =>
    isTauri ? invoke<Plugin[]>("list_plugins") : Promise.resolve([]),
};

/** Skill commands */
export const skillsApi = {
  /** List skills; pass the active work mode ("code"|"depwork") to hide
   * skills declared for the other mode (empty = all modes). Omit for the
   * full management view. */
  list: (workMode?: string) =>
    isTauri
      ? invoke<Skill[]>("list_skills", workMode ? { workMode } : {})
      : Promise.resolve([]),
  save: (skill: Record<string, unknown>) =>
    isTauri ? invoke<void>("save_skill", { skill }) : Promise.resolve(),
  delete: (skillId: string) =>
    isTauri ? invoke<void>("delete_skill", { skillId }) : Promise.resolve(),
  /** Hot-reload skills after a compat gate toggle (no restart needed). */
  refresh: () =>
    isTauri ? invoke<void>("refresh_skills") : Promise.resolve(),
};

