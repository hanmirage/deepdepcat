/**
 * Settings store — split by concern (types / storage / helpers / sync).
 */


import { isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";
import type {
  ProviderConfig,
  GeneralSettings,
  VisionSettings,
  TitleBarStyle,
  ApiFormat,
} from "./types";

export const STORAGE_KEY = "deepdepcat-settings";
export function loadFromStorage(): {
  providers: ProviderConfig[];
  general: GeneralSettings;
  vision: VisionSettings;
} {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw);
      return {
        providers: parsed.providers ?? [],
        // Merge defaults so fields added later don't come back as undefined
        // from older persisted settings.
        general: { ...defaultGeneral(), ...(parsed.general ?? {}) },
        vision: { ...defaultVision(), ...(parsed.vision ?? {}) },
      };
    }
  } catch {
    // Corrupted storage — fall through to defaults
  }
  return {
    providers: defaultProviders(),
    general: defaultGeneral(),
    vision: defaultVision(),
  };
}

/**
 * Persist to localStorage. In the Tauri app the backend AppConfig is the
 * source of truth for API keys — localStorage must never hold plaintext
 * secrets (a renderer XSS could read them). Keys are stripped before the
 * write; browser dev mode keeps them so the mock/dev flow still works.
 */
export function saveToStorage(
  providers: ProviderConfig[],
  general: GeneralSettings,
  vision?: VisionSettings,
): void {
  try {
    const storedProviders = isTauri
      ? providers.map((p) => ({ ...p, apiKey: "" }))
      : providers;
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        providers: storedProviders,
        general,
        vision: vision ?? defaultVision(),
      }),
    );
  } catch (e) {
    logError("settingsStore", "Failed to save:", e);
  }
}

/** Remove any API keys an older build wrote to localStorage (Tauri only). */
export function scrubStoredSecrets(): void {
  if (!isTauri) return;
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return;
    const parsed = JSON.parse(raw) as {
      providers?: ProviderConfig[];
      general?: Partial<GeneralSettings>;
      vision?: VisionSettings;
    };
    const providers = parsed.providers ?? [];
    if (!providers.some((p) => p.apiKey)) return;
    saveToStorage(
      providers,
      { ...defaultGeneral(), ...(parsed.general ?? {}) },
      parsed.vision,
    );
  } catch {
    // Ignore — corrupted storage is handled by loadFromStorage.
  }
}

export function defaultGeneral(): GeneralSettings {
  return {
    // Keep in sync with i18n default language (src/i18n/index.ts) — a
    // mismatch makes the onboarding flow render in one language while the
    // main UI renders in another.
    language: "zh",
    proxyUrl: "",
    noProxyList: "localhost,127.0.0.1,::1",
    showThinking: false,
    showTodo: true,
    streamingSpeed: "smooth" as "smooth" | "instant",
    titleBarStyle: "windows" as TitleBarStyle,
serverUrl: "https://deepdepcat.hsmiai.xyz",
deepseekAutoReasoning: true,
sessionTokenLimit: 0,
sessionCostLimit: 0,
diagnosticsEnabled: true,
};
}

export function defaultVision(): VisionSettings {
  return {
    enabled: false,
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    apiKey: "",
    model: "glm-4v-flash",
  };
}

export function defaultProviders(): ProviderConfig[] {
  // Default provider skeletons — NO hardcoded models. Model lists are the
  // user's own data: fetched from the provider's /models endpoint (or added
  // manually) in Settings → Model Providers. The model picker reflects
  // exactly what is configured there, nothing else.
  return [
    {
      id: "deepseek",
      name: "DeepSeek",
      // Official base_url per https://api-docs.deepseek.com — no /v1 suffix.
      baseUrl: "https://api.deepseek.com",
      apiKey: "",
      apiFormat: "openai" as ApiFormat,
      models: [],
      enabled: true,
    },
    {
      id: "openai",
      name: "OpenAI",
      baseUrl: "https://api.openai.com/v1",
      apiKey: "",
      apiFormat: "openai" as ApiFormat,
      models: [],
      enabled: false,
    },
    {
      id: "anthropic",
      name: "Anthropic",
      baseUrl: "https://api.anthropic.com",
      apiKey: "",
      apiFormat: "anthropic" as ApiFormat,
      models: [],
      enabled: false,
    },
  ];
}
