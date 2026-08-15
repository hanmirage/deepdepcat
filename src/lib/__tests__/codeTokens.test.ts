/**
 * codeTokens tests — Shiki token flattening helpers.
 */

import { describe, it, expect } from "vitest";
import { flattenShikiTokens } from "@/lib/codeTokens";

describe("flattenShikiTokens", () => {
  it("flattens lines and re-adds newlines between them", () => {
    const lines = [
      [{ content: "const", color: "#569cd6" }, { content: " x = 1" }],
      [{ content: "let", color: "#569cd6" }, { content: " y" }],
    ];
    const tokens = flattenShikiTokens(lines);
    expect(tokens).toEqual([
      { text: "const", color: "#569cd6" },
      { text: " x = 1\n", color: undefined },
      { text: "let", color: "#569cd6" },
      { text: " y\n", color: undefined },
    ]);
  });

  it("keeps empty lines as newline tokens", () => {
    const lines = [[], [{ content: "x" }]];
    const tokens = flattenShikiTokens(lines);
    expect(tokens).toEqual([{ text: "\n", color: undefined }, { text: "x\n", color: undefined }]);
  });

  it("empty input yields no tokens", () => {
    expect(flattenShikiTokens([])).toEqual([]);
  });
});
