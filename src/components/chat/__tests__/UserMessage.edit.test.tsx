/**
 * UserMessage edit-and-resend tests.
 *
 * The edit action reuses the store's deleteMessage (backend truncates the
 * conversation AT that user message), then restores the text to the input
 * for review and resend.
 */

import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { UserMessage } from "@/components/chat/UserMessage";
import { useChatStore } from "@/stores/chatStore";
import { useAppStore } from "@/stores/appStore";
import { streamState } from "@/stores/chatStore/streamState";

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    sessionApi: {
      ...actual.sessionApi,
      deleteMessage: vi.fn(async () => {}),
    },
  };
});

function seedConversation() {
  useAppStore.setState({ mode: "code" });
  // deleteMessage now operates on the per-session buffer — seed it too.
  const messages = [
    {
      id: "u1",
      role: "user" as const,
      blocks: [{ type: "text" as const, content: "旧消息" }],
      timestamp: 0,
    },
    {
      id: "a1",
      role: "assistant" as const,
      blocks: [{ type: "text" as const, content: "回答" }],
      timestamp: 1,
    },
  ];
  streamState("s1").messages = messages;
  useChatStore.setState({ currentSessionId: "s1", messages });
}

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("UserMessage edit & resend", () => {
  it("right-aligns the bubble and keeps hover actions to its left", () => {
    seedConversation();
    const { container } = render(<UserMessage messageId="u1" />);
    // The bubble is the distinctive max-w-[85%] block.
    const bubble = container.querySelector('[class*="max-w-[85%]"]');
    expect(bubble).not.toBeNull();
    // The message ROW itself right-aligns (the wrapper's justify-end is
    // ineffective because the component root is w-full).
    const row = bubble?.parentElement;
    expect(row?.className).toMatch(/justify-end/);
    // The bubble is the LAST child — actions sit to its LEFT, not outside
    // the right edge.
    const lastChild = row?.lastElementChild;
    expect(lastChild).toBe(bubble);
  });

  it("truncates the conversation at the message and restores text to the input", async () => {
    seedConversation();
    render(<UserMessage messageId="u1" />);

    fireEvent.click(screen.getByRole("button", { name: /编辑并重发/ }));

    await waitFor(() => {
      expect(useChatStore.getState().messages).toHaveLength(0);
      expect(useChatStore.getState().inputText).toBe("旧消息");
    });
  });
});
