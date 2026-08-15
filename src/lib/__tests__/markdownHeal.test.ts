/**
 * markdownHeal tests — closing unclosed inline markers for streaming.
 */

import { describe, it, expect } from "vitest";
import { healInline } from "@/lib/markdownHeal";

describe("healInline", () => {
  it("closes an unclosed bold with content after the marker", () => {
    expect(healInline("**加粗")).toBe("**加粗**");
  });

  it("closes unclosed em, code and del the same way", () => {
    expect(healInline("*斜体")).toBe("*斜体*");
    expect(healInline("`npm run dev")).toBe("`npm run dev`");
    expect(healInline("~~删除")).toBe("~~删除~~");
  });

  it("does NOT close a bare trailing marker with no content", () => {
    expect(healInline("使用 **")).toBe("使用 **");
    expect(healInline("**")).toBe("**");
  });

  it("keeps already-closed pairs untouched", () => {
    const text = "**加粗** 和 *斜体* 和 `代码`";
    expect(healInline(text)).toBe(text);
  });

  it("heals the marker that is still open when others are closed", () => {
    expect(healInline("**加粗** 还有 *斜")).toBe("**加粗** 还有 *斜*");
  });

  it("handles escaping-free literal text and plain lines", () => {
    expect(healInline("普通文本")).toBe("普通文本");
    expect(healInline("")).toBe("");
  });

  it("does not heal links (stays plain text until fully closed)", () => {
    expect(healInline("[文档](https://x.dev")).toBe("[文档](https://x.dev");
    expect(healInline("[文档")).toBe("[文档");
  });

  it("leaves a nested unclosed marker literal (no phantom delimiter)", () => {
    // strong opened first, em opened second — the em is nested, so only the
    // top-level strong is healed; the em's `*` stays literal rather than
    // injecting a phantom `*` that would pop away at turn_end.
    expect(healInline("**加 *斜")).toBe("**加 *斜**");
  });

  it("does not inject a phantom closer for a nested em followed by a strong run", () => {
    // `**bold *both**` — the trailing `**` closes the strong; the inner `*`
    // em is unclosed and must stay literal (healing it appends a stray `*`
    // not present in the source, which disappears at turn_end).
    expect(healInline("**bold *both**")).toBe("**bold *both**");
  });

  it("closes markers that opened after a closing pair", () => {
    expect(healInline("`a` 之后 `b")).toBe("`a` 之后 `b`");
  });
});
