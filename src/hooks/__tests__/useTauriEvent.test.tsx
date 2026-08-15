/**
 * useTauriEvent tests — subscription lifecycle contract.
 *
 * Guards against the two failure modes found in the codebase:
 *  1. resolving the unlisten and invoking it on mount (kills the listener)
 *  2. unmounting before the subscription resolves (leaked listener)
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { useTauriEvent } from "@/hooks/useTauriEvent";

describe("useTauriEvent", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("keeps the listener alive after mount and unsubscribes on unmount", async () => {
    const unlisten = vi.fn();
    const subscribe = vi.fn(() => Promise.resolve(unlisten));
    const handler = vi.fn();
    const { unmount } = renderHook(() => useTauriEvent(subscribe, handler));

    await act(async () => {});
    expect(subscribe).toHaveBeenCalledTimes(1);
    expect(unlisten).not.toHaveBeenCalled();

    unmount();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("cancels immediately when unmounted before the subscription resolves", async () => {
    const unlisten = vi.fn();
    let resolve: (fn: () => void) => void = () => {};
    const subscribe = vi.fn(
      () =>
        new Promise<() => void>((r) => {
          resolve = r;
        }),
    );
    const { unmount } = renderHook(() => useTauriEvent(subscribe, vi.fn()));

    unmount();
    await act(async () => {
      resolve(unlisten);
    });
    // The late-resolved unlisten must be invoked right away, not leaked.
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("delivers payloads to the latest handler (no stale closure)", async () => {
    const subscribe = vi.fn(() => Promise.resolve(() => {}));
    const handler1 = vi.fn();
    const { rerender } = renderHook(
      ({ h }) => useTauriEvent(subscribe, h),
      { initialProps: { h: handler1 } },
    );
    await act(async () => {});

    const wrapped = (subscribe.mock.calls as unknown as unknown[][])[0][0] as (
      p: unknown,
    ) => void;
    act(() => wrapped({ n: 1 }));
    expect(handler1).toHaveBeenCalledWith({ n: 1 });

    const handler2 = vi.fn();
    rerender({ h: handler2 });
    act(() => wrapped({ n: 2 }));
    expect(handler2).toHaveBeenCalledWith({ n: 2 });
    // Re-rendering must not re-subscribe.
    expect(subscribe).toHaveBeenCalledTimes(1);
  });

  it("does not subscribe while disabled and resubscribes when enabled", async () => {
    const subscribe = vi.fn(() => Promise.resolve(() => {}));
    const { rerender, unmount } = renderHook(
      ({ enabled }) => useTauriEvent(subscribe, vi.fn(), enabled),
      { initialProps: { enabled: false } },
    );
    expect(subscribe).not.toHaveBeenCalled();

    rerender({ enabled: true });
    await act(async () => {});
    expect(subscribe).toHaveBeenCalledTimes(1);

    rerender({ enabled: false });
    expect(subscribe).toHaveBeenCalledTimes(1);
    unmount();
  });
});
