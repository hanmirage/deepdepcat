import { describe, it, expect } from "vitest";
import {
  getModelSetupStatus,
  buildModelsFromProviders,
  resolveContextWindow,
  contextWindowOptions,
  formatContextWindow,
  type ProviderConfig,
} from "@/stores/settingsStore";

function provider(overrides: Partial<ProviderConfig>): ProviderConfig {
  return {
    id: "p",
    name: "Test",
    baseUrl: "https://api.example.com",
    apiKey: "sk-test",
    apiFormat: "openai",
    models: [],
    enabled: true,
    ...overrides,
  };
}

describe("getModelSetupStatus", () => {
  it("reports no-provider when there are no providers", () => {
    expect(getModelSetupStatus([])).toBe("no-provider");
  });

  it("reports no-provider when none are enabled", () => {
    expect(getModelSetupStatus([provider({ enabled: false })])).toBe("no-provider");
  });

  it("reports missing-key when an enabled provider lacks an API key", () => {
    expect(getModelSetupStatus([provider({ apiKey: "" })])).toBe("missing-key");
  });

  it("reports no-models when a keyed provider has an empty model list", () => {
    expect(getModelSetupStatus([provider({})])).toBe("no-models");
  });

  it("reports ready when any enabled provider has models", () => {
    const ready = provider({ models: [{ id: "m1", name: "Model 1", contextWindow: 32000 }] });
    expect(getModelSetupStatus([provider({ apiKey: "" }), ready])).toBe("ready");
  });

  it("missing-key wins over a keyless provider in the list", () => {
    expect(getModelSetupStatus([provider({ apiKey: "" }), provider({ apiKey: "" })])).toBe(
      "missing-key",
    );
  });
});

describe("buildModelsFromProviders", () => {
  it("carries the backend provider id for session routing", () => {
    const models = buildModelsFromProviders([
      provider({
        id: "relay-1",
        name: "Moonshot Relay",
        models: [{ id: "kimi-k3", name: "Kimi K3", contextWindow: 32000 }],
      }),
    ]);
    expect(models).toHaveLength(1);
    expect(models[0].provider).toBe("Moonshot Relay");
    expect(models[0].providerId).toBe("relay-1");
  });
});

describe("resolveContextWindow", () => {
  it("treats an explicitly set value as authoritative", () => {
    expect(resolveContextWindow("gpt-4o", 500_000)).toBe(500_000);
    expect(resolveContextWindow("custom-model", 256_000)).toBe(256_000);
  });

  it("falls back to the known table only for the 32000 fetch fallback", () => {
    expect(resolveContextWindow("gpt-4o", 32000)).toBe(128_000);
    expect(resolveContextWindow("custom-model", 32000)).toBe(32_000);
  });
});

describe("contextWindowOptions", () => {
  it("lists the standard presets in ascending order", () => {
    expect(contextWindowOptions(128_000).map((o) => o.value)).toEqual([
      "128000",
      "256000",
      "512000",
      "1000000",
    ]);
  });

  it("keeps a non-standard current value as the first option", () => {
    const options = contextWindowOptions(200_000);
    expect(options[0]).toEqual({ value: "200000", label: "200K" });
    expect(options.map((o) => o.value)).toContain("1000000");
  });

  it("formats token counts readably", () => {
    expect(formatContextWindow(1_000_000)).toBe("1M");
    expect(formatContextWindow(512_000)).toBe("512K");
    expect(formatContextWindow(128_000)).toBe("128K");
  });
});
