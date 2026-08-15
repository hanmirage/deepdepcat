import { describe, it, expect } from "vitest";
import { parseModelListPayload } from "@/stores/settingsStore/helpers";

describe("parseModelListPayload", () => {
  it("parses OpenAI-compatible payloads (openai / responses / custom)", () => {
    const parsed = parseModelListPayload(
      {
        data: [
          { id: "gpt-4o", name: "GPT-4o", context_window: 128_000 },
          { id: "gpt-4o-mini", name: "GPT-4o Mini" },
        ],
      },
      "openai",
    );
    expect(parsed).toEqual([
      { id: "gpt-4o", name: "GPT-4o", context_window: 128_000 },
      { id: "gpt-4o-mini", name: "GPT-4o Mini", context_window: undefined },
    ]);
  });

  it("parses Responses payloads the same way as OpenAI-compatible", () => {
    const parsed = parseModelListPayload(
      { data: [{ id: "deepseek-v4-flash", name: "DeepSeek V4 Flash" }] },
      "responses",
    );
    expect(parsed[0]).toEqual({
      id: "deepseek-v4-flash",
      name: "DeepSeek V4 Flash",
      context_window: undefined,
    });
  });

  it("parses Anthropic payloads with display_name and context_window", () => {
    const parsed = parseModelListPayload(
      {
        data: [
          { id: "claude-sonnet-4", display_name: "Claude Sonnet 4", context_window: 200_000 },
        ],
      },
      "anthropic",
    );
    expect(parsed).toEqual([
      { id: "claude-sonnet-4", name: "Claude Sonnet 4", context_window: 200_000 },
    ]);
  });

  it("tolerates Anthropic payloads under a models key", () => {
    const parsed = parseModelListPayload(
      { models: [{ id: "claude-3-5-haiku", name: "Claude 3.5 Haiku" }] },
      "anthropic",
    );
    expect(parsed[0].id).toBe("claude-3-5-haiku");
  });

  it("parses Gemini payloads, stripping the models/ name prefix", () => {
    const parsed = parseModelListPayload(
      {
        models: [
          {
            name: "models/gemini-1.5-pro",
            displayName: "Gemini 1.5 Pro",
            inputTokenLimit: 2_000_000,
          },
        ],
      },
      "gemini",
    );
    expect(parsed).toEqual([
      { id: "gemini-1.5-pro", name: "Gemini 1.5 Pro", context_window: 2_000_000 },
    ]);
  });

  it("ignores non-object entries and falls back to the model id as name", () => {
    const parsed = parseModelListPayload(
      { data: [{ id: "m1" }, "junk", null, 42] },
      "openai",
    );
    expect(parsed).toEqual([{ id: "m1", name: "m1", context_window: undefined }]);
  });
});
