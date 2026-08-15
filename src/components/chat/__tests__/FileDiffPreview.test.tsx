/**
 * FileDiffPreview hunk-windowing tests — a one-line edit in a huge file
 * must NOT render every context row (that grew the diff table unbounded
 * and blew it out of its container); the window keeps the table bounded.
 */

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { FileDiffPreview } from "@/components/chat/FileDiffPreview";

describe("FileDiffPreview hunk windowing", () => {
  it("renders the full diff for small edits", () => {
    const oldText = "line1\nline2\nline3";
    const newText = "line1\nline2 changed\nline3";
    render(
      <FileDiffPreview filePath="src/a.ts" oldText={oldText} newText={newText} />,
    );
    // Context 2 lines (line1, line3) + 1 changed line + the header.
    expect(screen.getAllByText(/line1/).length).toBeGreaterThan(0);
    expect(screen.getAllByText(/line2 changed/).length).toBe(1);
    expect(screen.getAllByText(/line3/).length).toBeGreaterThan(0);
  });

  it("windows long context runs into an omitted-count row", () => {
    // 60 unchanged lines before a one-line change: only the window (8 + 8)
    // context lines survive, plus the "⋯ N 行未显示" marker.
    const lines: string[] = [];
    for (let i = 0; i < 60; i++) lines.push(`ctx-${i}`);
    const oldText = [...lines, "the change"].join("\n");
    const newText = [...lines, "the changed"].join("\n");

    render(
      <FileDiffPreview filePath="src/big.ts" oldText={oldText} newText={newText} />,
    );

    // The omitted marker is present.
    expect(screen.getByText(/行未显示/)).toBeTruthy();
    // The window edges survive (first + last window lines).
    expect(screen.getByText(/ctx-0/)).toBeTruthy();
    expect(screen.getByText(/ctx-52/)).toBeTruthy();
    // Lines inside the omitted range are NOT rendered.
    expect(screen.queryByText(/ctx-30/)).toBeNull();
  });

  it("pure additions also window the trailing context", () => {
    const lines: string[] = [];
    for (let i = 0; i < 40; i++) lines.push(`keep-${i}`);
    const oldText = lines.join("\n");
    // Append a big block — the common prefix (40 context lines) must be
    // windowed, not rendered in full.
    const added: string[] = [];
    for (let i = 0; i < 20; i++) added.push(`added-${i}`);
    const newText = [...lines, ...added].join("\n");

    render(
      <FileDiffPreview filePath="src/pure.ts" oldText={oldText} newText={newText} />,
    );

    expect(screen.getByText(/行未显示/)).toBeTruthy();
    expect(screen.getByText(/keep-0/)).toBeTruthy();
    expect(screen.getByText(/added-19/)).toBeTruthy();
    expect(screen.queryByText(/keep-20/)).toBeNull();
  });

  it("collapses to the header on click (container never overflows)", () => {
    const oldText = "a\nb\nc";
    const newText = "a\nB\nc";
    render(
      <FileDiffPreview filePath="src/x.ts" oldText={oldText} newText={newText} />,
    );
    expect(screen.getByText(/B/)).toBeTruthy();

    // Click the header (file path) — the diff body hides.
    fireEvent.click(screen.getByText(/x\.ts/));
    expect(screen.queryByText(/B/)).toBeNull();
  });

  it("caps pathological single-line blobs (horizontal overflow guard)", () => {
    // A 5000-char line with no whitespace must never render at full width —
    // the table would stretch past the container (whitespace-pre).
    const long = "x".repeat(5000);
    const oldText = `before\n${long}\nafter`;
    const newText = `before\n${long}CHANGED\nafter`;

    render(
      <FileDiffPreview filePath="src/minified.ts" oldText={oldText} newText={newText} />,
    );

    // The capped lines are rendered with the omitted-count note (both the
    // removed and the added row carry the same 5000-char blob).
    const notes = screen.getAllByText(/完整内容已截断/);
    expect(notes.length).toBeGreaterThan(0);
    expect(notes[0].textContent).toContain("5000");
    // The full blob is NOT in the DOM (only the 400-char cap is).
    expect(screen.queryByText(new RegExp(`x{5000}`))).toBeNull();
    // The tooltip still carries the full content for copy/paste access.
    expect(notes[0].closest("span")?.getAttribute("title")?.length).toBe(5000);
  });

  it("keeps short lines intact", () => {
    const oldText = "a\nb";
    const newText = "a\nB";
    render(
      <FileDiffPreview filePath="src/short.ts" oldText={oldText} newText={newText} />,
    );
    expect(screen.getByText("B")).toBeTruthy();
    expect(screen.queryByText(/完整内容已截断/)).toBeNull();
  });
});
