/**
 * Settings store — split by concern (types / storage / helpers / sync).
 */


import { resolvePricing, type ModelWithPricing } from "@/config/models";
import type { ApiFormat, ProviderConfig } from "./types";

/** A raw model entry returned by a provider `/models` endpoint. */
export interface RawFetchedModel {
  id: string;
  name: string;
  context_window?: number;
}

export type ModelListPayload = { data?: unknown[]; models?: unknown[] };

/** Industry-standard context window presets (tokens). */
export const CONTEXT_WINDOW_OPTIONS: number[] = [128_000, 256_000, 512_000, 1_000_000];

/** Format a token count readably: 128_000 → "128K", 1_000_000 → "1M". */
export function formatContextWindow(tokens: number): string {
  if (tokens >= 1_000_000) {
    const m = tokens / 1_000_000;
    return `${m % 1 === 0 ? m.toFixed(0) : m.toFixed(1)}M`;
  }
  if (tokens >= 1_000) return `${Math.round(tokens / 1000)}K`;
  return String(tokens);
}

/**
 * Context-window select options for the settings UI. Standard presets plus
 * the current value when it is not one of them (so non-standard windows such
 * as Claude's 200K stay visible instead of silently snapping).
 */
export function contextWindowOptions(current: number): { value: string; label: string }[] {
  const presets = CONTEXT_WINDOW_OPTIONS.map((n) => ({
    value: String(n),
    label: formatContextWindow(n),
  }));
  if (CONTEXT_WINDOW_OPTIONS.includes(current)) return presets;
  return [{ value: String(current), label: formatContextWindow(current) }, ...presets];
}

/**
 * Parse provider model-list payloads into a normalized raw list.
 *
 * Handles the three response shapes the app supports:
 * - OpenAI-compatible / Responses: `{ data: [{ id, name?, context_window? }] }`
 * - Anthropic: `{ data: [{ id, display_name?, context_window? }] }`
 *   (also tolerates a `models` key)
 * - Gemini: `{ models: [{ name: "models/xxx", displayName?, inputTokenLimit? }] }`
 */
export function parseModelListPayload(
  payload: ModelListPayload,
  apiFormat: ApiFormat,
): RawFetchedModel[] {
  const objects = (entries: unknown[]) =>
    entries.filter((e): e is Record<string, unknown> => typeof e === "object" && e !== null);

  if (apiFormat === "gemini") {
    return objects(payload.models ?? []).map((m) => {
      const name = String(m.name ?? "");
      return {
        id: name.replace(/^models\//, ""),
        name: String(m.displayName ?? m.name ?? ""),
        context_window: typeof m.inputTokenLimit === "number" ? m.inputTokenLimit : undefined,
      };
    });
  }

  if (apiFormat === "anthropic") {
    return objects(payload.data ?? payload.models ?? []).map((m) => ({
      id: String(m.id ?? m.name ?? ""),
      name: String(m.display_name ?? m.name ?? m.id ?? ""),
      context_window: typeof m.context_window === "number" ? m.context_window : undefined,
    }));
  }

  // OpenAI-compatible + Responses
  return objects(payload.data ?? []).map((m) => ({
    id: String(m.id ?? ""),
    name: String(m.name ?? m.id ?? ""),
    context_window: typeof m.context_window === "number" ? m.context_window : undefined,
  }));
}

export function buildModelsFromProviders(providers: ProviderConfig[]): ModelWithPricing[] {
  return providers
    .filter((p) => p.enabled)
    .flatMap((provider) =>
      provider.models
        // Capability matching: the DeepSeek Responses API currently only
        // serves deepseek-v4-flash — hide the pro model when the provider is
        // on the Responses protocol so the picker matches the backend.
        .filter(
          (m) =>
            !(
              provider.id === "deepseek" &&
              provider.apiFormat === "responses" &&
              m.id.toLowerCase().includes("pro")
            ),
        )
        .map((m) => ({
          id: m.id,
          name: m.name,
          provider: provider.name,
          providerId: provider.id,
          description: `${provider.name} · ${provider.apiFormat}`,
          context_window: resolveContextWindow(m.id, m.contextWindow),
          pricing: resolvePricing(m.id),
        })),
    );
}

/**
 * The gap between the current provider config and a usable model list.
 * Drives the setup card / input notice: tell the user exactly what's
 * missing, and offer the one-click action that closes the gap.
 */
export type ModelSetupStatus = "ready" | "no-provider" | "missing-key" | "no-models";

export function getModelSetupStatus(providers: ProviderConfig[]): ModelSetupStatus {
  const enabled = providers.filter((p) => p.enabled);
  if (enabled.length === 0) return "no-provider";
  if (enabled.some((p) => p.models.length > 0)) return "ready";
  if (enabled.some((p) => p.apiKey && p.baseUrl)) return "no-models";
  return "missing-key";
}

/** Context windows for models whose `/models` endpoint omits the field. */
const KNOWN_MODEL_CONTEXTS: Record<string, number> = {
  "deepseek-v4-pro": 1_000_000,
  "deepseek-v4-flash": 1_000_000,
  "gpt-4o": 128_000,
  "gpt-4o-mini": 128_000,
  "claude-3.5-sonnet": 200_000,
  "claude-3-5-sonnet-20241022": 200_000,
};

/** Look up the known context window for a model id (exact match, then prefix). */
export function knownContextWindow(modelId: string): number | undefined {
  const id = modelId.trim();
  if (KNOWN_MODEL_CONTEXTS[id]) return KNOWN_MODEL_CONTEXTS[id];
  if (id.startsWith("deepseek-v4-")) return 1_000_000;
  return undefined;
}

/**
 * Display-side context window resolution: the known value wins over a
 * stored fallback; an explicitly set value (anything other than the 32000
 * fetch fallback) is authoritative — manual edits in Settings must reach
 * the backend unchanged.
 */
export function resolveContextWindow(modelId: string, stored: number): number {
  if (stored !== 32000) return stored;
  return knownContextWindow(modelId) ?? stored;
}

/**
 * Heal model lists loaded from storage/backend: models whose id is known
 * and whose stored contextWindow is still the fetch fallback 32000 get
 * corrected, so a stale `?? 32000` from an earlier fetch does not stick
 * forever. Manually-chosen values are never overwritten.
 */
export function healKnownContexts(providers: ProviderConfig[]): ProviderConfig[] {
  return providers.map((p) => ({
    ...p,
    models: p.models.map((m) => {
      const known = knownContextWindow(m.id);
      const healed = known !== undefined && m.contextWindow === 32000 ? { ...m, contextWindow: known } : m;
      // Heal synthetic ids from the old fetch bug (`model-<ts>-<n>`): the
      // id IS the API model name — a synthetic id makes every request fail
      // with HTTP 400. When the display name itself looks like a real model
      // id (lowercase, no spaces), promote it; otherwise leave for re-fetch.
      if (/^model-\d+(-\d+)?$/.test(healed.id) && /^[a-z0-9_.:-]+$/.test(healed.name.trim())) {
        return { ...healed, id: healed.name.trim() };
      }
      return healed;
    }),
  }));
}

// ── Backend sync ─────────────────────────────────────────────
