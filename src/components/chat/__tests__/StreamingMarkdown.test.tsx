/**
 * StreamingMarkdown tests — row-level incremental rendering with healing.
 *
 * Covers the streaming-markdown invariants:
 *  - closed inline markers render WHILE streaming (bold is bold)
 *  - unclosed markers with content render bold immediately (healing —
 *    no plain-text asterisks waiting for the closing pair)
 *  - a bare trailing marker (still being typed) stays literal
 *  - row-leading formats (heading/list/quote) render live
 *  - fenced code opens a code container; content streams inside it with
 *    lightweight syntax highlighting
 *  - turn_end keeps completed blocks untouched (zero-pop)
 *
 * NOTE: streaming content must be passed as a JS variable (expression
 * prop), never as a JSX string-literal attribute — esbuild does not decode
 * escape sequences in JSX attribute string literals (`\n` stays literal).
 */

import { describe, it, expect, afterEach, vi } from "vitest";
import { render, screen, act } from "@testing-library/react";
import { StreamingMarkdown } from "@/components/chat/StreamingMarkdown";

function renderStreaming(content: string, isStreaming = true) {
  return render(<StreamingMarkdown content={content} isStreaming={isStreaming} />);
}

afterEach(() => {
  vi.useRealTimers();
});

describe("StreamingMarkdown — inline formats", () => {
  it("renders closed bold as <strong> while streaming", () => {
    renderStreaming("使用 **重要** 功能");
    expect(screen.getByText("重要").tagName).toBe("STRONG");
  });

  it("renders an unclosed bold WITH content as <strong> immediately (healed)", () => {
    const { container } = renderStreaming("使用 **重要");
    const strong = container.querySelector("strong");
    expect(strong).not.toBeNull();
    expect(strong!.textContent).toBe("重要");
  });

  it("a bare trailing marker stays literal (still being typed)", () => {
    const { container } = renderStreaming("使用 **");
    expect(container.querySelector("strong")).toBeNull();
    expect(container.textContent).toContain("**");
  });

  it("closing a bold marker keeps the same rendered form (no pop)", () => {
    const { container, rerender } = render(
      <StreamingMarkdown content="使用 **重要" isStreaming />,
    );
    const strong = container.querySelector("strong");
    expect(strong).not.toBeNull();
    rerender(<StreamingMarkdown content="使用 **重要** 功能" isStreaming />);
    expect(screen.getByText("重要").tagName).toBe("STRONG");
  });

  it("renders inline code, em and links", () => {
    renderStreaming("跑 `npm run dev`，*注意* [文档](https://x.dev)");
    expect(screen.getByText("npm run dev").tagName).toBe("CODE");
    expect(screen.getByText("注意").tagName).toBe("EM");
    const link = screen.getByText("文档");
    expect(link.tagName).toBe("A");
    expect(link.getAttribute("href")).toBe("https://x.dev");
  });

  it("paces a large burst growth across ticks", () => {
    vi.useFakeTimers();
    const { container, rerender } = render(
      <StreamingMarkdown content="开头" isStreaming />,
    );
    rerender(<StreamingMarkdown content={`开头${"续".repeat(600)}`} isStreaming />);
    // The stream can burst large increments; the frontend reveals them
    // progressively — no block jump, then it catches up.
    expect(container.textContent!.length).toBeLessThan(602);
    act(() => vi.advanceTimersByTime(1000));
    expect(container.textContent).toContain("续".repeat(600));
  });

  it("typing continues seamlessly ACROSS newlines (block-level reveal)", () => {
    vi.useFakeTimers();
    const { container, rerender } = render(
      <StreamingMarkdown content="第一行" isStreaming />,
    );
    rerender(<StreamingMarkdown content={`第一行\n第二行${"字".repeat(600)}`} isStreaming />);
    const partial = container.textContent!;
    expect(partial).toContain("第一行");
    // Mid-reveal: the second line is only partially typed.
    expect(partial).not.toContain(`第二行${"字".repeat(600)}`);
    act(() => vi.advanceTimersByTime(1000));
    expect(container.textContent).toContain(`第二行${"字".repeat(600)}`);
  });
});

describe("StreamingMarkdown — row formats", () => {
  it("renders headings live", () => {
    const { container } = renderStreaming("## 标题行");
    const h = container.querySelector("div.font-bold");
    expect(h).not.toBeNull();
    expect(h!.textContent).toContain("标题行");
  });

  it("renders list items with a marker", () => {
    const { container } = renderStreaming("- 第一条\n- 第二条");
    const lines = container.querySelectorAll("div.flex.items-baseline");
    expect(lines.length).toBe(2);
    expect(container.textContent).toContain("•");
    expect(container.textContent).toContain("第一条");
    expect(container.textContent).toContain("第二条");
  });

  it("renders task rows as real checkboxes (checked / unchecked)", () => {
    const { container } = renderStreaming("- [x] 已完成\n- [ ] 待办");
    // Two checkbox boxes (the streaming cursor also carries aria-hidden —
    // exclude it), and exactly one check mark (the checked box).
    const boxes = container.querySelectorAll("span[aria-hidden]:not(.streaming-cursor)");
    expect(boxes.length).toBe(2);
    expect(container.querySelectorAll("svg").length).toBe(1);
  });

  it("indents nested list rows by their leading spaces", () => {
    const { container } = renderStreaming("- 一级\n - 二级");
    const rows = container.querySelectorAll("div.flex.items-baseline");
    expect(rows.length).toBe(2);
    const indent = rows[1].getAttribute("style") ?? "";
    expect(indent).toContain("padding-left: 14px");
  });

  it("renders a completed `---` as a hairline divider", () => {
    const { container } = renderStreaming("上面\n---\n下面");
    const divider = container.querySelector("div.h-px");
    expect(divider).not.toBeNull();
  });

  it("renders blockquotes with a left border", () => {
    const { container } = renderStreaming("> 引用内容");
    const quote = container.querySelector("div.border-l-2");
    expect(quote).not.toBeNull();
    expect(quote!.textContent).toContain("引用内容");
  });

  it("half-typed list marker is plain until the space arrives", () => {
    const { container, rerender } = render(
      <StreamingMarkdown content="-列表" isStreaming />,
    );
    expect(container.querySelector("div.flex.items-baseline")).toBeNull();
    rerender(<StreamingMarkdown content="- 列表" isStreaming />);
    expect(container.querySelector("div.flex.items-baseline")).not.toBeNull();
  });
});

describe("StreamingMarkdown — code fences", () => {
  it("opens a code container once the fence starts", () => {
    const { container } = renderStreaming("代码如下：\n```ts\nconst x");
    const pre = container.querySelector("pre");
    expect(pre).not.toBeNull();
    expect(pre!.textContent).toContain("const x");
    // Header shows the display name (matches the completed CodeBlock).
    expect(container.textContent).toContain("TypeScript");
  });

  it("streams code inside the container with a live cursor", () => {
    const first = "```\nfn main";
    const second = "```\nfn main() {}";
    const { container, rerender } = render(
      <StreamingMarkdown content={first} isStreaming />,
    );
    expect(container.querySelector(".streaming-cursor")).not.toBeNull();
    rerender(<StreamingMarkdown content={second} isStreaming />);
    expect(container.querySelector("pre")!.textContent).toContain("fn main() {}");
  });

  it("highlights keywords while the code streams", () => {
    const { container } = renderStreaming("```ts\nconst x = 1");
    const kw = container.querySelector(".code-tok-keyword");
    expect(kw).not.toBeNull();
    expect(kw!.textContent).toBe("const");
    const num = container.querySelector(".code-tok-number");
    expect(num).not.toBeNull();
    expect(num!.textContent).toBe("1");
  });

  it("returns to markdown rows after the fence closes", () => {
    const { container } = renderStreaming("```\ncode\n```\n# 之后是标题");
    expect(container.querySelector("pre")).not.toBeNull();
    expect(container.querySelector("div.font-bold")).not.toBeNull();
  });
});

describe("StreamingMarkdown — oversized tails", () => {
  it("hard-splits a single unbounded line (no line breaks) so parsing stays bounded", () => {
    vi.useFakeTimers();
    const big = "A".repeat(7000);
    const { container, rerender } = render(
      <StreamingMarkdown content="" isStreaming />,
    );
    rerender(<StreamingMarkdown content={big} isStreaming />);
    // The reveal types 32 chars/tick; once the revealed prefix crosses the
    // splitter's active-block cap (~6000 chars) the completed prefix becomes
    // a MarkdownRenderer block and the active block continues — per-tick
    // work stays bounded either way.
    act(() => vi.advanceTimersByTime(5000));
    expect(container.querySelectorAll("div.prose").length).toBeGreaterThanOrEqual(1);
    act(() => vi.advanceTimersByTime(3000));
    expect(container.textContent).toContain(big);
  });

  it("bounds a giant in-fence line (content starts with a fence)", () => {
    vi.useFakeTimers();
    const big = "```ts\n" + "C".repeat(6500);
    const { container, rerender } = render(
      <StreamingMarkdown content="" isStreaming />,
    );
    rerender(<StreamingMarkdown content={big} isStreaming />);
    // A code container appears once the fence is revealed.
    act(() => vi.advanceTimersByTime(500));
    expect(container.querySelector("pre")).not.toBeNull();
    // The in-fence line is split across two blocks once the reveal crosses
    // the active-block cap; the FULL text only lands contiguously at
    // turn_end's finalize pass (after the reveal catches up).
    rerender(<StreamingMarkdown content={big} isStreaming={false} />);
    act(() => vi.advanceTimersByTime(6000));
    // The split survives finalize (zero-pop): completed prefix + final block
    // together hold every character — the full run is never lost. (The
    // "Copy" button label contributes one capital C — strip it.)
    expect(container.querySelectorAll("div.prose").length).toBeGreaterThanOrEqual(1);
    const codeText = (container.textContent ?? "").replace(/Copy/g, "");
    expect((codeText.match(/C/g) ?? []).length).toBe(6500);
  });
});

describe("StreamingMarkdown — completed pass", () => {
  it("renders full markdown when the stream ends", () => {
    renderStreaming("**加粗** 和 `代码`", false);
    expect(screen.getByText("加粗").tagName).toBe("STRONG");
    expect(screen.getByText("代码").tagName).toBe("CODE");
  });

  it("turn_end leaves completed blocks untouched (zero-pop)", () => {
    vi.useFakeTimers();
    // >200 chars so the split kicks in: block1 completes during streaming.
    const long =
      `${"第一段内容".repeat(42)} **粗体**\n\n第二段 \`代码\` 结尾`;
    const { container, rerender } = render(
      <StreamingMarkdown content={long} isStreaming />,
    );
    // Advance past the \n\n so the first paragraph completes into a block
    // (the reveal — not the raw split — decides when a block is done now).
    act(() => vi.advanceTimersByTime(500));
    const before = Array.from(container.querySelectorAll("div.prose"));
    expect(before.length).toBeGreaterThanOrEqual(1);

    rerender(<StreamingMarkdown content={long} isStreaming={false} />);
    // The reveal is already caught up, so the final block joins as one more
    // MarkdownRenderer pass…
    act(() => vi.advanceTimersByTime(100));
    const after = Array.from(container.querySelectorAll("div.prose"));
    expect(after.length).toBe(before.length + 1);
    // …and the completed blocks keep their exact DOM nodes — no rebuild, no pop.
    for (let i = 0; i < before.length; i++) {
      expect(after[i]).toBe(before[i]);
    }
  });
});
