/**
 * Settings store — split by concern (types / storage / helpers / sync).
 */


export type ApiFormat = "openai" | "anthropic" | "gemini" | "responses" | "custom";
export interface ModelConfig {
  id: string;
  name: string;
  contextWindow: number;
}

export interface ProviderConfig {
  id: string;
  name: string;
  baseUrl: string;
  apiKey: string;
  apiFormat: ApiFormat;
  models: ModelConfig[];
  enabled: boolean;
}

/**
 * Vision model config for the `visual_describe` tool. Self-contained — the
 * user fills base_url/api_key/model directly (the main chat model may be
 * text-only, so the vision model is a separate endpoint).
 */
export interface VisionSettings {
  enabled: boolean;
  baseUrl: string;
  apiKey: string;
  model: string;
}

export type TitleBarStyle = "mac" | "windows";

export interface GeneralSettings {
language: "en" | "zh";
proxyUrl: string;
noProxyList: string;
showThinking: boolean;
showTodo: boolean;
/** Streaming display pacing — "smooth" types text out, "instant" reveals
 *  each arriving batch immediately (backend pacing is bypassed too). */
streamingSpeed: "smooth" | "instant";
titleBarStyle: TitleBarStyle;
/** Remote server URL for auth and cloud sync. */
serverUrl: string;
/** DeepSeek optimization master switch — enables cache-aware summarization
 *  (summary calls reuse the session's prompt prefix) and effort auto-tiering
 *  for DeepSeek sessions. Off → thinking fixed at "high". */
deepseekAutoReasoning: boolean;
/** Session-level total token limit (0 = unlimited). */
sessionTokenLimit: number;
/** Session-level total cost limit in USD (0 = unlimited). */
sessionCostLimit: number;
/** Anonymous diagnostics (tool-error telemetry) — opt-out, default on. */
diagnosticsEnabled: boolean;
}

// ── Storage helpers ──────────────────────────────────────────

export interface SettingsState {
  providers: ProviderConfig[];
  general: GeneralSettings;
  vision: VisionSettings;
  activeProviderId: string | null;
  loaded: boolean;
  /** Last backend sync failure message (null when the last sync succeeded). */
  lastSyncError: string | null;
  /** Clear the surfaced sync error (e.g. user dismissed it). */
  clearSyncError: () => void;
  /** Ecosystem skill compat gates (mirror of the `[skills]` config section). */
  skillsCompat: { claudeEnabled: boolean; cursorEnabled: boolean };

  // ── Actions ────────────────────────────────────────────────
  init: () => Promise<void>;
  /** Toggle a compat gate: patch backend config + hot-reload skills. */
  setSkillsCompat: (key: "claudeEnabled" | "cursorEnabled", enabled: boolean) => Promise<void>;
  addProvider: (config: Omit<ProviderConfig, "id" | "models" | "enabled">, models?: ModelConfig[]) => void;
  updateProvider: (id: string, patch: Partial<ProviderConfig>) => void;
  removeProvider: (id: string) => void;
  toggleProvider: (id: string, enabled: boolean) => void;
  addModel: (providerId: string, model: Omit<ModelConfig, "id"> & { id?: string }) => void;
  removeModel: (providerId: string, modelId: string) => void;
  updateModel: (
    providerId: string,
    modelId: string,
    patch: Partial<Pick<ModelConfig, "name" | "contextWindow">>,
  ) => void;
  updateGeneral: (patch: Partial<GeneralSettings>) => void;
  updateVision: (patch: Partial<VisionSettings>) => void;
  setActiveProvider: (id: string | null) => void;
  fetchModels: (providerId: string) => Promise<{ success: boolean; count: number; error?: string }>;
  fetchModelsByConfig: (
    baseUrl: string,
    apiKey: string,
    apiFormat: ApiFormat,
  ) => Promise<{ success: boolean; models: ModelConfig[]; error?: string }>;

  // ── Right panel actions ───────────────────────────────────
  /** Force-sync all settings to backend. */
  saveAll: () => Promise<void>;
  /** Reset all settings to defaults. */
  resetAll: () => void;
}
