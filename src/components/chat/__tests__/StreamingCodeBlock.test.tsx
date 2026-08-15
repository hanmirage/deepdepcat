/**
 * StreamingCodeBlock tests — the worker-highlight pending window.
 *
 * Regression guard for the "code block renders blank" bug: while the Shiki
 * worker's first highlight pass is still in flight (debounce + latency), the
 * block must fall back to the lightweight tokenizer so the code text is
 * visible immediately — never an empty <pre> that "pops" in highlighted.
 */

import { describe, it, expect, vi } from "vitest";
import { render } from "@testing-library/react";

// Simulate a browser where the worker is available but the first request
// never resolves (pending). highlightWorkerAvailable() === true sends the
// block down the worker path; highlightStreaming() returning a never-settling
// promise keeps `workerTokens === null`, which is exactly the window that
// used to render zero code spans.
vi.mock("@/lib/highlightClient", () => ({
  highlightWorkerAvailable: () => true,
  highlightStreaming: () => new Promise(() => {}),
  disposeHighlight: () => {},
}));

import { StreamingCodeBlock } from "@/components/chat/StreamingMarkdownParts";

describe("StreamingCodeBlock — worker pending", () => {
  it("renders the code text immediately via the lightweight fallback", () => {
    const { container } = render(
      <StreamingCodeBlock text="const x = 1" lang="ts" stalled={false} />,
    );

    const pre = container.querySelector("pre");
    expect(pre).not.toBeNull();
    // The code text is visible even before the worker resolves.
    expect(pre!.textContent).toContain("const x = 1");
    // And it came from the lightweight tokenizer, not a blank pass.
    expect(container.querySelector(".code-tok-keyword")?.textContent).toBe("const");
    expect(container.querySelector(".code-tok-number")?.textContent).toBe("1");
  });

  it("mirrors the completed CodeBlock chrome (header + line-number gutter)", () => {
    // Multi-line so the header shows the live line count and the gutter fills.
    const { container } = render(
      <StreamingCodeBlock text={"const a = 1\nconst b = 2"} lang="ts" stalled={false} />,
    );
    // Display name, not the raw lang — the completed block shows the same.
    expect(container.textContent).toContain("TypeScript");
    expect(container.textContent).toContain("2 lines");
    // Line-number gutter renders 1..N, so no gutter is inserted at completion.
    expect(container.textContent).toContain("1");
    expect(container.textContent).toContain("2");
    expect(container.querySelector(".border-r")).not.toBeNull();
  });
});
