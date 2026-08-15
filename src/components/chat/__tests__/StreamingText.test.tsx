/**
 * StreamingText tests — frontend reveal pacing.
 *
 * The backend forwards every LLM delta as it arrives; the component reveals
 * content progressively. Covers:
 *  - small deltas render immediately
 *  - large bursts reveal smoothly across ticks (no block jump)
 *  - truncation syncs exactly
 *  - the cursor sits at the stream frontier; stall breathing when the
 *    stream goes quiet
 */

import { describe, it, expect, afterEach, vi } from "vitest";
import { render, act } from "@testing-library/react";
import { StreamingText, StreamingCursor } from "@/components/chat/StreamingText";

function textOf(container: HTMLElement): string {
  const inner = container.querySelector("span");
  return inner?.textContent ?? "";
}

afterEach(() => {
  vi.useRealTimers();
});

describe("StreamingText", () => {
  it("renders the initial content immediately (no backlog on mount)", () => {
    const { container } = render(<StreamingText content="hello world" />);
    expect(textOf(container)).toBe("hello world");
  });

  it("renders small deltas instantly", () => {
    const { container, rerender } = render(<StreamingText content="hello " />);
    rerender(<StreamingText content="hello world" />);
    expect(textOf(container)).toBe("hello world");
  });

  it("paces a large burst across ticks (smooth reveal, no block jump)", () => {
    vi.useFakeTimers();
    const { container, rerender } = render(<StreamingText content="" />);
    const burst = "一".repeat(600);
    rerender(<StreamingText content={burst} />);
    // Not all revealed immediately — the text types out smoothly
    // instead of jumping in blocks.
    expect(textOf(container).length).toBeLessThan(600);
    // The bounded curve types 600 chars in ~840ms (32→20→12→6→3 steps).
    act(() => vi.advanceTimersByTime(1000));
    expect(textOf(container)).toBe(burst);
  });

  it("truncation syncs exactly (mid-burst)", () => {
    vi.useFakeTimers();
    const { container, rerender } = render(<StreamingText content="" />);
    rerender(<StreamingText content="一二三四五六七八九十" />);
    rerender(<StreamingText content="短" />);
    expect(textOf(container)).toBe("短");
  });

  it("clears the node when content becomes empty", () => {
    const { container, rerender } = render(<StreamingText content="hello" />);
    rerender(<StreamingText content="" />);
    expect(textOf(container)).toBe("");
  });

  it("keeps a streaming cursor element", () => {
    const { container } = render(<StreamingText content="hi" />);
    expect(container.querySelector(".streaming-cursor")).not.toBeNull();
  });

  it("quiet stream (>1.5s without text) switches the cursor to stall breathing", () => {
    vi.useFakeTimers();
    const { container, rerender } = render(<StreamingText content="abc" />);
    expect(container.querySelector(".streaming-cursor")?.className).not.toContain(
      "streaming-cursor-stalled",
    );
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(container.querySelector(".streaming-cursor")?.className).toContain(
      "streaming-cursor-stalled",
    );
    // A new append snaps the cursor back to the normal pulse.
    rerender(<StreamingText content="abc def" />);
    expect(container.querySelector(".streaming-cursor")?.className).not.toContain(
      "streaming-cursor-stalled",
    );
  });

  it("does not stall-report before any text arrived", () => {
    vi.useFakeTimers();
    const { container } = render(<StreamingText content="" />);
    act(() => {
      vi.advanceTimersByTime(5000);
    });
    expect(container.querySelector(".streaming-cursor")?.className).not.toContain(
      "streaming-cursor-stalled",
    );
  });
});

describe("StreamingCursor", () => {
  it("renders the normal pulse by default and the stall variant when stalled", () => {
    const { container, rerender } = render(<StreamingCursor stalled={false} />);
    expect(container.querySelector(".streaming-cursor")).not.toBeNull();
    expect(container.querySelector(".streaming-cursor-stalled")).toBeNull();
    rerender(<StreamingCursor stalled />);
    expect(container.querySelector(".streaming-cursor-stalled")).not.toBeNull();
  });
});
