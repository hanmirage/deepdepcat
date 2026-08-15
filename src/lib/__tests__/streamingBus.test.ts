/**
 * streamingBus tests — cross-store streaming notifications.
 */

import { describe, it, expect } from "vitest";
import { renderHook, act } from "@testing-library/react";
import {
  setSessionStreaming,
  isSessionStreaming,
  useStreamingSessions,
} from "@/lib/streamingBus";

describe("streamingBus", () => {
  it("tracks active sessions and emits only on change", () => {
    const { result, unmount } = renderHook(() => useStreamingSessions());
    expect(result.current.size).toBe(0);

    act(() => {
      setSessionStreaming("s1", true);
      setSessionStreaming("s1", true); // no-op — no emit
    });
    expect(result.current.has("s1")).toBe(true);
    expect(isSessionStreaming("s1")).toBe(true);
    expect(isSessionStreaming("s2")).toBe(false);

    act(() => setSessionStreaming("s2", true));
    expect(result.current.size).toBe(2);

    act(() => setSessionStreaming("s1", false));
    expect(result.current.has("s1")).toBe(false);
    expect(result.current.has("s2")).toBe(true);

    unmount();
  });
});
