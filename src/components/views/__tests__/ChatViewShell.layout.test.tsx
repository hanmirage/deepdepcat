/**
 * ChatViewShell layout regression tests.
 *
 * Guards the flex structure that keeps the message list above the input:
 * the content area MUST be a column flex container — without flex-col,
 * MessageList (which sizes itself with flex-1/h-full) overflows the area
 * and the list's tail slides UNDER the input row, hiding output text.
 */

import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ChatViewShell } from "@/components/views/ChatViewShell";
import type { UIMessage } from "@/types";

vi.mock("/icon.png", () => ({ default: "icon.png" }));
vi.mock("/icon-code.png", () => ({ default: "icon-code.png" }));
vi.mock("/icon-depwork.png", () => ({ default: "icon-depwork.png" }));

vi.mock("@/lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("@/lib/tauri")>("@/lib/tauri");
  return {
    ...actual,
    onEvent: () => Promise.resolve(() => {}),
  };
});

// jsdom has no ResizeObserver — MessageList / ChatInput rely on it.
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
vi.stubGlobal("ResizeObserver", ResizeObserverStub);

function makeMessages(): UIMessage[] {
  return [
    {
      id: "m1",
      role: "user",
      blocks: [{ type: "text", content: "hello" }],
      timestamp: 0,
    },
    {
      id: "m2",
      role: "assistant",
      blocks: [{ type: "text", content: "response text" }],
      timestamp: 0,
    },
  ];
}

function renderShell(overrides: Partial<React.ComponentProps<typeof ChatViewShell>> = {}) {
  return render(
    <ChatViewShell
      mode="code"
      messages={makeMessages()}
      isEmpty={false}
      notification={null}
      dismissNotification={() => {}}
      sessionId="s1"
      pendingElicitation={null}
      respondElicitation={async () => {}}
      {...overrides}
    />,
  );
}

describe("ChatViewShell layout", () => {
  it("renders the message list BEFORE the input row (list on top, input docked)", () => {
    const { container } = renderShell();

    // The input root is tagged with data-chat-input-root (ChatInput).
    const input = container.querySelector("[data-chat-input-root]");
    expect(input).not.toBeNull();

    // The message list scroll area exists.
    const list = container.querySelector("[data-radix-scroll-area-viewport]");
    expect(list).not.toBeNull();

    // Order: list must come before the input in the DOM.
    expect(list!.compareDocumentPosition(input!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("keeps the message list inside a column-flex content area", () => {
    const { container } = renderShell();

    const input = container.querySelector("[data-chat-input-root]");
    const inputRow = input?.parentElement;
    // The input's immediate parent is the "input row" (shrink-0, relative).
    expect(inputRow?.className).toMatch(/shrink-0/);
    expect(inputRow?.className).toMatch(/relative/);

    // Its parent is the content area — must be a column flex container.
    // Without flex-col, MessageList (flex-1/h-full) overflows and the list
    // tail gets hidden UNDER the input row (regression fixed in #layout).
    const contentArea = inputRow?.parentElement;
    expect(contentArea?.className).toMatch(/flex-col/);
    expect(contentArea?.className).toMatch(/flex-1/);
  });

  it("renders the empty state (welcome) without a docked input row", () => {
    const { container } = renderShell({ messages: [], isEmpty: true });
    // The welcome card embeds its own ChatInput…
    const input = container.querySelector("[data-chat-input-root]");
    expect(input).not.toBeNull();
    // …but NOT in the docked input row (shrink-0 wrapper) — the docked row
    // only exists in the non-empty state.
    const parent = input?.parentElement;
    expect(parent?.className).not.toMatch(/shrink-0/);
  });

  it("keeps user messages right-aligned and assistant messages left-aligned", () => {
    const { container } = renderShell();
    const user = container.querySelector('[data-message-role="user"]');
    const assistant = container.querySelector('[data-message-role="assistant"]');
    // User bubble on the RIGHT, assistant narrative on the LEFT — the two
    // sides stay visually apart (user's product decision).
    expect(user?.className).toMatch(/justify-end/);
    expect(user?.className).not.toMatch(/justify-start/);
    expect(assistant?.className).toMatch(/justify-start/);
    expect(assistant?.className).not.toMatch(/justify-end/);
    // The REAL alignment lives inside UserMessage (its root is w-full, so
    // the wrapper alone cannot move the bubble) — asserted in
    // UserMessage.edit.test.tsx (row justify-end + bubble last child).
  });
});
