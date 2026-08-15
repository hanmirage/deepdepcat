/**
 * codeHighlight tests — lightweight streaming syntax highlighting.
 */

import { describe, it, expect } from "vitest";
import { highlightTokens } from "@/lib/codeHighlight";

function classes(code: string, lang: string) {
  return highlightTokens(code, lang).filter((t) => t.className !== null);
}

describe("highlightTokens", () => {
  it("returns a single plain token for unknown languages", () => {
    const tokens = highlightTokens("const x = 1", "cobol");
    expect(tokens.length).toBe(1);
    expect(tokens[0].className).toBeNull();
    expect(tokens[0].text).toBe("const x = 1");
  });

  it("colors keywords, strings, comments and numbers in JS", () => {
    const marked = classes("const x = 'hi'; // note\nlet n = 42", "js");
    const byText = new Map(marked.map((t) => [t.text, t.className]));
    expect(byText.get("const")).toBe("code-tok-keyword");
    expect(byText.get("'hi'")).toBe("code-tok-string");
    expect(byText.get("// note")).toBe("code-tok-comment");
    expect(byText.get("42")).toBe("code-tok-number");
  });

  it("covers python keywords", () => {
    const marked = classes("def f(): return None", "py");
    const byText = new Map(marked.map((t) => [t.text, t.className]));
    expect(byText.get("def")).toBe("code-tok-keyword");
    expect(byText.get("return")).toBe("code-tok-keyword");
    expect(byText.get("None")).toBe("code-tok-keyword");
  });

  it("leaves an unterminated string uncolored (streaming-safe)", () => {
    const marked = classes("const s = 'half", "js");
    expect(marked.some((t) => t.className === "code-tok-string")).toBe(false);
    // Plain text preserved exactly.
    expect(marked.every((t) => t.className !== "code-tok-string")).toBe(true);
    const all = highlightTokens("const s = 'half", "js");
    expect(all.map((t) => t.text).join("")).toBe("const s = 'half");
  });

  it("leaves an unterminated block comment uncolored", () => {
    const marked = classes("/* not closed yet", "js");
    expect(marked.some((t) => t.className === "code-tok-comment")).toBe(false);
  });

  it("preserves the full text across tokens", () => {
    const code = "if (x > 10) { console.log('ok'); }";
    const tokens = highlightTokens(code, "js");
    expect(tokens.map((t) => t.text).join("")).toBe(code);
  });

  it("tags and attributes in html", () => {
    const marked = classes("<div class=\"card\">", "html");
    const byText = new Map(marked.map((t) => [t.text, t.className]));
    expect(byText.get("<div")).toBe("code-tok-tag");
    expect(byText.get("class")).toBe("code-tok-attr");
    expect(byText.get('"card"')).toBe("code-tok-string");
  });

  it("covers rust keywords and comments", () => {
    const marked = classes("fn main() { let x = 42; // init\n} ", "rust");
    const byText = new Map(marked.map((t) => [t.text, t.className]));
    expect(byText.get("fn")).toBe("code-tok-keyword");
    expect(byText.get("let")).toBe("code-tok-keyword");
    expect(byText.get("42")).toBe("code-tok-number");
    expect(byText.get("// init")).toBe("code-tok-comment");
  });

  it("covers go keywords", () => {
    const marked = classes("package main\nfunc add(a int) int { return a + 1 }", "go");
    const byText = new Map(marked.map((t) => [t.text, t.className]));
    expect(byText.get("package")).toBe("code-tok-keyword");
    expect(byText.get("func")).toBe("code-tok-keyword");
    expect(byText.get("return")).toBe("code-tok-keyword");
  });

  it("covers c++ keywords and block comments", () => {
    const marked = classes("int x = 5; /* note */", "cpp");
    const byText = new Map(marked.map((t) => [t.text, t.className]));
    expect(byText.get("int")).toBe("code-tok-keyword");
    expect(byText.get("5")).toBe("code-tok-number");
    expect(byText.get("/* note */")).toBe("code-tok-comment");
  });

  it("covers ruby hash comments", () => {
    const marked = classes("def f\n  x = 1 # note\nend", "ruby");
    const byText = new Map(marked.map((t) => [t.text, t.className]));
    expect(byText.get("def")).toBe("code-tok-keyword");
    expect(byText.get("end")).toBe("code-tok-keyword");
    expect(byText.get("# note")).toBe("code-tok-comment");
  });

  it("sh highlights numbers too", () => {
    const marked = classes("echo 42", "sh");
    const byText = new Map(marked.map((t) => [t.text, t.className]));
    expect(byText.get("echo")).toBe("code-tok-keyword");
    expect(byText.get("42")).toBe("code-tok-number");
  });

  it("empty code yields no tokens", () => {
    expect(highlightTokens("", "js")).toEqual([]);
  });
});
