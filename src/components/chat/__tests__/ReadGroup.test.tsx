/**
 * ReadGroup tests — the collapsed read-tool aggregate row.
 *
 * Groups stay expandable while a member runs: each member keeps its own
 * per-tool details lock, so peeking at which file is being read is safe.
 */

import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ReadGroup } from "@/components/chat/ReadGroup";
import type { ToolCallState } from "@/types";

function readTool(id: string, status: ToolCallState["status"]): ToolCallState {
  return {
    id,
    name: "read_file",
    arguments: JSON.stringify({ path: "src/a.ts" }),
    status,
    result: status === "done" ? "ok" : undefined,
    startedAt: Date.now() - 42_000,
  };
}

describe("ReadGroup", () => {
  it("expands while a member is running to reveal which file is read", () => {
    render(<ReadGroup tools={[readTool("r1", "running")]} />);
    fireEvent.click(screen.getByRole("button", { name: "Read group" }));
    expect(screen.getByText("读取中")).toBeInTheDocument();
  });

  it("collapses a done group into the aggregate verb", () => {
    render(<ReadGroup tools={[readTool("r1", "done"), readTool("r2", "done")]} />);
    expect(screen.getByText("已读取 2 项")).toBeInTheDocument();
  });

  it("collapsed group is a bare row — no card container", () => {
    render(<ReadGroup tools={[readTool("r1", "done")]} />);
    const btn = screen.getByRole("button", { name: "Read group" });
    // Matches the bare ToolCallCard contract — no rounded card chrome.
    expect(btn.className).not.toContain("rounded-lg");
    expect(btn.className).not.toContain("border-border");
  });
});
