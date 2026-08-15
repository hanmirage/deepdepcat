/**
 * MarkdownRenderer security tests — raw HTML passthrough (rehype-raw) must
 * be sanitized (rehype-sanitize) before anything reaches the DOM.
 */

import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { MarkdownRenderer } from "@/components/chat/MarkdownRenderer";

describe("MarkdownRenderer sanitization", () => {
  it("strips event handlers from raw HTML", () => {
    const { container } = render(
      <MarkdownRenderer content={'<img src="x" onerror="alert(1)">'} />,
    );
    const img = container.querySelector("img");
    expect(img).not.toBeNull();
    expect(img?.getAttribute("onerror")).toBeNull();
  });

  it("drops script and iframe elements entirely", () => {
    const { container } = render(
      <MarkdownRenderer
        content={'<script>alert(1)</script><iframe src="https://evil.example"></iframe>'}
      />,
    );
    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("iframe")).toBeNull();
  });

  it("sanitizes javascript: links", () => {
    const { container } = render(
      <MarkdownRenderer content="[click](javascript:alert(1))" />,
    );
    expect(container.innerHTML).not.toContain("javascript:");
  });

  it("keeps legitimate formatting intact", () => {
    const { container } = render(
      <MarkdownRenderer content="**bold** and `code`" />,
    );
    expect(container.querySelector("strong")?.textContent).toBe("bold");
    expect(container.querySelector("code")?.textContent).toBe("code");
  });

  it("keeps external links safe with noopener", () => {
    const { container } = render(
      <MarkdownRenderer content="[docs](https://example.com)" />,
    );
    const link = container.querySelector("a");
    expect(link?.getAttribute("href")).toBe("https://example.com");
    expect(link?.getAttribute("rel")).toContain("noopener");
  });

  it("keeps syntax-highlight classes on code blocks", () => {
    const { container } = render(
      <MarkdownRenderer content={"```js\nconst x = 1;\n```"} />,
    );
    const code = container.querySelector("pre code");
    expect(code?.className).toContain("hljs");
    expect(code?.className).toContain("language-js");
  });
});
