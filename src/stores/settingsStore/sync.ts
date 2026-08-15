/**
 * Settings store — split by concern (types / storage / helpers / sync).
 */


import { isTauri, configApi, diagnosticsApi } from "@/lib/tauri";
import { setClientErrorReporting } from "@/lib/clientErrorReporter";
import { logError, logWarn } from "@/lib/logger";
import type {
  ProviderConfig,
  GeneralSettings,
  VisionSettings,
  ApiFormat,
} from "./types";

/** Map frontend ProviderConfig[] → backend LlmSection.providers JSON. */
function toBackendProviders(providers: ProviderConfig[]): unknown[] {
  return providers.map((p) => {
    const envName = p.id.toUpperCase().replace(/-/g, "_") + "_API_KEY";
    // Wire protocol is DERIVED from the API format — the provider card's
    // "API 格式" picker is the single control:
    //   openai    → chat completions (backend default)
    //   anthropic → Messages API
    //   responses → OpenAI Responses API
    //   gemini/custom → backend auto-detect (non-anthropic name → chat completions)
    const protocol = p.apiFormat === "anthropic"
      ? "anthropic"
      : p.apiFormat === "responses"
        ? "responses"
        : undefined;
    return {
      name: p.id,
      api_key_env: envName,
      api_key: p.apiKey || null,
      base_url: p.baseUrl,
      enabled: p.enabled,
      // Only send when it differs from the backend's auto-detection.
      ...(protocol ? { protocol } : {}),
    };
  });
}

/** Map backend LlmSection.providers JSON → frontend ProviderConfig[]. */
export function fromBackendProviders(
  backend: { name?: string; api_key?: string | null; base_url?: string; enabled?: boolean; api_key_env?: string; protocol?: string | null }[],
  fallback: ProviderConfig[],
): ProviderConfig[] {
  if (!backend || backend.length === 0) return fallback;

  return backend.map((bp) => {
    const id = bp.name ?? "unknown";
    // Try to match with an existing frontend provider to preserve models/apiFormat
    const existing = fallback.find((fp) => fp.id === id);

    const apiFormat: ApiFormat =
      id === "anthropic" ? "anthropic"
      : id === "gemini" ? "gemini"
      : bp.protocol === "responses" ? "responses"
      : "openai";

    return {
      id,
      name: existing?.name ?? id.charAt(0).toUpperCase() + id.slice(1),
      baseUrl: bp.base_url ?? existing?.baseUrl ?? "",
      apiKey: bp.api_key ?? "",
      apiFormat: existing?.apiFormat ?? apiFormat,
      models: existing?.models ?? [],
      enabled: bp.enabled ?? true,
    };
  });
}

/** Sync current providers + vision config to the backend AppConfig. */
export async function syncToBackend(
  providers: ProviderConfig[],
  general: GeneralSettings,
  vision?: VisionSettings,
): Promise<void> {
  if (!isTauri) return;
  try {
    // Get current backend config to preserve non-LLM sections
    const current = await configApi.getConfig();
    // Patch the llm.providers section
    (current as Record<string, unknown>).llm = {
      ...((current as Record<string, unknown>).llm as Record<string, unknown>),
      providers: toBackendProviders(providers),
    };
    // Patch the [agent] section — DeepSeek optimization master switch
    // (cache-aware compaction for DeepSeek sessions).
    (current as Record<string, unknown>).agent = {
      ...((current as Record<string, unknown>).agent as Record<string, unknown>),
      deepseek_auto_reasoning: general.deepseekAutoReasoning,
    };
    // Patch the [vision] section (self-contained vision model config).
    // api_key is sent as an empty string (never null) — the backend
    // VisionSection.api_key is a plain String, and null would fail
    // deserialization and reject the ENTIRE config update.
    if (vision) {
      (current as Record<string, unknown>).vision = {
        enabled: vision.enabled,
        base_url: vision.baseUrl,
        api_key: vision.apiKey ?? "",
        model: vision.model,
      };
    }
    await configApi.updateConfig(current as Record<string, unknown>);
    // A successful write clears any previously surfaced sync error.
    (await getStore()).setState({ lastSyncError: null });
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    logError("settingsStore", "Failed to sync to backend:", e);
    // Surface the failure in the settings UI (see lastSyncError).
    (await getStore()).setState({ lastSyncError: msg });
  }
}

let syncTimer: ReturnType<typeof setTimeout> | null = null;

/**
 * Debounced full-settings sync. Every mutation persists to localStorage
 * immediately (crash-safe) but pushes to the backend at most once per 500ms
 * window — typing a URL or API key would otherwise fire one config update
 * per keystroke (and each update rewrites the whole config). At flush time
 * the LATEST store state is pushed, so rapid consecutive edits collapse
 * into a single write.
 */
export function scheduleBackendSync(): void {
  if (syncTimer !== null) clearTimeout(syncTimer);
  syncTimer = setTimeout(() => {
    syncTimer = null;
    void (async () => {
      const store = await getStore();
      const { providers, general, vision } = store.getState();
      await syncToBackend(providers, general, vision);
    })();
  }, 500);
}

/** Cancel any pending debounced sync (used by saveAll for an immediate flush). */
export function clearPendingSync(): void {
  if (syncTimer !== null) {
    clearTimeout(syncTimer);
    syncTimer = null;
  }
}

/**
 * Apply the persisted diagnostics toggle to the backend reporter.
 *
 * The Rust side keeps its own AtomicBool (default ON). Without this, a user
 * who turned diagnostics off would see it silently re-enable on the next
 * launch — the settings UI would show "off" but the backend would still send.
 * This is called on every app startup with the persisted localStorage value.
 */
export async function syncDiagnosticsToBackend(enabled: boolean): Promise<void> {
  setClientErrorReporting(enabled);
  if (!isTauri) return;
  try {
    await diagnosticsApi.setEnabled(enabled);
  } catch (e) {
    logWarn("settingsStore", "Failed to sync diagnostics toggle:", e);
  }
}

// ── Store ────────────────────────────────────────────────────

/** Late-bound store access — sync.ts is imported by the store, so the store
 *  must be imported lazily here (avoids a module-init cycle). */
async function getStore() {
  const m = await import("./settingsStore");
  return m.useSettingsStore;
}
