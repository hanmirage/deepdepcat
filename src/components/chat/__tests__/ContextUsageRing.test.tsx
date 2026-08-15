/**
 * ContextUsageRing tests — the ring shows real occupancy and, while streaming,
 * projects this turn's streamed-output estimate on top (DeepSeek reports usage
 * only at stream end).
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { ContextUsageRing } from "@/components/chat/ContextUsageRing";
import { useChatStore } from "@/stores/chatStore";
import { useAppStore } from "@/stores/appStore";

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    sessionApi: {
      ...actual.sessionApi,
      getSessionUsage: vi.fn(async () => ({
        session_id: "s1",
        total_prompt_tokens: 10000,
        total_completion_tokens: 0,
        total_cached_read_tokens: 0,
        total_reasoning_tokens: 0,
        total_tool_calls: 0,
        total_tool_result_tokens: 0,
        turn_count: 1,
        context_window: 64000,
        current_context_tokens: 10000,
        context_breakdown: {
          system_prompt_tokens: 2000,
          skill_tokens: 0,
          tool_definition_tokens: 1000,
          conversation_tokens: 6000,
          tool_result_tokens: 1000,
        },
        total_cache_hit_tokens: 0,
        total_cache_miss_tokens: 0,
        cache_hit_ratio: null,
      })),
    },
  };
});

beforeEach(() => {
  useAppStore.setState({ mode: "code" });
  useChatStore.setState({
    currentSessionId: "s1",
    isStreaming: false,
    messages: [],
  });
});

function ringTitle(): string {
  return screen.getByRole("button").title;
}

describe("ContextUsageRing", () => {
  it("shows the real anchor when idle", async () => {
    render(<ContextUsageRing sessionId="s1" mode="code" />);
    await waitFor(() => {
      expect(ringTitle()).toContain("10,000");
    });
    expect(ringTitle()).toContain("64,000");
  });

  it("projects the streamed-output estimate on top while streaming", async () => {
    render(<ContextUsageRing sessionId="s1" mode="code" />);
    await waitFor(() => {
      expect(ringTitle()).toContain("10,000");
    });

    // 300 ASCII chars stream in → +75 estimated tokens → 10,075 projected.
    useChatStore.setState({
      isStreaming: true,
      messages: [
        {
          id: "a1",
          role: "assistant",
          timestamp: 0,
          isStreaming: true,
          blocks: [{ type: "text", content: "x".repeat(300), streamId: "t1:s0" }],
        },
      ],
    });

    await waitFor(() => {
      expect(ringTitle()).toContain("10,075");
    });
  });
});
