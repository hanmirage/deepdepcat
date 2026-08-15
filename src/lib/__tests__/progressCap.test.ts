/**
 * progressCap tests — streamed tool field caps.
 */

import { describe, it, expect } from "vitest";
import { appendCapped, MAX_PROGRESS_CHARS } from "@/lib/progressCap";

describe("appendCapped", () => {
  it("accumulates below the cap untouched", () => {
    expect(appendCapped("ab", "cd")).toBe("abcd");
    expect(appendCapped("", "x".repeat(100))).toBe("x".repeat(100));
  });

  it("keeps only the tail beyond the cap", () => {
    const big = "a".repeat(MAX_PROGRESS_CHARS) + "END";
    const out = appendCapped("", big);
    expect(out.length).toBe(MAX_PROGRESS_CHARS);
    expect(out.endsWith("END")).toBe(true);
    expect(out.startsWith("a")).toBe(true);
  });

  it("drops the head when appending to an already capped value", () => {
    const out = appendCapped("HEAD" + "b".repeat(MAX_PROGRESS_CHARS - 4), "TAIL");
    expect(out.length).toBe(MAX_PROGRESS_CHARS);
    expect(out.endsWith("TAIL")).toBe(true);
    expect(out.includes("HEAD")).toBe(false);
  });

  it("empty deltas are no-ops", () => {
    expect(appendCapped("abc", "")).toBe("abc");
    expect(appendCapped("abc", "", 5)).toBe("abc");
  });
});
