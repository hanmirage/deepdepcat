/**
 * Direct render probe — a message seeded as streaming (isStreaming: true,
 * the real-app state during a held invoke) must type its long text out, not
 * paint it whole. Isolates the render path from listener/mock timing.
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup, act } from "@testing-library/react";
import { AssistantMessage } from "@/components/chat/AssistantMessage";
import { useChatStore } from "@/stores/chatStore";
import { useAppStore } from "@/stores/appStore";
import type { UIMessage } from "@/types";

function renderStreaming(text: string) {
  const message: UIMessage = {
    id: "m1",
    role: "assistant",
    blocks: [{ type: "text", content: text, streamId: "t1:s0" }],
    timestamp: 0,
    isStreaming: true,
  };
  useChatStore.setState({ messages: [message] });
  return render(<AssistantMessage messageId="m1" />);
}

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("streaming text render", () => {
  it("a short reply shows immediately", () => {
    useAppStore.setState({ mode: "code" });
    renderStreaming("你好！有什么可以帮你的吗？");
    expect(screen.getByText(/你好/)).toBeInTheDocument();
  });

  it("a long reply types out progressively, not all at once", () => {
    vi.useFakeTimers();
    useAppStore.setState({ mode: "code" });
    const long = `好的，我来为你写一个官网。${"这是详细说明。".repeat(80)}`;
    const { container } = renderStreaming(long);
    // At mount only a prefix is revealed — not the whole long reply.
    expect(container.textContent!.length).toBeLessThan(long.length);
    expect(container.textContent!.length).toBeGreaterThan(0);
    // Advance a few ticks — the reveal should grow.
    act(() => vi.advanceTimersByTime(100));
    const after100 = container.textContent!.length;
    act(() => vi.advanceTimersByTime(2000));
    // Eventually the whole reply is revealed.
    expect(container.textContent!.length).toBeGreaterThan(after100);
    expect(container.textContent!.length).toBeGreaterThanOrEqual(long.length);
  });
});
