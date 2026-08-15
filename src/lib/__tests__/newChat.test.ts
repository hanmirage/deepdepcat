/**
 * newChatInCurrentMode tests — Ctrl+N / New Chat stays in the current mode
 * and only resets the active product's conversation.
 */

import { describe, it, expect, beforeEach } from "vitest";
import { newChatInCurrentMode } from "@/lib/newChat";
import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";

const msg = {
  id: "u1",
  role: "user" as const,
  blocks: [{ type: "text" as const, content: "hi" }],
  timestamp: 1,
};

describe("newChatInCurrentMode", () => {
  beforeEach(() => {
    useAppStore.setState({ mode: "code" });
    useRightPanelStore.setState({ open: true });
    useChatStore.setState({
      messages: [msg],
      currentSessionId: "c1",
      inputText: "code text",
      sessionTitle: "Code session",
    });
    useDepworkChatStore.setState({
      messages: [msg],
      currentSessionId: "d1",
      inputText: "depwork text",
      sessionTitle: "Depwork session",
    });
  });

  it("resets only the Code surface when in code mode", () => {
    newChatInCurrentMode();

    expect(useChatStore.getState().messages).toEqual([]);
    expect(useChatStore.getState().currentSessionId).toBeNull();
    expect(useChatStore.getState().sessionTitle).toBe("New Session");
    // The other product's conversation stays untouched.
    expect(useDepworkChatStore.getState().messages).toHaveLength(1);
    expect(useDepworkChatStore.getState().currentSessionId).toBe("d1");
    // The right panel closes with the fresh conversation.
    expect(useRightPanelStore.getState().open).toBe(false);
  });

  it("resets only the Depwork surface when in depwork mode", () => {
    useAppStore.setState({ mode: "depwork" });
    newChatInCurrentMode();

    expect(useDepworkChatStore.getState().messages).toEqual([]);
    expect(useDepworkChatStore.getState().currentSessionId).toBeNull();
    expect(useChatStore.getState().messages).toHaveLength(1);
    expect(useChatStore.getState().currentSessionId).toBe("c1");
    expect(useRightPanelStore.getState().open).toBe(false);
  });
});
