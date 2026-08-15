/**
 * JsonHighlight tests — VSCode-style JSON token coloring + JSON detection.
 */

import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { JsonHighlight, looksLikeJson } from "@/components/chat/JsonHighlight";

describe("JsonHighlight", () => {
  it("colors keys sky and string values green", () => {
    const { container } = render(<JsonHighlight json={'{"path": "src/main.tsx"}'} />);
    const keys = container.querySelectorAll(".text-sky-600");
    expect(keys.length).toBe(1);
    expect(keys[0].textContent).toBe('"path"');
    const strings = container.querySelectorAll(".text-green-700");
    expect(strings.length).toBe(1);
    expect(strings[0].textContent).toBe('"src/main.tsx"');
  });

  it("colors numbers amber, booleans and null purple", () => {
    const { container } = render(<JsonHighlight json='{"a": 42, "b": true, "c": null}' />);
    expect(container.querySelector(".text-amber-600")?.textContent).toBe("42");
    const purple = container.querySelectorAll(".text-purple-600");
    expect(purple.length).toBe(2);
  });

  it("leaves non-JSON text unchanged (fallback safe)", () => {
    const { container } = render(<JsonHighlight json="command not found: lss" />);
    expect(container.textContent).toBe("command not found: lss");
  });
});

describe("looksLikeJson", () => {
  it("accepts JSON objects and arrays", () => {
    expect(looksLikeJson('{"a": 1}')).toBe(true);
    expect(looksLikeJson('[1, 2, 3]')).toBe(true);
    expect(looksLikeJson('  {"a": 1}  ')).toBe(true);
  });

  it("rejects plain text and broken JSON", () => {
    expect(looksLikeJson("no json here")).toBe(false);
    expect(looksLikeJson('{"a": }')).toBe(false);
  });
});
