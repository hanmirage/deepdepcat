import { describe, it, expect, beforeEach } from "vitest";
import type { ChatStreamEvent, StreamEventShape } from "@/lib/tauri";
import {
  createWorkspace,
  reduceWorkspace,
  workspaceBlocks,
  mergeWorkspaceBlocks,
  type TurnWorkspace,
} from "../workspace";
import type { MessageBlock } from "@/types";

let seq = 0;
function evt(body: StreamEventShape): ChatStreamEvent {
  seq += 1;
  return { seq, ...body };
}

function textBlocks(blocks: MessageBlock[]): string[] {
  return blocks.filter((b) => b.type === "text").map((b) => (b.type === "text" ? b.content : ""));
}

describe("stream workspace reducer", () => {
  beforeEach(() => {
    seq = 0;
  });

  it("merges consecutive text deltas into one block", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(ws, evt({ type: "text_delta", turn_id: "t1", text: "hel" }));
    ws = reduceWorkspace(ws, evt({ type: "text_delta", turn_id: "t1", text: "lo" }));
    const blocks = workspaceBlocks(ws);
    expect(blocks).toHaveLength(1);
    if (blocks[0].type !== "text") throw new Error("expected text");
    expect(blocks[0].content).toBe("hello");
  });

  it("keeps interleaved order: text → tool → text creates two text segments", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(ws, evt({ type: "text_delta", turn_id: "t1", text: "first" }));
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_start", turn_id: "t1", call_id: "c1", name: "grep" }),
    );
    ws = reduceWorkspace(ws, evt({ type: "text_delta", turn_id: "t1", text: "second" }));
    const blocks = workspaceBlocks(ws);
    expect(blocks.map((b) => b.type)).toEqual(["text", "tool_call", "text"]);
    expect(textBlocks(blocks)).toEqual(["first", "second"]);
  });

  it("inserts late reasoning ahead of the first text segment", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(ws, evt({ type: "text_delta", turn_id: "t1", text: "answer" }));
    ws = reduceWorkspace(
      ws,
      evt({ type: "reasoning_delta", turn_id: "t1", text: "think" }),
    );
    const blocks = workspaceBlocks(ws);
    expect(blocks.map((b) => b.type)).toEqual(["reasoning", "text"]);
  });

  it("keeps parallel tool rows in stream-declaration order (batch stability)", () => {
    let ws = createWorkspace("t1");
    for (const [id, name] of [
      ["a", "read_file"],
      ["b", "grep"],
      ["c", "glob"],
    ] as const) {
      ws = reduceWorkspace(
        ws,
        evt({ type: "tool_call_start", turn_id: "t1", call_id: id, name }),
      );
    }
    // Results complete out of order — rows must NOT reorder.
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_result", turn_id: "t1", call_id: "c", name: "glob", result: "r", is_error: false }),
    );
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_result", turn_id: "t1", call_id: "a", name: "read_file", result: "r", is_error: false }),
    );
    const blocks = workspaceBlocks(ws);
    const tools = blocks
      .filter((b) => b.type === "tool_call")
      .map((b) => (b.type === "tool_call" ? b.tool.id : ""));
    expect(tools).toEqual(["a", "b", "c"]);
  });

  it("assigns one batch to overlapping tools and a fresh batch after they drain", () => {
    let ws = createWorkspace("t1");
    // Two tools start while nothing else runs → same batch 0 (parallel).
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_start", turn_id: "t1", call_id: "a", name: "bash" }),
    );
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_start", turn_id: "t1", call_id: "b", name: "bash" }),
    );
    // Both complete → the batch drains to 1.
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_result", turn_id: "t1", call_id: "a", name: "bash", result: "ok", is_error: false }),
    );
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_result", turn_id: "t1", call_id: "b", name: "bash", result: "ok", is_error: false }),
    );
    // The next tool starts a fresh batch.
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_start", turn_id: "t1", call_id: "c", name: "bash" }),
    );
    const batches = workspaceBlocks(ws)
      .filter((b) => b.type === "tool_call")
      .map((b) => (b.type === "tool_call" ? b.tool.parallelBatch : null));
    expect(batches).toEqual([0, 0, 1]);
  });

  it("applies the overlap trim once per delta", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(ws, evt({ type: "text_delta", turn_id: "t1", text: "hello world" }));
    ws = reduceWorkspace(
      ws,
      evt({ type: "text_delta", turn_id: "t1", text: "hello world again" }),
    );
    const blocks = workspaceBlocks(ws);
    if (blocks[0].type !== "text") throw new Error("expected text");
    expect(blocks[0].content).toBe("hello world again");
  });

  it("sums usage cumulatively", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(
      ws,
      evt({
        type: "usage",
        turn_id: "t1",
        usage: { prompt_tokens: 10, completion_tokens: 5 },
      }),
    );
    ws = reduceWorkspace(
      ws,
      evt({
        type: "usage",
        turn_id: "t1",
        usage: { prompt_tokens: 7, completion_tokens: 3 },
      }),
    );
    expect(ws.usage?.prompt_tokens).toBe(17);
    expect(ws.usage?.completion_tokens).toBe(8);
  });

  it("mergeWorkspaceBlocks patches stream blocks by identity, keeps injected blocks", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(ws, evt({ type: "text_delta", turn_id: "t1", text: "hi" }));
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_start", turn_id: "t1", call_id: "c1", name: "grep" }),
    );
    const existing: MessageBlock[] = [
      ...workspaceBlocks(ws),
      // Injected narrative — no streamId, must survive the next commit.
      { type: "text", content: "\n✓ 子代理完成\n" },
    ];
    ws = reduceWorkspace(ws, evt({ type: "text_delta", turn_id: "t1", text: " world" }));
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_result", turn_id: "t1", call_id: "c1", name: "grep", result: "ok", is_error: false }),
    );
    const merged = mergeWorkspaceBlocks(ws, existing);
    // " world" arrives AFTER the tool row AND after the narrative was
    // injected → its own text segment, appended after the narrative
    // (chronological order), which survives untouched (no streamId).
    expect(merged.map((b) => b.type)).toEqual(["text", "tool_call", "text", "text"]);
    expect(textBlocks(merged)).toEqual(["hi", "\n✓ 子代理完成\n", " world"]);
    const tool = merged.find((b) => b.type === "tool_call");
    if (tool?.type !== "tool_call") throw new Error("expected tool");
    expect(tool.tool.status).toBe("done");
    expect(tool.tool.result).toBe("ok");
  });

  it("returns the same workspace reference when the event changes nothing", () => {
    const ws = createWorkspace("t1");
    expect(
      reduceWorkspace(
        ws,
        evt({ type: "turn_status", turn_id: "t1", session_id: "s1", phase: "verifying", reason: "gate" }),
      ),
    ).toBe(ws);
    expect(
      reduceWorkspace(
        ws,
        evt({ type: "tool_call_delta", turn_id: "t1", call_id: "ghost", arguments: "{}" }),
      ),
    ).toBe(ws);
  });

  it("creates a completed row when tool_call_result arrives before its start", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_result", turn_id: "t1", call_id: "c9", name: "bash", result: "ok", is_error: false }),
    );
    const blocks = workspaceBlocks(ws);
    expect(blocks.map((b) => b.type)).toEqual(["tool_call"]);
    if (blocks[0].type !== "tool_call") throw new Error("expected tool");
    expect(blocks[0].tool.status).toBe("done");
    expect(blocks[0].tool.result).toBe("ok");
    // A later start for the same id must not duplicate the row.
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_start", turn_id: "t1", call_id: "c9", name: "bash" }),
    );
    expect(workspaceBlocks(ws)).toHaveLength(1);
  });

  it("attaches an early mcp_app without dropping the row", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(
      ws,
      evt({
        type: "mcp_app",
        turn_id: "t1",
        call_id: "c7",
        name: "mcp__dash",
        server: "charts",
        resource_uri: "ui://x",
        html: "<h1/>",
        is_error: false,
      }),
    );
    const blocks = workspaceBlocks(ws);
    if (blocks[0].type !== "tool_call") throw new Error("expected tool");
    expect(blocks[0].tool.mcpApp?.server).toBe("charts");
  });
});

describe("document artifacts", () => {
  it("appends an artifact block right after a tool result carrying a document path", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(
      ws,
      evt({ type: "tool_call_start", turn_id: "t1", call_id: "c1", name: "create_doc" }),
    );
    ws = reduceWorkspace(
      ws,
      evt({
        type: "tool_call_result",
        turn_id: "t1",
        call_id: "c1",
        name: "create_doc",
        result: "Created Word document: D:\\out\\report.docx\n(12 KB, Word-compatible)",
        is_error: false,
      }),
    );
    const blocks = workspaceBlocks(ws);
    expect(blocks.map((b) => b.type)).toEqual(["tool_call", "artifact"]);
    const art = blocks[1];
    if (art.type !== "artifact") throw new Error("expected artifact");
    expect(art.path).toBe("D:\\out\\report.docx");
    expect(art.name).toBe("report.docx");
  });

  it("detects pdf artifacts too", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(
      ws,
      evt({
        type: "tool_call_result",
        turn_id: "t1",
        call_id: "c1",
        name: "create_doc",
        result: "Generated: /home/user/report.pdf",
        is_error: false,
      }),
    );
    const blocks = workspaceBlocks(ws);
    expect(blocks.some((b) => b.type === "artifact")).toBe(true);
  });

  it("adds no artifact when the result has no document path", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(
      ws,
      evt({
        type: "tool_call_result",
        turn_id: "t1",
        call_id: "c1",
        name: "grep",
        result: "no matches",
        is_error: false,
      }),
    );
    expect(workspaceBlocks(ws).some((b) => b.type === "artifact")).toBe(false);
  });

  it("adds no artifact for errored results", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(
      ws,
      evt({
        type: "tool_call_result",
        turn_id: "t1",
        call_id: "c1",
        name: "create_doc",
        result: "failed to write",
        is_error: true,
      }),
    );
    expect(workspaceBlocks(ws).some((b) => b.type === "artifact")).toBe(false);
  });

  it("is idempotent per tool call (no duplicate artifact on re-processing)", () => {
    let ws = createWorkspace("t1");
    const result = evt({
      type: "tool_call_result",
      turn_id: "t1",
      call_id: "c1",
      name: "create_doc",
      result: "Created: D:\\out\\a.docx",
      is_error: false,
    });
    ws = reduceWorkspace(ws, result);
    ws = reduceWorkspace(ws, result);
    const arts = workspaceBlocks(ws).filter((b) => b.type === "artifact");
    expect(arts).toHaveLength(1);
  });

  it("artifact blocks materialize through mergeWorkspaceBlocks (kept by identity)", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(
      ws,
      evt({
        type: "tool_call_result",
        turn_id: "t1",
        call_id: "c1",
        name: "create_doc",
        result: "Created: D:\\out\\a.docx",
        is_error: false,
      }),
    );
    const merged = mergeWorkspaceBlocks(ws, []);
    expect(merged.some((b) => b.type === "artifact")).toBe(true);
  });

  it("adds NO artifact in code mode (artifact cards are depwork-only)", () => {
    let ws = createWorkspace("t1");
    ws = reduceWorkspace(
      ws,
      evt({
        type: "tool_call_result",
        turn_id: "t1",
        call_id: "c1",
        name: "create_doc",
        result: "Created: D:\\out\\a.docx",
        is_error: false,
      }),
      "code",
    );
    expect(workspaceBlocks(ws).some((b) => b.type === "artifact")).toBe(false);
    // The tool row itself still lands.
    expect(workspaceBlocks(ws).some((b) => b.type === "tool_call")).toBe(true);
  });
});
