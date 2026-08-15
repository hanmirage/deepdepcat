/**
 * typewriter tests — smooth reveal pacing for streaming text.
 *
 * The backend forwards every LLM delta as it arrives; this module smooths
 * the renderer's reveal. A burst must reveal progressively (no block jump)
 * and catch up within a few ticks; huge bursts race to catch up.
 */

import { describe, it, expect, afterEach, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";
import { stepFor, nextEnd, useReveal, REVEAL_PACE_MS } from "@/lib/typewriter";

afterEach(() => {
  vi.useRealTimers();
});

describe("stepFor", () => {
  it("paces small backlogs in small steps", () => {
    expect(stepFor(12)).toBe(3);
    expect(stepFor(48)).toBe(6);
    expect(stepFor(96)).toBe(12);
  });

  it("types large backlogs at a bounded rate (no fast-forward)", () => {
    expect(stepFor(200)).toBe(20);
    expect(stepFor(1024)).toBe(32);
  });

  it("caps the step so huge bursts stay a visible typewriter", () => {
    expect(stepFor(10000)).toBe(32);
  });
});

describe("nextEnd", () => {
  it("never advances past the text length", () => {
    expect(nextEnd("abc", 0)).toBeLessThanOrEqual(3);
  });

  it("snaps forward to a word boundary after the step", () => {
    // "hello world" from 0: step=3 lands mid-"hello"; snap → end of "hello"
    // (the boundary space is consumed — a word is never split).
    const text = "hello world";
    const end = nextEnd(text, 0);
    expect(text.slice(0, end)).toBe("hello ");
  });

  it("snap lookahead is bounded", () => {
    // A very long word: no boundary within the lookahead → plain step end.
    const text = "supercalifragilisticexpialidocious";
    const end = nextEnd(text, 0);
    expect(end).toBeLessThanOrEqual(6 + 8);
  });

  it("snaps CJK text at clause punctuation", () => {
    // 中文断句标点也在边界集里 — 落在自然断句处，而不是数数字符。
    const text = "第一句。第二句。";
    const end = nextEnd(text, 0);
    expect(text.slice(0, end)).toBe("第一句。");
  });

  it("paces CJK text by the step (no ASCII boundary chars among glyphs)", () => {
    const text = "一二三四五六七八九十";
    const end = nextEnd(text, 0);
    expect(end).toBe(3); // step only; no clause punctuation among CJK glyphs
  });
});

describe("useReveal", () => {
  it("shows the initial content immediately (no backlog on mount)", () => {
    const { result } = renderHook(() => useReveal("hello"));
    expect(result.current.shown).toBe("hello");
    expect(result.current.caughtUp).toBe(true);
  });

  it("revealOnMount types out a large first chunk instead of painting it", () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => useReveal("x".repeat(200), false, true));
    // Mount shows only the instant window, not the whole first burst.
    expect(result.current.shown.length).toBeLessThan(200);
    expect(result.current.caughtUp).toBe(false);

    let steps = 0;
    while (result.current.shown.length < 200 && steps < 100) {
      act(() => vi.advanceTimersByTime(24 + 1));
      steps += 1;
    }
    expect(result.current.shown).toBe("x".repeat(200));
    expect(result.current.caughtUp).toBe(true);
  });

  it("revealOnMount still shows a small first chunk whole", () => {
    const { result } = renderHook(() => useReveal("hello", false, true));
    expect(result.current.shown).toBe("hello");
    expect(result.current.caughtUp).toBe(true);
  });

  it("revealOnMount types a LARGE initial content too (live first chunk must not snap)", () => {
    // Regression: a live stream can open with a big delta (> the old
    // REVEAL_ON_MOUNT_MAX heuristic) — it must STILL type, not show whole.
    vi.useFakeTimers();
    const { result } = renderHook(() => useReveal("x".repeat(500), false, true));
    expect(result.current.shown.length).toBeLessThan(500);
    expect(result.current.caughtUp).toBe(false);

    let steps = 0;
    while (result.current.shown.length < 500 && steps < 200) {
      act(() => vi.advanceTimersByTime(24 + 1));
      steps += 1;
    }
    expect(result.current.shown).toBe("x".repeat(500));
    expect(result.current.caughtUp).toBe(true);
  });

  it("renders small deltas instantly (≤ REVEAL_IMMEDIATE)", () => {
    vi.useFakeTimers();
    const { result, rerender } = renderHook(({ c }) => useReveal(c), {
      initialProps: { c: "hello" },
    });
    rerender({ c: "hello world" }); // +5 chars
    expect(result.current.shown).toBe("hello world");
    expect(result.current.caughtUp).toBe(true);
  });

  it("smooths a 600-char burst across ticks, then catches up", () => {
    vi.useFakeTimers();
    const { result, rerender } = renderHook(({ c }) => useReveal(c), {
      initialProps: { c: "" },
    });
    const burst = "x".repeat(600);
    rerender({ c: burst });
    // Not revealed in one jump — the burst types out smoothly.
    expect(result.current.shown.length).toBeLessThan(600);
    expect(result.current.caughtUp).toBe(false);

    let steps = 0;
    while (result.current.shown.length < 600 && steps < 200) {
      act(() => vi.advanceTimersByTime(REVEAL_PACE_MS + 1));
      steps += 1;
    }
    expect(result.current.shown).toBe(burst);
    expect(result.current.caughtUp).toBe(true);
  });

  it("races a huge burst to catch up", () => {
    vi.useFakeTimers();
    const { result, rerender } = renderHook(({ c }) => useReveal(c), {
      initialProps: { c: "" },
    });
    const burst = "x".repeat(2000);
    rerender({ c: burst });
    expect(result.current.shown.length).toBeLessThan(2000);

    let steps = 0;
    while (result.current.shown.length < 2000 && steps < 300) {
      act(() => vi.advanceTimersByTime(REVEAL_PACE_MS + 1));
      steps += 1;
      expect(result.current.shown.length).toBeGreaterThanOrEqual(1);
    }
    expect(result.current.shown).toBe(burst);
    expect(result.current.caughtUp).toBe(true);
  });

  it("reveals in monotonic prefix steps (shown is always a prefix)", () => {
    vi.useFakeTimers();
    const { result, rerender } = renderHook(({ c }) => useReveal(c), {
      initialProps: { c: "" },
    });
    rerender({ c: "abc def ghi jkl mno pqr stu" });
    let prev = "";
    let steps = 0;
    while (result.current.shown.length < 27 && steps < 100) {
      act(() => vi.advanceTimersByTime(REVEAL_PACE_MS + 1));
      steps += 1;
      const s = result.current.shown;
      expect(s.startsWith(prev)).toBe(true);
      prev = s;
    }
    expect(result.current.shown).toBe("abc def ghi jkl mno pqr stu");
  });

  it("syncs immediately on truncation", () => {
    vi.useFakeTimers();
    const { result, rerender } = renderHook(({ c }) => useReveal(c), {
      initialProps: { c: "" },
    });
    rerender({ c: "x".repeat(200) });
    rerender({ c: "short" }); // shrinks → sync
    expect(result.current.shown).toBe("short");
    expect(result.current.caughtUp).toBe(true);
  });

  it("flips stalled only after catching up AND quiet > 1.5s", () => {
    vi.useFakeTimers();
    const { result, rerender } = renderHook(({ c }) => useReveal(c), {
      initialProps: { c: "" },
    });
    rerender({ c: "hello" }); // small delta, instant, caughtUp
    expect(result.current.stalled).toBe(false);
    act(() => vi.advanceTimersByTime(2000));
    expect(result.current.stalled).toBe(true);
    // A new append snaps it back.
    rerender({ c: "hello world" });
    expect(result.current.stalled).toBe(false);
  });

  it("does NOT jump a large live burst at the quiet window (still typing)", () => {
    // Regression: a live turn's big first delta must keep typing, not snap to
    // the end when the stream briefly pauses (e.g. the agent switches to a
    // tool) — a static catch-up on live content was the "一次性出来" bug.
    vi.useFakeTimers();
    const { result } = renderHook(() => useReveal("x".repeat(3000), false, true));
    // Revealing, not caught up.
    expect(result.current.shown.length).toBeLessThan(3000);
    expect(result.current.caughtUp).toBe(false);
    // After the quiet window (1.5s) the reveal is still pacing — it must NOT
    // have jumped to the end (3000 chars at 32/tick = ~62 ticks = ~1.5s, so
    // it is mid-reveal, not done).
    act(() => vi.advanceTimersByTime(1500));
    expect(result.current.shown.length).toBeLessThan(3000);
    expect(result.current.caughtUp).toBe(false);
  });

  it("keeps typing a static backlog (no catch-up jump), stalls only when done", () => {
    vi.useFakeTimers();
    const { result, rerender } = renderHook(({ c }) => useReveal(c), {
      initialProps: { c: "" },
    });
    rerender({ c: "x".repeat(6000) });
    // Still revealing at 1s and 2s — 6000 chars at 32/tick takes ~4.5s.
    act(() => vi.advanceTimersByTime(1000));
    expect(result.current.caughtUp).toBe(false);
    expect(result.current.stalled).toBe(false);
    act(() => vi.advanceTimersByTime(1000));
    expect(result.current.caughtUp).toBe(false);
    expect(result.current.stalled).toBe(false);
    // Let it finish typing, then the settled stream reaches the stall breathing.
    act(() => vi.advanceTimersByTime(6000));
    expect(result.current.shown.length).toBe(6000);
    expect(result.current.caughtUp).toBe(true);
    act(() => vi.advanceTimersByTime(2000));
    expect(result.current.stalled).toBe(true);
  });

  it("never stalls before any text arrived", () => {
    vi.useFakeTimers();
    const { result } = renderHook(() => useReveal(""));
    act(() => vi.advanceTimersByTime(5000));
    expect(result.current.stalled).toBe(false);
  });
});
