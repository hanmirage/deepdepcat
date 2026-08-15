/**
 * Settings store — split by concern (types / storage / helpers / sync).
 */


import { create } from "zustand";
import { logError } from "@/lib/logger";
import { isTauri, configApi, skillsApi, modelsApi } from "@/lib/tauri";
import i18n from "@/i18n";
import type {
  SettingsState,
  GeneralSettings,
  VisionSettings,
  ProviderConfig,
  ModelConfig,
} from "./types";
import {
  loadFromStorage,
  saveToStorage,
  scrubStoredSecrets,
  defaultGeneral,
  defaultVision,
  defaultProviders,
} from "./storage";
import {
  knownContextWindow,
  healKnownContexts,
  parseModelListPayload,
  type ModelListPayload,
} from "./helpers";
import {
  syncToBackend,
  scheduleBackendSync,
  clearPendingSync,
  syncDiagnosticsToBackend,
  fromBackendProviders,
} from "./sync";

export const useSettingsStore = create<SettingsState>((set, get) => ({
  providers: healKnownContexts(loadFromStorage().providers),
  general: loadFromStorage().general,
  vision: defaultVision(),
  activeProviderId: null,
  loaded: false,
  lastSyncError: null,
  /** Ecosystem skill compat gates (mirror of the `[skills]` config section). */
  skillsCompat: { claudeEnabled: true, cursorEnabled: true },

  init: async () => {
    if (get().loaded) return;
    // Remove secrets an older build may have persisted (Tauri only).
    scrubStoredSecrets();

    // Apply the persisted language before the UI renders, so the app doesn't
    // always boot in Chinese even when the user chose English.
    const applyLanguage = (general: GeneralSettings) => {
      if (general.language && general.language !== i18n.language) {
        void i18n.changeLanguage(general.language);
      }
    };

    // Backend vision config (fallback for the localStorage path below).
    let backendVision: VisionSettings | null = null;

    // Try loading from backend first
    if (isTauri) {
      try {
        const config = await configApi.getConfig();
        const llm = (config as Record<string, unknown>).llm as
          | { providers?: unknown[] }
          | undefined;
        const backendProviders = (llm?.providers ?? []) as {
          name?: string;
          api_key?: string | null;
          base_url?: string;
          enabled?: boolean;
        }[];

        // Skill compat gates live in the `[skills]` config section.
        const skills = (config as Record<string, unknown>).skills as
          | { claude_enabled?: boolean; cursor_enabled?: boolean }
          | undefined;
        if (skills) {
          set({
            skillsCompat: {
              claudeEnabled: skills.claude_enabled ?? true,
              cursorEnabled: skills.cursor_enabled ?? true,
            },
          });
        }

        // Vision model config lives in the `[vision]` config section.
        const vision = (config as Record<string, unknown>).vision as
          | { enabled?: boolean; base_url?: string; api_key?: string | null; model?: string }
          | undefined;
        if (vision) {
          backendVision = {
            enabled: vision.enabled ?? false,
            baseUrl: vision.base_url ?? defaultVision().baseUrl,
            apiKey: vision.api_key ?? "",
            model: vision.model ?? defaultVision().model,
          };
        }

        if (backendProviders.length > 0) {
          const local = loadFromStorage();
          const providers = fromBackendProviders(backendProviders, local.providers);
          // Merge: if backend has a key but local doesn't, use backend's
          for (const p of providers) {
            const localP = local.providers.find((lp) => lp.id === p.id);
            if (!p.apiKey && localP?.apiKey) {
              p.apiKey = localP.apiKey;
            }
            // Preserve models from localStorage if backend doesn't have them
            if (p.models.length === 0 && localP?.models?.length) {
              p.models = localP.models;
            }
          }
          const healed = healKnownContexts(providers);
          const effectiveVision = backendVision ?? local.vision;
          saveToStorage(healed, local.general, effectiveVision);
          applyLanguage(local.general);
          set({
            providers: healed,
            general: local.general,
            vision: effectiveVision,
            loaded: true,
            activeProviderId: healed[0]?.id ?? null,
          });
          // Apply the persisted diagnostics toggle to the backend so a user
          // who opted out stays opted out across restarts (P0 privacy fix).
          void syncDiagnosticsToBackend(local.general.diagnosticsEnabled);
          return;
        }
      } catch {
        // Backend not available — fall through to localStorage
      }
    }

    // Fallback: localStorage
    const { providers, general, vision: storedVision } = loadFromStorage();
    const healed = healKnownContexts(providers);
    const effectiveVision = backendVision ?? storedVision;
    saveToStorage(healed, general, effectiveVision);
    applyLanguage(general);
    set({
      providers: healed,
      general,
      vision: effectiveVision,
      loaded: true,
      activeProviderId: providers[0]?.id ?? null,
    });
    // Apply the persisted diagnostics toggle to the backend (P0 privacy fix).
    void syncDiagnosticsToBackend(general.diagnosticsEnabled);
  },

  addProvider: (config, models) => {
    const id = `provider-${Date.now()}`;
    const newProvider: ProviderConfig = {
      ...config,
      id,
      models: models ?? [],
      enabled: true,
    };
    set((s) => {
      const providers = [...s.providers, newProvider];
      saveToStorage(providers, s.general, s.vision);
      scheduleBackendSync();
      return { providers };
    });
  },

  updateProvider: (id, patch) => {
    set((s) => {
      const providers = s.providers.map((p) =>
        p.id === id ? { ...p, ...patch } : p,
      );
      saveToStorage(providers, s.general, s.vision);
      scheduleBackendSync();
      return { providers };
    });
  },

  removeProvider: (id) => {
    set((s) => {
      const providers = s.providers.filter((p) => p.id !== id);
      saveToStorage(providers, s.general, s.vision);
      scheduleBackendSync();
      return {
        providers,
        activeProviderId: s.activeProviderId === id ? providers[0]?.id ?? null : s.activeProviderId,
      };
    });
  },

  toggleProvider: (id, enabled) => {
    get().updateProvider(id, { enabled });
  },

  addModel: (providerId, model) => {
    // The model id is the API model name sent in requests — prefer the
    // caller-provided id (manual add passes the typed API model name),
    // fall back to a synthetic id only for name-only stubs.
    const modelId = model.id?.trim() ? model.id.trim() : `model-${Date.now()}`;
    set((s) => {
      const providers = s.providers.map((p) =>
        p.id === providerId
          ? { ...p, models: [...p.models, { ...model, id: modelId }] }
          : p,
      );
      saveToStorage(providers, s.general, s.vision);
      scheduleBackendSync();
      return { providers };
    });
  },

  removeModel: (providerId, modelId) => {
    set((s) => {
      const providers = s.providers.map((p) =>
        p.id === providerId
          ? { ...p, models: p.models.filter((m) => m.id !== modelId) }
          : p,
      );
      saveToStorage(providers, s.general, s.vision);
      scheduleBackendSync();
      return { providers };
    });
  },

  updateModel: (providerId, modelId, patch) => {
    set((s) => {
      const providers = s.providers.map((p) =>
        p.id === providerId
          ? {
              ...p,
              models: p.models.map((m) => (m.id === modelId ? { ...m, ...patch } : m)),
            }
          : p,
      );
      saveToStorage(providers, s.general, s.vision);
      scheduleBackendSync();
      return { providers };
    });
  },

  updateGeneral: (patch) => {
    if (patch.language) {
      void i18n.changeLanguage(patch.language);
    }
    set((s) => {
      const general = { ...s.general, ...patch };
      saveToStorage(s.providers, general, s.vision);
      scheduleBackendSync();
      return { general };
    });
  },

  updateVision: (patch) => {
    set((s) => {
      const vision = { ...s.vision, ...patch };
      saveToStorage(s.providers, s.general, vision);
      scheduleBackendSync();
      return { vision };
    });
  },

  setActiveProvider: (id) => set({ activeProviderId: id }),

  clearSyncError: () => set({ lastSyncError: null }),

  fetchModelsByConfig: async (baseUrl, apiKey, apiFormat) => {
    try {
      const trimmedUrl = baseUrl.replace(/\/+$/, "");

      // Tauri: native HTTP (provider APIs don't send CORS headers to the
      // webview, so the browser fetch below only exists for dev mode).
      let payload: ModelListPayload;
      if (isTauri) {
        payload = (await modelsApi.fetchProviderModels(baseUrl, apiKey, apiFormat)) as ModelListPayload;
      } else {
        // Build the models endpoint URL
        // OpenAI-compatible: GET {baseUrl}/models
        // Anthropic: GET {baseUrl}/v1/models (with x-api-key header)
        // Gemini: GET {baseUrl}/v1beta/models?key={apiKey}
        let url: string;
        const headers: Record<string, string> = { "Content-Type": "application/json" };

        if (apiFormat === "anthropic") {
          url = `${trimmedUrl}/v1/models`;
          headers["x-api-key"] = apiKey;
          headers["anthropic-version"] = "2023-06-01";
        } else if (apiFormat === "gemini") {
          url = `${trimmedUrl}/v1beta/models?key=${encodeURIComponent(apiKey)}`;
        } else {
          // OpenAI-compatible (openai / responses / custom)
          url = `${trimmedUrl}/models`;
          if (apiKey) {
            headers["Authorization"] = `Bearer ${apiKey}`;
          }
        }

        const res = await fetch(url, { method: "GET", headers });

        if (!res.ok) {
          const errText = await res.text().catch(() => "");
          return {
            success: false,
            models: [],
            error: `HTTP ${res.status}: ${errText || res.statusText}`,
          };
        }

        payload = (await res.json()) as ModelListPayload;
      }

      // Parse models from different API response formats
      const rawModels = parseModelListPayload(payload, apiFormat);

      // Filter out empty IDs and deduplicate
      const seen = new Set<string>();
      const models: ModelConfig[] = [];
      for (const m of rawModels) {
        if (!m.id || seen.has(m.id)) continue;
        seen.add(m.id);
        models.push({
          // CRITICAL: the id IS the API model name (e.g. "deepseek-v4-flash")
          // — it is sent verbatim as `model` in requests. Never substitute a
          // synthetic id here, or providers reject it (DeepSeek HTTP 400:
          // "The supported API model names are ...").
          id: m.id,
          name: m.name || m.id,
          contextWindow: m.context_window ?? knownContextWindow(m.id) ?? 32000,
        });
      }

      return { success: true, models, error: undefined };
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      return { success: false, models: [], error: msg };
    }
  },

  fetchModels: async (providerId) => {
    const state = get();
    const provider = state.providers.find((p) => p.id === providerId);
    if (!provider) {
      return { success: false, count: 0, error: "PROVIDER_NOT_FOUND" };
    }
    if (!provider.baseUrl) {
      return { success: false, count: 0, error: "MISSING_BASE_URL" };
    }

    const result = await get().fetchModelsByConfig(
      provider.baseUrl,
      provider.apiKey,
      provider.apiFormat,
    );

    if (!result.success || result.models.length === 0) {
      return {
        success: false,
        count: 0,
        error: result.error ?? "NO_MODELS_FETCHED",
      };
    }

    // Replace the provider's model list with fetched models
    set((s) => {
      const providers = s.providers.map((p) =>
        p.id === providerId ? { ...p, models: result.models } : p,
      );
      saveToStorage(providers, s.general, s.vision);
      scheduleBackendSync();
      return { providers };
    });

    return { success: true, count: result.models.length, error: undefined };
  },

  // ── Right panel actions ───────────────────────────────────
  saveAll: async () => {
    // Flush any pending debounced write so the immediate sync below is the
    // only backend write in flight.
    clearPendingSync();
    const { providers, general, vision } = get();
    await syncToBackend(providers, general, vision);
    saveToStorage(providers, general, vision);
  },

  setSkillsCompat: async (key, enabled) => {
    set((s) => ({ skillsCompat: { ...s.skillsCompat, [key]: enabled } }));
    if (!isTauri) return;
    try {
      const current = await configApi.getConfig();
      const skills = {
        ...((current as Record<string, unknown>).skills as Record<string, unknown> | undefined),
        [key === "claudeEnabled" ? "claude_enabled" : "cursor_enabled"]: enabled,
      };
      (current as Record<string, unknown>).skills = skills;
      await configApi.updateConfig(current as Record<string, unknown>);
      await skillsApi.refresh();
    } catch (e) {
      logError("settingsStore", "Failed to sync skills compat:", e);
      // Roll back the optimistic toggle on failure.
      set((s) => ({ skillsCompat: { ...s.skillsCompat, [key]: !enabled } }));
    }
  },

  resetAll: () => {
    const providers = defaultProviders();
    const general = defaultGeneral();
    const vision = defaultVision();
    set({ providers, general, vision, activeProviderId: providers[0]?.id ?? null });
    saveToStorage(providers, general, vision);
    // Sync restored defaults to backend (fire-and-forget, like other mutations)
    void syncToBackend(providers, general, vision);
  },
}));
