/**
 * OfficeTypingHint tests — event-driven floating hint while the agent
 * types into an open WPS window.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, act } from "@testing-library/react";
import { OfficeTypingHint } from "@/components/chat/OfficeTypingHint";
import { onEvent } from "@/lib/tauri";

const emit = vi.fn();

vi.mock("@/lib/tauri", () => ({
  onEvent: vi.fn(() => Promise.resolve(() => {})),
}));

beforeEach(() => {
  emit.mockClear();
  (onEvent as unknown as ReturnType<typeof vi.fn>).mockClear();
});

afterEach(() => {
  vi.restoreAllMocks();
});

/** Capture the event callback and return a trigger helper. */
function bindEvent() {
  const { calls } = (onEvent as unknown as ReturnType<typeof vi.fn>).mock;
  const handler = calls[0][1] as (p: unknown) => void;
  return {
    fire: (payload: unknown) => handler(payload),
    event: calls[0][0],
  };
}

describe("OfficeTypingHint", () => {
  it("listens on the office-typing channel", () => {
    render(<OfficeTypingHint />);
    expect(onEvent).toHaveBeenCalled();
    const { event } = bindEvent();
    expect(event).toBe("office-typing");
  });

  it("shows the hint while typing is active", () => {
    const { container } = render(<OfficeTypingHint />);
    const { fire } = bindEvent();
    act(() => fire({ active: true, chunk: 2, total: 5, chars: 800, target: "a.docx" }));
    expect(container.textContent).toContain("WPS");
    expect(container.textContent).toContain("2/5");
  });

  it("hides after the done event (after the linger delay)", async () => {
    vi.useFakeTimers();
    try {
      const { container } = render(<OfficeTypingHint />);
      const { fire } = bindEvent();
      act(() => fire({ active: true, chunk: 1, total: 3 }));
      expect(container.textContent).toContain("WPS");
      act(() => fire({ active: false }));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1000);
      });
      expect(container.textContent).toBe("");
    } finally {
      vi.useRealTimers();
    }
  });

  it("renders nothing before any event", () => {
    const { container } = render(<OfficeTypingHint />);
    bindEvent();
    expect(container.textContent).toBe("");
  });

  it("keeps the listener alive after mount and unsubscribes on unmount", async () => {
    const unlisten = vi.fn();
    (onEvent as unknown as ReturnType<typeof vi.fn>).mockResolvedValueOnce(unlisten);
    const { unmount } = render(<OfficeTypingHint />);
    await act(async () => {});
    // The resolved unlisten must be retained, NOT invoked immediately —
    // invoking it on mount would unsubscribe the just-registered listener
    // and make the feature dead (regression guard for the swapped branch).
    expect(unlisten).not.toHaveBeenCalled();
    unmount();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });
});
