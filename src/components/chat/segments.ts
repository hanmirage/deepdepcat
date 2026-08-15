/**
 * Segment builder — turns a message's blocks into ordered renderable units.
 *
 * - text / ask_user / error blocks render inline in the narrative flow
 * - read-family tools (read_file, grep, glob, …) collapse into one group
 *   when consecutive (Claude-style "已读取 N 项")
 * - every other tool renders as its own bare row
 *
 * The former normal/verbose/summary reply-density views were removed as
 * useless product clutter (2026-08-11): the tool rows are already minimal
 * in the narrative, so the three modes only re-shuffled the same content.
 */

import type { MessageBlock, ToolCallState } from "@/types";
import { isReadTool } from "@/config/toolNarrative";

type TextBlock = Extract<MessageBlock, { type: "text" }>;
type ToolBlock = Extract<MessageBlock, { type: "tool_call" }>;
type ErrorBlock = Extract<MessageBlock, { type: "error" }>;
type ArtifactBlock = Extract<MessageBlock, { type: "artifact" }>;

/** Blocks that render inline in the flowing narrative. */
type NarrativeBlock =
  | TextBlock
  | Extract<ToolBlock, { tool: { name: "ask_user" } }>
  | ErrorBlock
  | ArtifactBlock;

/** One renderable unit, in block order. */
export type Segment =
  | { kind: "block"; block: NarrativeBlock }
  | { kind: "tool"; tool: ToolCallState }
  | { kind: "readGroup"; tools: ToolCallState[] }
  | { kind: "parallelGroup"; tools: ToolCallState[] };

/** Build renderable segments from message blocks. */
export function buildSegments(blocks: MessageBlock[]): Segment[] {
  const out: Segment[] = [];
  for (const block of blocks) {
    if (block.type === "reasoning") continue; // rendered above, not in flow
    if (block.type === "tool_call" && block.tool.name !== "ask_user") {
      if (isReadTool(block.tool.name)) {
        // Merge into the trailing read group when still consecutive.
        const last = out[out.length - 1];
        if (last && last.kind === "readGroup") {
          last.tools.push(block.tool);
        } else {
          out.push({ kind: "readGroup", tools: [block.tool] });
        }
      } else {
        out.push({ kind: "tool", tool: block.tool });
      }
      continue;
    }
    out.push({ kind: "block", block: block as NarrativeBlock });
  }
  return mergeParallel(out);
}

/** Fold consecutive non-read tool rows that ran in the SAME concurrent batch
 *  into one parallel group (≥2 members). A single-tool batch stays a plain
 *  tool row — there is nothing to fold. A tool without a batch id (restored
 *  history from older builds) is never grouped. */
function mergeParallel(segments: Segment[]): Segment[] {
  const out: Segment[] = [];
  for (const seg of segments) {
    const last = out[out.length - 1];
    if (seg.kind !== "tool" || seg.tool.parallelBatch === undefined) {
      out.push(seg);
      continue;
    }
    if (last?.kind === "parallelGroup" && last.tools[0].parallelBatch === seg.tool.parallelBatch) {
      last.tools.push(seg.tool);
    } else if (last?.kind === "tool" && last.tool.parallelBatch === seg.tool.parallelBatch) {
      out[out.length - 1] = { kind: "parallelGroup", tools: [last.tool, seg.tool] };
    } else {
      out.push(seg);
    }
  }
  return out;
}
