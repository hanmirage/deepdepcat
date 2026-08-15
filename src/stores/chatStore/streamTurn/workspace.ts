/**
 * Pure stream reducer + per-turn workspace.
 *
 * One `TurnWorkspace` accumulates the turn's streaming content as ordered
 * draft blocks (text/reasoning segments keep their stream position; tool
 * content lives in a map keyed by call id). Every wire delta flows through
 * `reduceWorkspace`, a PURE function — no store writes, no timers, no
 * side effects — so ordering and overlap-trimming are unit-testable in
 * isolation.
 *
 * The listener schedules a RAF commit: `mergeWorkspaceBlocks` patches the
 * assistant message's blocks BY IDENTITY (stream-owned blocks replaced in
 * place, injected blocks like subagent narrative / error / changes summary
 * untouched), instead of rebuilding the whole message from scattered
 * accumulators.
 */

import { appendCapped, MAX_PROGRESS_CHARS } from "@/lib/progressCap";
import { trimStreamOverlap } from "@/types/chat";
import { extractDocumentPath, basenameOf } from "../docDispatch";
import type { ChatWorkMode } from "../types";
import type { ChatStreamEvent, TokenUsageEvent } from "@/lib/tauri";
import type { MessageBlock, ProgressKind, ToolCallState } from "@/types";

/** Live draft of one tool call inside the workspace. */
export interface ToolDraft {
  id: string;
  name: string;
  arguments: string;
  status: "running" | "done" | "error";
  result?: string;
  progressKind?: ProgressKind;
  progressDelta?: string;
  progressTotalBytes?: number;
  mcpApp?: ToolCallState["mcpApp"];
  startedAt: number;
  /** Concurrent-run batch id — tools that overlap share it, so the UI can
   *  fold a parallel run into one group (Claude-style "N 个工具并行"). */
  parallelBatch: number;
}

/** Ordered block reference — content lives inline for text/reasoning and
 *  in `tools` for tool calls. */
export type DraftBlock =
  | { kind: "text"; streamId: string; content: string }
  | { kind: "reasoning"; streamId: string; content: string }
  | { kind: "tool"; id: string }
  | { kind: "artifact"; id: string; path: string; name: string };

/** Per-turn streaming accumulator (immutable updates, cheap structural
 *  sharing — deltas only touch the blocks array and/or the tools map). */
export interface TurnWorkspace {
  turnId: string;
  blocks: DraftBlock[];
  tools: Map<string, ToolDraft>;
  /** Summed usage across the turn's LLM calls (cumulative, like the store). */
  usage: TokenUsageEvent | null;
  /** Monotonic counter for text/reasoning block ids. */
  nextBlockSeq: number;
  /** Currently-running tool count — drives parallel-batch assignment. */
  openTools: number;
  /** Increments each time the last open tool completes; the NEXT tool
   *  started after that belongs to a fresh (non-overlapping) batch. */
  batchSeq: number;
}

export function createWorkspace(turnId: string): TurnWorkspace {
  return {
    turnId,
    blocks: [],
    tools: new Map(),
    usage: null,
    nextBlockSeq: 0,
    openTools: 0,
    batchSeq: 0,
  };
}

function textStreamId(ws: TurnWorkspace): string {
  return `${ws.turnId}:t${ws.nextBlockSeq}`;
}

function reasoningStreamId(ws: TurnWorkspace): string {
  return `${ws.turnId}:r${ws.nextBlockSeq}`;
}

/** Append a text delta to the LAST draft when it is a text segment (the
 *  model continues its answer), otherwise start a new segment — mirrors the
 *  old flush's block-order behavior (text after a tool = a new block). */
function appendText(ws: TurnWorkspace, text: string): TurnWorkspace {
  const last = ws.blocks[ws.blocks.length - 1];
  if (last && last.kind === "text") {
    const merged = last.content + trimStreamOverlap(last.content, text);
    return {
      ...ws,
      blocks: [...ws.blocks.slice(0, -1), { ...last, content: merged }],
    };
  }
  return {
    ...ws,
    nextBlockSeq: ws.nextBlockSeq + 1,
    blocks: [
      ...ws.blocks,
      { kind: "text", streamId: textStreamId(ws), content: text },
    ],
  };
}

/** Append a reasoning delta — to the last reasoning draft when adjacent,
 *  otherwise inserted AHEAD of the first text segment (deepseek-native:
 *  the thinking block always renders above the answer, even when the
 *  provider interleaves late reasoning tails). */
function appendReasoning(ws: TurnWorkspace, text: string): TurnWorkspace {
  const last = ws.blocks[ws.blocks.length - 1];
  if (last && last.kind === "reasoning") {
    return {
      ...ws,
      blocks: [...ws.blocks.slice(0, -1), { ...last, content: last.content + text }],
    };
  }
  const draft: DraftBlock = {
    kind: "reasoning",
    streamId: reasoningStreamId(ws),
    content: text,
  };
  const idx = ws.blocks.findIndex((b) => b.kind === "text");
  const blocks = [...ws.blocks];
  blocks.splice(idx === -1 ? blocks.length : idx, 0, draft);
  return { ...ws, nextBlockSeq: ws.nextBlockSeq + 1, blocks };
}

function updateTool(
  ws: TurnWorkspace,
  callId: string,
  patch: Partial<ToolDraft>,
): TurnWorkspace {
  const tool = ws.tools.get(callId);
  if (!tool) return ws;
  return {
    ...ws,
    tools: new Map(ws.tools).set(callId, { ...tool, ...patch }),
  };
}

/** Append a document-artifact draft when a tool result carries a document
 *  path (docx/pptx/xlsx/html/pdf). Depwork-only — the artifact card is a
 *  document-workbench concept; code mode keeps the flow clean (the user's
 *  explicit call). Idempotent per tool call — the artifact block lands right
 *  after the tool that produced it, in flow order. */
function maybeAppendArtifact(
  ws: TurnWorkspace,
  callId: string,
  result: string | undefined,
  mode: ChatWorkMode,
): TurnWorkspace {
  if (mode !== "depwork") return ws;
  if (!result) return ws;
  if (ws.blocks.some((b) => b.kind === "artifact" && b.id === callId)) return ws;
  const path = extractDocumentPath(result);
  if (!path) return ws;
  return {
    ...ws,
    blocks: [
      ...ws.blocks,
      { kind: "artifact", id: callId, path, name: basenameOf(path) },
    ],
  };
}

/** Fold one wire delta into the workspace. Returns the SAME workspace
 *  reference when the event changes nothing (idempotent events, events for
 *  unknown call ids, non-stream events). `mode` gates mode-specific blocks
 *  (document artifacts are depwork-only); defaults to depwork so the pure
 *  tests exercise the full feature. */
export function reduceWorkspace(
  ws: TurnWorkspace,
  event: ChatStreamEvent,
  mode: ChatWorkMode = "depwork",
  now: number = Date.now(),
): TurnWorkspace {
  switch (event.type) {
    case "text_delta":
      return appendText(ws, event.text);
    case "reasoning_delta":
      return appendReasoning(ws, event.text);
    case "tool_call_start": {
      if (ws.tools.has(event.call_id)) return ws;
      const draft: ToolDraft = {
        id: event.call_id,
        name: event.name,
        arguments: "",
        status: "running",
        startedAt: now,
        parallelBatch: ws.batchSeq,
      };
      return {
        ...ws,
        openTools: ws.openTools + 1,
        blocks: [...ws.blocks, { kind: "tool", id: event.call_id }],
        tools: new Map(ws.tools).set(event.call_id, draft),
      };
    }
    case "tool_call_delta": {
      const tool = ws.tools.get(event.call_id);
      if (!tool) return ws;
      return updateTool(ws, event.call_id, {
        arguments: appendCapped(tool.arguments, event.arguments, MAX_PROGRESS_CHARS),
      });
    }
    case "tool_call_progress": {
      const tool = ws.tools.get(event.call_id);
      if (!tool) return ws;
      return updateTool(ws, event.call_id, {
        progressKind: event.kind as ProgressKind,
        progressDelta: event.delta
          ? appendCapped(tool.progressDelta ?? "", event.delta, MAX_PROGRESS_CHARS)
          : tool.progressDelta,
        progressTotalBytes: event.total_bytes ?? tool.progressTotalBytes,
      });
    }
    case "tool_call_result": {
      const tool = ws.tools.get(event.call_id);
      if (!tool) {
        // Out-of-order wire (result before its start) — create the row in
        // its terminal state instead of silently dropping the outcome.
        const draft: ToolDraft = {
          id: event.call_id,
          name: event.name,
          arguments: "",
          status: event.is_error ? "error" : "done",
          result: event.result,
          startedAt: now,
          parallelBatch: ws.batchSeq,
        };
        return maybeAppendArtifact(
          {
            ...ws,
            blocks: [...ws.blocks, { kind: "tool", id: event.call_id }],
            tools: new Map(ws.tools).set(event.call_id, draft),
          },
          event.call_id,
          event.result,
          mode,
        );
      }
      // A running tool completes — drain one from the open counter; when the
      // last concurrent tool finishes, the next tool starts a fresh batch.
      const wasRunning = tool.status === "running";
      const openTools = wasRunning ? Math.max(0, ws.openTools - 1) : ws.openTools;
      const batchSeq = openTools === 0 ? ws.batchSeq + 1 : ws.batchSeq;
      return maybeAppendArtifact(
        {
          ...ws,
          openTools,
          batchSeq,
          tools: new Map(ws.tools).set(event.call_id, {
            ...tool,
            name: event.name,
            status: event.is_error ? "error" : "done",
            result: event.result,
          }),
        },
        event.call_id,
        event.result,
        mode,
      );
    }
    case "mcp_app": {
      const tool = ws.tools.get(event.call_id);
      if (!tool) {
        const draft: ToolDraft = {
          id: event.call_id,
          name: event.name,
          arguments: "",
          status: "running",
          startedAt: now,
          parallelBatch: ws.batchSeq,
          mcpApp: {
            server: event.server,
            resource_uri: event.resource_uri,
            html: event.html,
            is_error: event.is_error,
            csp: event.csp as Record<string, unknown> | undefined,
          },
        };
        return {
          ...ws,
          blocks: [...ws.blocks, { kind: "tool", id: event.call_id }],
          tools: new Map(ws.tools).set(event.call_id, draft),
        };
      }
      return updateTool(ws, event.call_id, {
        mcpApp: {
          server: event.server,
          resource_uri: event.resource_uri,
          html: event.html,
          is_error: event.is_error,
          csp: event.csp as Record<string, unknown> | undefined,
        },
      });
    }
    case "usage": {
      const prev = ws.usage;
      return {
        ...ws,
        usage: {
          prompt_tokens: (prev?.prompt_tokens ?? 0) + event.usage.prompt_tokens,
          completion_tokens:
            (prev?.completion_tokens ?? 0) + event.usage.completion_tokens,
          cached_read_tokens:
            (prev?.cached_read_tokens ?? 0) + (event.usage.cached_read_tokens ?? 0),
          reasoning_tokens:
            (prev?.reasoning_tokens ?? 0) + (event.usage.reasoning_tokens ?? 0),
          prompt_cache_hit_tokens:
            (prev?.prompt_cache_hit_tokens ?? 0) +
            (event.usage.prompt_cache_hit_tokens ?? 0),
          prompt_cache_miss_tokens:
            (prev?.prompt_cache_miss_tokens ?? 0) +
            (event.usage.prompt_cache_miss_tokens ?? 0),
        },
      };
    }
    default:
      return ws;
  }
}

/** Materialize the workspace into renderable message blocks (stream order). */
export function workspaceBlocks(ws: TurnWorkspace): MessageBlock[] {
  const out: MessageBlock[] = [];
  for (const draft of ws.blocks) {
    if (draft.kind === "text") {
      out.push({ type: "text", content: draft.content, streamId: draft.streamId });
    } else if (draft.kind === "reasoning") {
      out.push({
        type: "reasoning",
        content: draft.content,
        streamId: draft.streamId,
      });
    } else if (draft.kind === "artifact") {
      out.push({
        type: "artifact",
        id: draft.id,
        path: draft.path,
        name: draft.name,
      });
    } else {
      const t = ws.tools.get(draft.id);
      if (!t) continue;
      const tool: ToolCallState = {
        id: t.id,
        name: t.name,
        arguments: t.arguments,
        status: t.status,
        result: t.result,
        progressKind: t.progressKind,
        progressDelta: t.progressDelta,
        progressTotalBytes: t.progressTotalBytes,
        mcpApp: t.mcpApp,
        startedAt: t.startedAt,
        parallelBatch: t.parallelBatch,
      };
      out.push({ type: "tool_call", tool });
    }
  }
  return out;
}

/** Identity of a stream-owned block (null = injected, never replaced). */
function blockKey(block: MessageBlock): string | null {
  if (block.type === "tool_call") return `tool:${block.tool.id}`;
  if (block.type === "text" && block.streamId) return `text:${block.streamId}`;
  if (block.type === "reasoning" && block.streamId) {
    return `reasoning:${block.streamId}`;
  }
  if (block.type === "artifact") return `artifact:${block.id}`;
  return null;
}

/** Patch `existing` blocks in place by identity: stream-owned blocks are
 *  replaced (or appended when new), everything else keeps its position. */
export function mergeWorkspaceBlocks(
  ws: TurnWorkspace,
  existing: MessageBlock[],
): MessageBlock[] {
  const drafts = workspaceBlocks(ws);
  const pending = new Map<string, MessageBlock>();
  for (const draft of drafts) {
    const key = blockKey(draft);
    if (key) pending.set(key, draft);
  }
  const out: MessageBlock[] = [];
  for (const block of existing) {
    const key = blockKey(block);
    const replacement = key ? pending.get(key) : undefined;
    if (key && replacement) {
      out.push(replacement);
      pending.delete(key);
    } else {
      out.push(block);
    }
  }
  for (const draft of drafts) {
    const key = blockKey(draft);
    if (!key) continue;
    if (pending.has(key)) {
      out.push(pending.get(key) as MessageBlock);
      pending.delete(key);
    }
  }
  return out;
}
