/**
 * Tauri API bridge — split by domain (see index.ts for the barrel).
 * Types mirror Rust structs in src-tauri; every invoke is typed.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../core";

// ── Auth commands ──────────────────────────────────────

/** User info from website. */
export interface UserInfo {
  id: string;
  email: string;
  name: string;
  is_admin?: boolean;
  created_at?: string;
}

/** Auth response from website (legacy). */
export interface AuthResponse {
  user: UserInfo;
  token: string;
  message?: string;
}

/** Token response from the website password login. */
export interface DeviceTokenResponse {
  access_token: string;
  token_type: string;
  expires_in: number;
  user_id: string;
  username: string;
  /** Account avatar URL from the website ("" when unset). */
  avatar?: string;
}

/** Token verification response. */
export interface VerifyTokenResponse {
  valid: boolean;
  user_id: string | null;
  expires_at: number | null;
  /** Current avatar URL ("" when unset) — refreshed on startup verify. */
  avatar?: string | null;
}

/** User info attached to auth state. */
export interface AuthUserInfo {
  username: string;
  user_id: string | null;
  /** Account avatar URL from the website ("" when unset). */
  avatar?: string | null;
}

/** Auth API — direct email+password login + session persistence. */
export const deviceAuthApi = {
  /** Direct email+password login against the website account system.
   *  Returns a DeviceTokenResponse whose access_token is the website JWT. */
  loginWithPassword: (serverUrl: string, email: string, password: string): Promise<DeviceTokenResponse> => {
    if (!isTauri) return Promise.reject("Not in Tauri");
    return invoke<DeviceTokenResponse>("login_with_password", { args: { serverUrl, email, password } });
  },

  /** Verify if a token is still valid. */
  verifyToken: (serverUrl: string, token: string): Promise<VerifyTokenResponse> => {
    if (!isTauri) return Promise.reject("Not in Tauri");
    return invoke<VerifyTokenResponse>("verify_token", { args: { serverUrl, token } });
  },

  /** Revoke a token (logout). */
  revokeToken: (serverUrl: string, token: string): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("revoke_token", { args: { serverUrl, token } });
  },

  /** Get the default server URL. */
  getDefaultServerUrl: (): Promise<string> => {
    if (!isTauri) return Promise.resolve("https://deepdepcat.hsmiai.xyz");
    return invoke<string>("get_default_server_url");
  },

  /** Update the display name on the website account (cloud sync). */
  updateUserProfile: (serverUrl: string, token: string, name: string): Promise<string> => {
    if (!isTauri) return Promise.reject("Not in Tauri");
    return invoke<string>("update_user_profile", { args: { serverUrl, token, name } });
  },

  /** Upload an avatar image to the website account (cloud sync). Returns the new avatar path. */
  uploadAvatar: (serverUrl: string, token: string, filePath: string): Promise<string> => {
    if (!isTauri) return Promise.reject("Not in Tauri");
    return invoke<string>("upload_avatar", { args: { serverUrl, token, filePath } });
  },
};

/** Legacy localStorage key — kept for one-time migration to the keyring. */
export const TOKEN_STORAGE_KEY = "deepdepcat.auth.token";

/**
 * OS-keyring token persistence (Tauri only).
 *
 * The website access token must never sit in localStorage — any renderer
 * XSS could read it. In the desktop app it lives in the OS credential store
 * (Windows Credential Manager / macOS Keychain / Linux Secret Service).
 * Browser dev mode falls back to localStorage so the mock flow still works.
 */
export const authKeyringApi = {
  storeToken: (token: string): Promise<void> => {
    if (!isTauri) {
      try {
        localStorage.setItem(TOKEN_STORAGE_KEY, token);
      } catch {
        // storage unavailable — auth simply won't persist in browser mode
      }
      return Promise.resolve();
    }
    return invoke<void>("auth_store_token", { token });
  },
  loadToken: (): Promise<string | null> => {
    if (!isTauri) {
      try {
        return Promise.resolve(localStorage.getItem(TOKEN_STORAGE_KEY));
      } catch {
        return Promise.resolve(null);
      }
    }
    return invoke<string | null>("auth_load_token");
  },
  deleteToken: (): Promise<void> => {
    if (!isTauri) {
      try {
        localStorage.removeItem(TOKEN_STORAGE_KEY);
      } catch {
        // ignore
      }
      return Promise.resolve();
    }
    return invoke<void>("auth_delete_token");
  },
};

/**
 * Registration API — two-step account creation against the website
 * (send verification code → verify email). Runs through the Rust side
 * (the website API has no CORS).
 */
export const registerApi = {
  /** Step 1 — request the email verification code. */
  sendCode: (serverUrl: string, email: string, name: string, password: string): Promise<string> => {
    if (!isTauri) return Promise.reject("Not in Tauri");
    return invoke<string>("register_send_code", {
      args: { serverUrl, email, name, password },
    });
  },

  /** Step 2 — verify the code and create the account. */
  verifyEmail: (
    serverUrl: string,
    email: string,
    code: string,
  ): Promise<{ success: boolean; message?: string }> => {
    if (!isTauri) return Promise.reject("Not in Tauri");
    return invoke<{ success: boolean; message?: string }>("register_verify_email", {
      args: { serverUrl, email, code },
    });
  },
};

/** A single release entry from the website changelog. */
export interface ChangelogEntry {
  version: string;
  date: string;
  title: string;
  tag: string;
  items: string[];
}

/** Raw `/api/updates/changelog` payload. */
export interface ChangelogResponse {
  updates: ChangelogEntry[];
}

/** Raw `/api/site-config` payload (latest version, download links, contact). */
export interface AnnouncementConfig {
  id: string;
  enabled: boolean;
  title: string;
  message: string;
  level: "info" | "warning" | "critical";
}

export interface SiteConfig {
  siteUrl: string;
  githubUrl: string;
  githubIssuesUrl: string;
  contactEmail: string;
  latestVersion: string;
  latestDate: string;
  downloads?: Record<string, { url: string; label: string; enabled: boolean; soon: boolean }>;
  announcement?: AnnouncementConfig;
  [key: string]: unknown;
}

/**
 * Cloud content API — public website endpoints. The website has no CORS
 * configuration, so all calls go through the Rust side (native HTTP).
 */
export const cloudApi = {
  /** Submit like/dislike feedback (rating 1-5; category per the website). */
  submitFeedback: (
    serverUrl: string,
    rating: number,
    message: string,
    category: "bug" | "feature" | "general" | "praise" | "subscribe",
  ): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("submit_feedback", {
      args: { serverUrl, rating, message, category },
    });
  },

  /** Fetch the release changelog. */
  fetchChangelog: (serverUrl: string): Promise<ChangelogResponse> => {
    if (!isTauri) return Promise.resolve({ updates: [] });
    return invoke<ChangelogResponse>("fetch_changelog", { serverUrl });
  },

  /** Fetch the site-config (latest version, download links, contact). */
  fetchSiteConfig: (serverUrl: string): Promise<SiteConfig> => {
    if (!isTauri) return Promise.resolve({} as SiteConfig);
    return invoke<SiteConfig>("fetch_site_config", { serverUrl });
  },
};


// ── Circuit Breaker API ─────────────────────────────────────

/** Circuit breaker state for a single provider. */
export interface CircuitBreakerState {
  provider: string;
  state: "closed" | "open" | "half_open";
  consecutive_failures: number;
}

/** Circuit Breaker API — per-provider failure tracking and reset. */
export const circuitBreakerApi = {
  /** Get the circuit breaker state for all providers. */
  getStates: (): Promise<CircuitBreakerState[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<CircuitBreakerState[]>('get_circuit_breaker_states');
  },

  /** Manually reset a provider's circuit breaker. */
  reset: (provider: string): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>('reset_circuit_breaker', { provider });
  },
};

// ── Hook API ────────────────────────────────────────────────

/** A hook definition (mirrors Rust HookDefinition — "type" is the serialized hook_type). */
export interface HookDefinition {
  event: string;
  type: string;
  command?: string | null;
  prompt?: string | null;
  url?: string | null;
  condition?: string | null;
  timeout_ms?: number | null;
  shell?: string | null;
  enabled: boolean;
}

/** A hook definition plus its trust status (Rust HookView). */
export interface HookView extends HookDefinition {
  trusted: boolean;
  fingerprint: string;
}

/** Hook API — list/save/delete hook definitions persisted in hooks.toml. */
export const hookApi = {
  list: (): Promise<HookView[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<HookView[]>("list_hooks");
  },
  save: (hook: HookDefinition): Promise<number> => {
    if (!isTauri) return Promise.resolve(0);
    return invoke<number>("save_hook", { hook });
  },
  delete: (event: string, hookType: string, content: string): Promise<number> => {
    if (!isTauri) return Promise.resolve(0);
    return invoke<number>("delete_hook", { event, hookType, content });
  },
  /** Trust a hook by its current content fingerprint (persisted). */
  trust: (fingerprint: string): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("trust_hook", { fingerprint });
  },
  /** Revoke hook trust — the hook stops running until trusted again. */
  untrust: (fingerprint: string): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("untrust_hook", { fingerprint });
  },
  listEvents: (): Promise<string[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<string[]>("list_hook_events");
  },
  /** Expand env vars + redact secrets for UI preview (never executes). */
  preview: (hook: HookDefinition): Promise<HookPreview> => {
    if (!isTauri) return Promise.resolve({ command: null, prompt: null, url: null });
    return invoke<HookPreview>("preview_hook", { hook });
  },
  /** Audit view of project-level hooks (`.deepdepcat/hooks.toml`). They are
   *  read-only here — the master switch below is the only control. */
  listProjectHooks: (): Promise<HookView[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<HookView[]>("list_project_hooks");
  },
  /** Whether project hooks are enabled (default: false — a cloned repo
   *  must not execute arbitrary commands without an explicit opt-in). */
  getProjectHooksEnabled: (): Promise<boolean> => {
    if (!isTauri) return Promise.resolve(false);
    return invoke<boolean>("get_project_hooks_enabled");
  },
  setProjectHooksEnabled: (enabled: boolean): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("set_project_hooks_enabled", { enabled });
  },
};

/** Expanded, redacted preview of a hook's executable fields. */
export interface HookPreview {
  command: string | null;
  prompt: string | null;
  url: string | null;
}

// ── Agent Activity API ───────────────────────────────────────

/** Lifecycle state of a spawned subagent worker. */
export interface WorkerState {
  worker_id: string;
  task: string;
  /** Subagent type label ("explore" | "plan" | "general" | custom name). */
  agent_type: string;
  status: "pending" | "running" | "completed" | "failed" | "stopped";
  phase: "research" | "synthesis" | "implementation" | "verification";
  result: string | null;
  /** The parent session that spawned this worker (absent for legacy). */
  session_id?: string | null;
  /** Unix millis when the worker was registered — drives the elapsed-time display. */
  started_at_ms: number;
  /** Unix millis when the worker reached a terminal state (0 / absent = active). */
  ended_at_ms?: number;
}

/** A background task (bash background:true). */
export interface BackgroundTaskInfo {
  id: string;
  command: string;
  pid: number;
  started_at_ms: number;
  status: string;
  session_id: string;
  output_file: string | null;
  exit_code: number | null;
}

/** A chunk of background task output. */
export interface TaskOutputChunk {
  content: string;
  offset: number;
  done: boolean;
}

/** A parsed agent definition (built-in / user / project .deepdepcat/agents). */
export interface AgentDefinition {
  id: string;
  name: string;
  description: string;
  prompt_mode: "extend" | "full";
  model?: string;
  allowed_tools: string[];
  work_modes: string[];
  is_builtin: boolean;
}

/** Agent activity API — subagent workers + background tasks. */
export const agentApi = {
  /** List agent definitions for a work mode ("code" | "depwork"). */
  listDefinitions: (workMode?: string): Promise<AgentDefinition[]> =>
    isTauri
      ? invoke<AgentDefinition[]>("list_agent_definitions", { workMode: workMode ?? null })
      : Promise.resolve([]),

  listActiveWorkers: (): Promise<WorkerState[]> =>
    isTauri ? invoke<WorkerState[]>("list_active_workers") : Promise.resolve([]),

  listBackgroundTasks: (sessionId: string): Promise<BackgroundTaskInfo[]> =>
    isTauri ? invoke<BackgroundTaskInfo[]>("list_background_tasks", { sessionId }) : Promise.resolve([]),

  readTaskOutput: (taskId: string, offset: number, maxBytes: number): Promise<TaskOutputChunk | null> =>
    isTauri ? invoke<TaskOutputChunk | null>("read_task_output", { taskId, offset, maxBytes }) : Promise.resolve(null),

  killBackgroundTask: (taskId: string): Promise<boolean> =>
    isTauri ? invoke<boolean>("kill_background_task", { taskId }) : Promise.resolve(false),
};

// ── Feature Flag API ─────────────────────────────────────────

/** A remote/local feature flag. */
export interface FeatureFlag {
  key: string;
  enabled: boolean;
  rollout_percent: number;
  description: string;
}

/** Feature Flag API — list and toggle feature flags. */
export const featureFlagApi = {
  list: (): Promise<FeatureFlag[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<FeatureFlag[]>("get_feature_flags");
  },
  set: (key: string, enabled: boolean): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("set_feature_flag", { key, enabled });
  },
};

// ── Crash Report API ─────────────────────────────────────────
/** Metadata for a crash report file. */
export interface CrashReportInfo {
  filename: string;
  timestamp: string;
  file_size: number;
}

/** Structured crash payload read by the crash dialog on startup. */
export interface PendingCrash {
  client_id: string;
  app_version: string;
  os: string;
  arch: string;
  pid: number;
  panic_message: string;
  backtrace: string;
  timestamp: string;
}

/** Result of submitting a crash report to the server. */
export interface CrashSubmitResult {
  status: string;
  crash_id: number | null;
}

/** Crash Report API — list/read/delete captured panic reports + upload. */
export const crashApi = {
  list: (): Promise<CrashReportInfo[]> => {
    if (!isTauri) return Promise.resolve([]);
    return invoke<CrashReportInfo[]>("list_crash_reports");
  },
  read: (filename: string): Promise<string | null> => {
    if (!isTauri) return Promise.resolve(null);
    return invoke<string | null>("read_crash_report", { filename });
  },
  delete: (filename: string): Promise<boolean> => {
    if (!isTauri) return Promise.resolve(false);
    return invoke<boolean>("delete_crash_report", { filename });
  },
  getPending: (): Promise<PendingCrash | null> => {
    if (!isTauri) return Promise.resolve(null);
    return invoke<PendingCrash | null>("get_pending_crash");
  },
  dismissPending: (): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("dismiss_pending_crash");
  },
  exportSessionConversation: (sessionId: string): Promise<string> => {
    if (!isTauri) return Promise.resolve("[]");
    return invoke<string>("export_session_conversation", { sessionId });
  },
  submit: (
    serverUrl: string,
    includeConversation: boolean,
    conversationJson: string | null,
  ): Promise<CrashSubmitResult> => {
    if (!isTauri)
      return Promise.resolve({ status: "accepted", crash_id: null });
    return invoke<CrashSubmitResult>("submit_crash_report", {
      serverUrl,
      includeConversation,
      conversationJson,
    });
  },
};

/** Diagnostics API — anonymous error-telemetry toggle (Settings → Privacy). */
export const diagnosticsApi = {
  getEnabled: (): Promise<boolean> => {
    if (!isTauri) return Promise.resolve(true);
    return invoke<boolean>("get_diagnostics_enabled");
  },
  setEnabled: (enabled: boolean): Promise<void> => {
    if (!isTauri) return Promise.resolve();
    return invoke<void>("set_diagnostics_enabled", { enabled });
  },
};

// ── Ask User API ────────────────────────────────────────────
/** Payload of the "ask-user" event — emitted when the agent calls ask_user. */
export interface UserAskRequest {
  request_id: string;
  session_id: string;
  question: string;
  options: string[];
}

/** Ask User API — respond to a pending ask_user tool request. */
export const askUserApi = {
  /** Send the user's reply back to the waiting ask_user tool. */
  respond: (requestId: string, response: string): Promise<boolean> => {
    if (!isTauri) return Promise.resolve(false);
    return invoke<boolean>("respond_to_user_input", { requestId, response });
  },
};

// ── MCP Elicitation API ─────────────────────────────────────

/** MCP Elicitation API — respond to server-initiated input requests. */
export const elicitationApi = {
  /** Respond to a pending elicitation request. */
  respond: (
    elicitationId: string,
    action: "accept" | "decline" | "cancel",
    content?: unknown,
  ): Promise<boolean> => {
    if (!isTauri) return Promise.resolve(false);
    return invoke<boolean>('respond_elicitation', {
      elicitationId,
      action,
      content: content ?? null,
    });
  },
};

/** Tool description from Rust (reserved for future tool-system UI). */
export interface ToolDescription {
  id: { id: string };
  name: string;
  description: string;
  parameters: unknown;
}

