/**
 * inlineMarkdown tests — streaming-safe inline markdown parsing.
 *
 * Invariants:
 *  - closed markers produce styled tokens (strong/em/del/code/link)
 *  - unclosed markers stop the scan: everything after is one text tail with
 *    hasTail=true (never a flash mid-typing)
 *  - row-leading formats (heading/bullet/ordered/quote) are detected with
 *    their strip widths
 */

import { describe, it, expect } from "vitest";
import {
  parseInline,
  leadingFormat,
  leadingFormatWidth,
} from "@/lib/inlineMarkdown";

describe("parseInline", () => {
  it("leaves plain text untouched", () => {
    const { tokens, hasTail } = parseInline("hello world");
    expect(tokens).toEqual([{ type: "text", text: "hello world" }]);
    expect(hasTail).toBe(false);
  });

  it("parses closed strong markers", () => {
    const { tokens } = parseInline("a **bold** b");
    expect(tokens).toEqual([
      { type: "text", text: "a " },
      { type: "strong", text: "bold" },
      { type: "text", text: " b" },
    ]);
  });

  it("parses em, del and inline code", () => {
    const { tokens } = parseInline("*it* ~~gone~~ `code`");
    expect(tokens).toEqual([
      { type: "em", text: "it" },
      { type: "text", text: " " },
      { type: "del", text: "gone" },
      { type: "text", text: " " },
      { type: "code", text: "code" },
    ]);
  });

  it("parses links with href", () => {
    const { tokens } = parseInline("[docs](https://example.com) here");
    expect(tokens).toEqual([
      { type: "link", text: "docs", href: "https://example.com" },
      { type: "text", text: " here" },
    ]);
  });

  it("renders file-path inline code as a file reference", () => {
    expect(parseInline("`src/main.ts`").tokens).toEqual([
      { type: "file", text: "src/main.ts", path: "src/main.ts" },
    ]);
    expect(parseInline("`D:\\docs\\report.docx`").tokens).toEqual([
      { type: "file", text: "D:\\docs\\report.docx", path: "D:\\docs\\report.docx" },
    ]);
    expect(parseInline("`main.rs:42`").tokens).toEqual([
      { type: "file", text: "main.rs:42", path: "main.rs:42" },
    ]);
  });

  it("keeps non-path inline code as a plain code token", () => {
    expect(parseInline("`npm`").tokens).toEqual([{ type: "code", text: "npm" }]);
    expect(parseInline("`foo.bar()`").tokens).toEqual([
      { type: "code", text: "foo.bar()" },
    ]);
    expect(parseInline("`\\n`").tokens).toEqual([{ type: "code", text: "\\n" }]);
  });

  it("autolinks bare https URLs as they stream", () => {
    expect(parseInline("see https://example.com/a").tokens).toEqual([
      { type: "text", text: "see " },
      { type: "link", text: "https://example.com/a", href: "https://example.com/a" },
    ]);
  });

  it("trims trailing sentence punctuation off a bare URL", () => {
    expect(parseInline("https://example.com/x.").tokens).toEqual([
      { type: "link", text: "https://example.com/x", href: "https://example.com/x" },
      { type: "text", text: "." },
    ]);
  });

  it("allows http(s), mailto and relative hrefs", () => {
    expect(parseInline("[a](http://x.com)").tokens).toEqual([
      { type: "link", text: "a", href: "http://x.com" },
    ]);
    expect(parseInline("[a](mailto:hi@x.com)").tokens).toEqual([
      { type: "link", text: "a", href: "mailto:hi@x.com" },
    ]);
    expect(parseInline("[a](/docs/guide)").tokens).toEqual([
      { type: "link", text: "a", href: "/docs/guide" },
    ]);
    expect(parseInline("[a](#section)").tokens).toEqual([
      { type: "link", text: "a", href: "#section" },
    ]);
  });

  it("rejects dangerous link schemes as plain text", () => {
    expect(parseInline("[x](javascript:alert(1))").tokens).toEqual([
      { type: "text", text: "[x](javascript:alert(1))" },
    ]);
    expect(parseInline("[x](data:text/html,<script>1</script>)").tokens).toEqual([
      { type: "text", text: "[x](data:text/html,<script>1</script>)" },
    ]);
    expect(parseInline("[x](vbscript:msgbox(1))").tokens).toEqual([
      { type: "text", text: "[x](vbscript:msgbox(1))" },
    ]);
    expect(parseInline("[x](file:///etc/passwd)").tokens).toEqual([
      { type: "text", text: "[x](file:///etc/passwd)" },
    ]);
  });

  it("unclosed strong stays a plain tail with hasTail", () => {
    const { tokens, hasTail } = parseInline("a **bold");
    expect(hasTail).toBe(true);
    // The tail starts at the unclosed marker scan position — everything
    // from there streams through the typewriter (append continuity).
    expect(tokens).toEqual([{ type: "text", text: "a **bold" }]);
  });

  it("closing a previously unclosed marker promotes the tail", () => {
    const unclosed = parseInline("**bo");
    expect(unclosed.hasTail).toBe(true);
    const closed = parseInline("**bold**");
    expect(closed.hasTail).toBe(false);
    expect(closed.tokens).toEqual([{ type: "strong", text: "bold" }]);
  });

  it("a half-typed link is a tail, not a flash", () => {
    const { tokens, hasTail } = parseInline("see [docs](https://exa");
    expect(hasTail).toBe(true);
    expect(tokens).toEqual([{ type: "text", text: "see [docs](https://exa" }]);
  });

  it("mixed markers scan left to right", () => {
    const { tokens } = parseInline("**a** and `b` and *c*");
    expect(tokens).toEqual([
      { type: "strong", text: "a" },
      { type: "text", text: " and " },
      { type: "code", text: "b" },
      { type: "text", text: " and " },
      { type: "em", text: "c" },
    ]);
  });

  it("snake_case is NOT treated as emphasis", () => {
    const { tokens, hasTail } = parseInline("foo_bar_baz");
    expect(hasTail).toBe(false);
    expect(tokens).toEqual([{ type: "text", text: "foo_bar_baz" }]);
  });
});

describe("leadingFormat", () => {
  it("detects headings with level", () => {
    expect(leadingFormat("## Title")).toEqual({ kind: "heading", level: 2 });
    expect(leadingFormat("###### Deep")).toEqual({ kind: "heading", level: 6 });
  });

  it("detects bullets and ordered lists", () => {
    expect(leadingFormat("- item")).toEqual({ kind: "bullet", marker: "•", indent: 0 });
    expect(leadingFormat("* item")).toEqual({ kind: "bullet", marker: "•", indent: 0 });
    expect(leadingFormat("3. step")).toEqual({ kind: "ordered", marker: "3.", indent: 0 });
    // Nested rows carry their indentation level.
    expect(leadingFormat("  - nested")).toEqual({ kind: "bullet", marker: "•", indent: 2 });
  });

  it("detects task rows before the bullet pattern (both win over bullets)", () => {
    expect(leadingFormat("- [x] done")).toEqual({ kind: "task", checked: true, indent: 0 });
    expect(leadingFormat("- [ ] todo")).toEqual({ kind: "task", checked: false, indent: 0 });
    expect(leadingFormat("- [X] done")).toEqual({ kind: "task", checked: true, indent: 0 });
    // Half-typed checkbox stays a bullet (no flash before the `] ` arrives).
    expect(leadingFormat("- [x done")).toEqual({ kind: "bullet", marker: "•", indent: 0 });
  });

  it("detects a completed `---` as a divider", () => {
    expect(leadingFormat("---")).toEqual({ kind: "hr" });
    expect(leadingFormat("___\n")).toEqual({ kind: "hr" });
    expect(leadingFormat("--")).toEqual({ kind: "plain" });
  });

  it("detects table rows and separator rows", () => {
    expect(leadingFormat("| 文件 | 状态 |")).toEqual({ kind: "table" });
    expect(leadingFormat("| a |")).toEqual({ kind: "table" });
    expect(leadingFormat("|---|---|")).toEqual({ kind: "table-sep" });
    expect(leadingFormat("|:--|--:|")).toEqual({ kind: "table-sep" });
    // A data row with dashes inside is still a data row, not a separator.
    expect(leadingFormat("| a-b | c |")).toEqual({ kind: "table" });
  });

  it("detects quotes", () => {
    expect(leadingFormat("> note")).toEqual({ kind: "quote" });
  });

  it("plain otherwise", () => {
    expect(leadingFormat("normal line")).toEqual({ kind: "plain" });
    // half-typed markers are plain (no flash before the space arrives)
    expect(leadingFormat("-item")).toEqual({ kind: "plain" });
    expect(leadingFormat("1.")).toEqual({ kind: "plain" });
    expect(leadingFormat("#no-space")).toEqual({ kind: "plain" });
  });
});

describe("leadingFormatWidth", () => {
  it("returns the strip width per format", () => {
    expect(leadingFormatWidth("### H")).toBe(4);
    expect(leadingFormatWidth("- x")).toBe(2);
    expect(leadingFormatWidth("12. x")).toBe(4);
    expect(leadingFormatWidth("> q")).toBe(2);
    expect(leadingFormatWidth("plain")).toBe(0);
  });
});
