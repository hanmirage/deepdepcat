/**
 * Tauri API bridge — split by domain (see index.ts for the barrel).
 * Types mirror Rust structs in src-tauri; every invoke is typed.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../core";
import type { PermissionGrant, PermissionRulesView } from "../types";
import type {
  McpServerConfig,
  McpTool,
  Memory,
  DreamResult,
  MemorySearchResult,
  PermissionDecision,
  PermissionDecisionOptions,
  PermissionMode,
} from "@/types";

// ── Permission commands ──────────────────────────────────────

/** Permission API — respond to permission requests + set mode. */
export const permissionApi = {
  /** Respond to a pending permission request. */
  respond: (
    requestId: string,
    decision: PermissionDecision,
    opts?: PermissionDecisionOptions,
  ): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("respond_permission", {
      requestId,
      decision,
      scope: opts?.scope ?? null,
      reason: opts?.reason ?? null,
    });
  },
  /** Set the permission mode for a session (per-session scope; omit the
   *  session id only for the global default). */
  setMode: (mode: PermissionMode, sessionId?: string): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("set_permission_mode", { mode, sessionId });
  },
  /** Respond to a plan-approval request ("approve" | "reject" + feedback). */
  respondPlanApproval: (
    requestId: string,
    decision: "approve" | "reject",
    feedback?: string,
  ): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("respond_plan_approval", {
      requestId,
      decision,
      feedback: feedback ?? null,
    });
  },
  /** Forget session-scoped grants for a session (session switch). */
  clearSessionGrants: (sessionId: string): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("clear_session_grants", { sessionId });
  },
  /** Read the backend permission mode (restore after restart). */
  getMode: (): Promise<PermissionMode | null> => {
    if (!isTauri) return Promise.resolve(null);
    return invoke<PermissionMode | null>("get_permission_mode");
  },
  /** Clear all durable "always allow" permission grants. */
  clearGrants: (): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("clear_permission_grants");
  },
  /** List all durable "always allow" grants (audit view). */
  listGrants: (): Promise<PermissionGrant[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<PermissionGrant[]>("list_permission_grants");
  },
  /** Revoke one durable grant by tool + pattern — immediate effect. */
  removeGrant: (toolName: string, pattern: string): Promise<boolean> => {
    if (!isTauri) return Promise.resolve(false);
    return invoke<boolean>("remove_permission_grant", { toolName, pattern });
  },
  /** Read the settings rules (allow/deny/ask). */
  getRules: (): Promise<PermissionRulesView> => {
    if (!isTauri)
      return Promise.resolve({ mode: "default", allow: [], deny: [], ask: [] });
    return invoke<PermissionRulesView>("get_permission_rules");
  },
  /** Replace the settings rules — persists config and hot-applies them. */
  setRules: (allow: string[], deny: string[], ask: string[]): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("set_permission_rules", { allow, deny, ask });
  },
  /** Snapshot of the plugin policy map. */
  listPluginPolicy: (): Promise<Record<string, string>> => {
    if (!isTauri) return Promise.resolve({});
    return invoke<Record<string, string>>("list_plugin_policy");
  },
  /** Set a plugin's policy entry ("available" | "blocked"). */
  setPluginPolicy: (pluginId: string, action: string): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("set_plugin_policy", { pluginId, action });
  },
  /** Auto-Review enable state (independent reviewer for gray-zone asks). */
  getAutoReviewEnabled: (): Promise<boolean> => {
    if (!isTauri) return Promise.resolve(false);
    return invoke<boolean>("get_auto_review_enabled");
  },
  /** Toggle Auto-Review (persisted to config.toml). */
  setAutoReviewEnabled: (enabled: boolean): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("set_auto_review_enabled", { enabled });
  },
  /** Override one Auto-Review denial: exact-action session grant (one
   *  retry semantics). */
  overrideAutoReviewDenial: (
    sessionId: string,
    toolName: string,
    args: Record<string, unknown>,
  ): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("override_auto_review_denial", {
      sessionId,
      toolName,
      args,
    });
  },
};


// ── Update commands ──────────────────────────────────────────

/** Update info returned when an update is available. */
export interface UpdateInfo {
  version: string;
  current_version: string;
  date: string | null;
  body: string | null;
  /** True → backend-only release: auto-downloads in the background and
   *  installs on app exit. False → manual download/install UI. */
  silent: boolean;
  /** Oldest supported client version. When set and the running client is
   *  older, the update is mandatory (blocking force-update screen). */
  min_version?: string | null;
  /** True when the update is mandatory (paired with min_version). */
  force?: boolean;
}

/** Download progress event payload. */
export type UpdateProgress =
  | { phase: "started" }
  | { phase: "progress"; downloaded: number; total: number | null; fraction: number }
  | { phase: "finished" }
  | { phase: "error"; message: string };

/** Update API — check for updates and install them. */
export const updateApi = {
  /** Check if an update is available. Returns `null` if up-to-date. */
  checkForUpdate: (): Promise<UpdateInfo | null> => {
    if (!isTauri) return Promise.resolve(null);
    return invoke<UpdateInfo | null>("check_for_update");
  },

  /** Download, verify, and install the update. Returns `true` if installed. */
  downloadAndInstall: (): Promise<boolean> => {
    if (!isTauri) return Promise.resolve(false);
    return invoke<boolean>("download_and_install_update");
  },

  /** Download a silent update and stage it for the next app exit.
   *  Returns the staged version, or null when nothing to do. */
  downloadSilent: (): Promise<string | null> => {
    if (!isTauri) return Promise.resolve(null);
    return invoke<string | null>("download_silent_update");
  },

  /** Version staged for exit-install, if any. */
  hasPendingSilent: (): Promise<string | null> => {
    if (!isTauri) return Promise.resolve(null);
    return invoke<string | null>("has_pending_silent_update");
  },

  /** Remove a staged silent update (e.g. after the new version launched). */
  clearPendingSilent: (): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("clear_pending_silent_update");
  },

  /** Restart the application (used after a mandatory update installs). */
  relaunch: (): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("relaunch_app");
  },
};

// ── MCP commands ─────────────────────────────────────────────

/** MCP API — manage MCP server connections and discover tools. */
export const mcpApi = {
  /** List all configured MCP servers (from backend config). */
  listServers: (): Promise<McpServerConfig[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<McpServerConfig[]>("list_mcp_servers");
  },

  /** Persist an MCP server config (add or update by name). */
  addServer: (config: McpServerConfig): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("add_mcp_server", { config });
  },

  /** Remove an MCP server from the persisted config. */
  removeServer: (name: string): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("remove_mcp_server", { name });
  },

  /** Connect to an MCP server and register its tools. */
  connect: (config: McpServerConfig): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("connect_mcp_server", { config });
  },

  /** Disconnect from a connected MCP server. */
  disconnect: (name: string): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("disconnect_mcp_server", { name });
  },

  /** Get tools from a specific connected MCP server. */
  getTools: (serverName: string): Promise<McpTool[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<McpTool[]>("get_mcp_tools", { serverName });
  },

  /** List names of all currently connected MCP servers. */
  listConnected: (): Promise<string[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<string[]>("list_connected_mcp_servers");
  },

  /** List prompts exposed by a connected MCP server. */
  listPrompts: (serverName: string): Promise<McpPrompt[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<McpPrompt[]>("list_mcp_prompts", { serverName });
  },

  /** Get a prompt template with arguments filled in. */
  getPrompt: (
    serverName: string,
    promptName: string,
    args: Record<string, unknown>,
  ): Promise<unknown> => {
    if (!isTauri) return Promise.resolve(null);
    return invoke<unknown>("call_mcp_prompt", {
      serverName,
      promptName,
      arguments: args,
    });
  },

  /** Proxy an MCP Apps view request to its server (tools/call, resources/read).
   *  Returns `{ content, isError, app? }` for tools/call and
   *  `{ text }` for resources/read. */
  proxyAppRequest: (
    server: string,
    method: string,
    params: Record<string, unknown>,
  ): Promise<unknown> => {
    if (!isTauri) return Promise.reject(new Error("mcp_app_proxy unavailable"));
    return invoke<unknown>("mcp_app_proxy", { server, method, params });
  },

  /** Save an OAuth credential for an MCP server (persisted on disk). */
  saveCredential: (
    serverName: string,
    serverUrl: string,
    accessToken: string,
    tokenType: string,
    expiresAt?: string,
    refreshToken?: string,
    tokenEndpoint?: string,
    clientId?: string,
  ): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("save_mcp_credential", {
      serverName,
      serverUrl,
      accessToken,
      tokenType,
      expiresAt: expiresAt ?? null,
      refreshToken: refreshToken ?? null,
      tokenEndpoint: tokenEndpoint ?? null,
      clientId: clientId ?? null,
    });
  },

  /** Remove a stored credential for an MCP server. */
  deleteCredential: (serverName: string, serverUrl: string): Promise<boolean> => {
    if (!isTauri) return Promise.resolve(false);
    return invoke<boolean>("delete_mcp_credential", { serverName, serverUrl });
  },

  /** List server names that have stored credentials (tokens never exposed). */
  listCredentials: (): Promise<string[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<string[]>("list_mcp_credentials");
  },

  /** Forward an MCP App log/console message to the backend event log. */
  logApp: (
    server: string,
    level: string,
    message: string,
    sessionId?: string,
  ): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("mcp_app_log", {
      server,
      level,
      message,
      sessionId: sessionId ?? null,
    });
  },
};

/** A prompt template exposed by an MCP server. */
export interface McpPrompt {
  name: string;
  description: string;
  arguments: { name: string; description: string; required: boolean }[];
}
// ── Memory commands ─────────────────────────────────────

/** Memory API — manage the persistent memory store. */export const memoryApi = {
  /** List all memories (newest first, up to `limit`). */
  listMemories: (limit?: number): Promise<Memory[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<Memory[]>("list_memories", { limit: limit ?? 100 });
  },

  /** Store a new memory (category: project/preference/fact/…). */
  storeMemory: (content: string, category: string): Promise<number> => {
    if (!isTauri) return Promise.resolve(-1);
    return invoke<number>("store_memory", { content, category });
  },

  /** Hybrid search (BM25 + vector + recency). */
  searchMemories: (query: string, limit?: number): Promise<MemorySearchResult[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<MemorySearchResult[]>("search_memories", {
      query,
      limit: limit ?? 10,
    });
  },

  /** Delete a memory by id. */
  deleteMemory: (id: number): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("delete_memory", { id });
  },

  /** Total number of stored memories. */
  getMemoryCount: (): Promise<number> => {
    if (!isTauri) return Promise.resolve(0);
    return invoke<number>("get_memory_count");
  },

  /** Run dream synthesis now (compress raw memories into knowledge). */
  triggerDream: (model?: string): Promise<DreamResult> => {
    if (!isTauri) {
      return Promise.resolve({ source_count: 0, synthesized_count: 0, summaries: [] });
    }
    return invoke<DreamResult>("trigger_dream", { model: model ?? null });
  },

  /** Dual-layer MEMORY.md status (user + project standing memory). */
  getMemoryFiles: (): Promise<MemoryFilesView> => {
    if (!isTauri) {
      return Promise.resolve({ user: { path: "", exists: false, chars: 0, entries: 0, modified_at_ms: null }, project: null });
    }
    return invoke<MemoryFilesView>("get_memory_files");
  },

  /** Dual-layer procedures.md status (user + project procedural memory). */
  getProcedureFiles: (): Promise<ProcedureFilesView> => {
    if (!isTauri) {
      return Promise.resolve({ user: { path: "", exists: false, chars: 0, entries: 0, modified_at_ms: null }, project: null });
    }
    return invoke<ProcedureFilesView>("get_procedure_files");
  },
};

/** One MEMORY.md layer (standing memory managed by memory_write). */
export interface MemoryFileInfo {
  path: string;
  exists: boolean;
  chars: number;
  entries: number;
  modified_at_ms: number | null;
}

export interface MemoryFilesView {
  user: MemoryFileInfo;
  project: MemoryFileInfo | null;
}

export type ProcedureFilesView = MemoryFilesView;

// ── Task commands ──────────────────────────────────────

/** Task row from the backend TaskManager (core/types/task.rs). */
export interface CoworkTask {
  id: string;
  description: string;
  status: string;
  context_paths: string[];
  created_at: string;
  completed_at?: string;
  session_id?: string;
}

/** Task API — the backend task pipeline (sidebar task list). */
export const taskApi = {
  /** List all tasks. */
  listTasks: (): Promise<CoworkTask[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<CoworkTask[]>("list_tasks");
  },

  /** Create a task with a description and optional context paths. */
  createTask: (description: string, contextPaths: string[]): Promise<CoworkTask> => {
    if (!isTauri) {
      return Promise.reject("Not in Tauri");
    }
    return invoke<CoworkTask>("create_task", { description, contextPaths });
  },
};

/** Compaction API — manual "compact now" for a session. */
export const compactionApi = {
  /**
   * Compact a session's conversation immediately.
   * Resolves "compacted:<tokens>" | "skipped" | "busy", or rejects on error.
   */
  forceCompact: (sessionId: string): Promise<string> => {
    if (!isTauri) return Promise.resolve("skipped");
    return invoke<string>("force_compact", { sessionId });
  },
};



