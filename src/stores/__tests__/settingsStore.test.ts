import { describe, it, expect, beforeEach } from "vitest";
import { useSettingsStore } from "@/stores/settingsStore";

describe("settingsStore.updateModel", () => {
  beforeEach(() => {
    useSettingsStore.setState({
      providers: [
        {
          id: "relay",
          name: "Relay",
          baseUrl: "https://relay.example.com/v1",
          apiKey: "sk-test",
          apiFormat: "openai",
          models: [{ id: "kimi-k3", name: "Kimi K3", contextWindow: 32000 }],
          enabled: true,
        },
      ],
    });
  });

  it("updates a model's context window in state and storage", () => {
    useSettingsStore.getState().updateModel("relay", "kimi-k3", { contextWindow: 256_000 });
    const models = useSettingsStore.getState().providers[0].models;
    expect(models[0].contextWindow).toBe(256_000);

    const stored = JSON.parse(localStorage.getItem("deepdepcat-settings") ?? "{}");
    expect(stored.providers[0].models[0].contextWindow).toBe(256_000);
  });

  it("leaves other providers and models untouched", () => {
    useSettingsStore.getState().updateModel("relay", "kimi-k3", { contextWindow: 128_000 });
    useSettingsStore.getState().updateModel("relay", "missing", { contextWindow: 1 });
    const models = useSettingsStore.getState().providers[0].models;
    expect(models).toHaveLength(1);
    expect(models[0].contextWindow).toBe(128_000);
  });
});
