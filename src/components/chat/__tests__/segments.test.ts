import { describe, it, expect } from "vitest";
import { buildSegments } from "../segments";
import type { MessageBlock } from "@/types";

function tool(id: string, name: string, status: "running" | "done" | "error" = "done"): MessageBlock {
  return {
    type: "tool_call",
    tool: {
      id,
      name,
      arguments: "{}",
      status,
      startedAt: 0,
    },
  };
}

const BLOCKS: MessageBlock[] = [
  { type: "text", content: "checking…" },
  tool("r1", "read_file", "running"),
  tool("r2", "read_file"),
  tool("w1", "edit_file"),
  { type: "text", content: "done" },
];

describe("buildSegments", () => {
  it("keeps tool rows and read groups inline in the narrative", () => {
    const segments = buildSegments(BLOCKS);
    expect(segments.map((s) => s.kind)).toEqual([
      "block",
      "readGroup",
      "tool",
      "block",
    ]);
    const group = segments[1];
    if (group.kind !== "readGroup") throw new Error("expected readGroup");
    expect(group.tools.map((t) => t.id)).toEqual(["r1", "r2"]);
  });

  it("keeps ask_user inline and other tools as bare rows", () => {
    const blocks: MessageBlock[] = [
      tool("e1", "bash", "error"),
      tool("a1", "ask_user"),
      { type: "text", content: "?" },
    ];
    const segments = buildSegments(blocks);
    expect(segments.map((s) => s.kind)).toEqual(["tool", "block", "block"]);
  });

  it("folds adjacent same-batch non-read tools into a parallel group", () => {
    const blocks: MessageBlock[] = [
      { type: "tool_call", tool: { id: "p1", name: "bash", arguments: "{}", status: "done", startedAt: 0, parallelBatch: 0 } },
      { type: "tool_call", tool: { id: "p2", name: "edit_file", arguments: "{}", status: "done", startedAt: 0, parallelBatch: 0 } },
      { type: "tool_call", tool: { id: "s1", name: "bash", arguments: "{}", status: "done", startedAt: 0, parallelBatch: 1 } },
    ];
    const segments = buildSegments(blocks);
    expect(segments.map((s) => s.kind)).toEqual(["parallelGroup", "tool"]);
    const group = segments[0];
    if (group.kind !== "parallelGroup") throw new Error("expected parallelGroup");
    expect(group.tools.map((t) => t.id)).toEqual(["p1", "p2"]);
  });

  it("does not fold tools without a batch id (restored history)", () => {
    const blocks: MessageBlock[] = [
      { type: "tool_call", tool: { id: "x1", name: "bash", arguments: "{}", status: "done", startedAt: 0 } },
      { type: "tool_call", tool: { id: "x2", name: "bash", arguments: "{}", status: "done", startedAt: 0 } },
    ];
    const segments = buildSegments(blocks);
    expect(segments.map((s) => s.kind)).toEqual(["tool", "tool"]);
  });
});
