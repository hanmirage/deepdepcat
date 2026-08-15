/**
 * AnsiText tests — ANSI SGR coloring for streamed terminal output.
 */

import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { AnsiText, stripAnsi } from "@/components/chat/AnsiText";

const GREEN = "\x1b[32m";
const RED = "\x1b[31m";
const RESET = "\x1b[0m";

describe("AnsiText", () => {
  it("renders plain text as a single uncolored span", () => {
    const { container } = render(<AnsiText text="hello world" />);
    expect(container.textContent).toBe("hello world");
    expect(container.querySelectorAll("span").length).toBe(1);
    expect(container.querySelector("span")?.className).toBe("");
  });

  it("colors green runs and resets on \\x1b[0m", () => {
    const { container } = render(<AnsiText text={`${GREEN}ok${RESET} plain`} />);
    const green = container.querySelector(".text-emerald-600");
    expect(green?.textContent).toBe("ok");
    expect(container.textContent).toBe("ok plain");
  });

  it("handles multiple color codes and bold", () => {
    const { container } = render(
      <AnsiText text={`${RED}err${RESET} ${GREEN}ok${RESET}`} />,
    );
    expect(container.querySelector(".text-red-500")?.textContent).toBe("err");
    expect(container.querySelector(".text-emerald-600")?.textContent).toBe("ok");

    const bold = render(<AnsiText text={`\x1b[1;33mnote${RESET}`} />);
    const note = bold.container.querySelector(".text-amber-600");
    expect(note?.textContent).toBe("note");
    expect(note?.className).toContain("font-semibold");
  });

  it("strips unsupported 256-color sequences without corrupting output", () => {
    const { container } = render(<AnsiText text={`\x1b[38;5;196mred-ish\x1b[0m tail`} />);
    // 256-color is unsupported — the run stays uncolored, output intact.
    expect(container.textContent).toBe("red-ish tail");
  });
});

describe("stripAnsi", () => {
  it("removes all SGR escapes", () => {
    expect(stripAnsi(`${GREEN}ok${RESET}`)).toBe("ok");
    expect(stripAnsi("no codes")).toBe("no codes");
  });
});
