import { describe, it, expect } from "vitest";
import { splitFencedCode } from "@/lib/userMessageBlocks";

describe("splitFencedCode", () => {
  it("keeps plain text as one part", () => {
    expect(splitFencedCode("hello world")).toEqual([{ kind: "text", text: "hello world" }]);
  });

  it("splits a fenced code block out with its language", () => {
    const parts = splitFencedCode("before\n```ts\nconst x = 1;\n```\nafter");
    expect(parts).toEqual([
      { kind: "text", text: "before" },
      { kind: "code", text: "const x = 1;", lang: "ts" },
      { kind: "text", text: "after" },
    ]);
  });

  it("handles multiple blocks and a bare fence", () => {
    const parts = splitFencedCode("```\na\n```\nmid\n```py\nb\n```");
    expect(parts).toEqual([
      { kind: "code", text: "a", lang: undefined },
      { kind: "text", text: "mid" },
      { kind: "code", text: "b", lang: "py" },
    ]);
  });

  it("keeps an unclosed fence as plain text", () => {
    expect(splitFencedCode("```js\nconst x = 1;")).toEqual([
      { kind: "text", text: "```js\nconst x = 1;" },
    ]);
  });

  it("preserves leading/trailing text and blank lines", () => {
    const parts = splitFencedCode("a\n\n```\nx\n```\n\nb");
    expect(parts).toEqual([
      { kind: "text", text: "a\n" },
      { kind: "code", text: "x", lang: undefined },
      { kind: "text", text: "\nb" },
    ]);
  });
});
