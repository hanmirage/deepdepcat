/**
 * Chat-related types.
 */

import type { ProgressContentBlock, ChatStreamEvent } from "@/lib/tauri";

export type { ModelInfo, ChatMessage, ChatStreamEvent, ProgressContentBlock } from "@/lib/tauri";

/** Live phase of the current turn, inferred from the chat-stream events.
 *  The UI shows a status line ONLY for the phases that have no other
 *  visual feedback (connecting, and thinking when the panel is hidden). */
export type StreamPhase =
  | "idle"
  | "connecting"
  | "thinking"
  | "generating"
  | "tool_running"
  /** A stop-path gate held the turn (verification pending / evaluator
   *  review / discipline nudge) — the already-streamed text is NOT final. */
  | "verifying";

/** Terminal outcome of a turn, mirrored from the backend `TurnOutcome`
 *  (renamed so the live `turn_status` event keeps the name). `done` is
 *  terminal: the turn must not self-drive another round. */
export type TurnOutcome =
  | "done"
  | "needs_input"
  | "failed"
  | "limit"
  | "cancelled"
  | "denied";

/** Infer the turn phase from a chat-stream event. Returns the new phase,
 *  or null when the event doesn't change it. */
export function inferStreamPhase(ev: ChatStreamEvent): StreamPhase | null {
  switch (ev.type) {
    case "turn_start":
      return "connecting";
    case "reasoning_delta":
      return "thinking";
    case "text_delta":
      return "generating";
    case "tool_call_start":
    case "tool_call_delta":
    case "tool_call_progress":
    case "tool_call_result":
    case "mcp_app":
      return "tool_running";
    case "turn_status":
      if (ev.phase === "verifying") return "verifying";
      return null;
    case "snapshot":
      // Snapshot is a repair payload — it never changes the live phase.
      return null;
    case "turn_end":
    case "error":
      return "idle";
    default:
      return null;
  }
}

/** Minimum overlap length treated as a repeated tail. */
const MIN_OVERLAP_LENGTH = 10;

/** Max delta prefix length examined for an overlap. */
const MAX_OVERLAP_CHECK = 30;

/** Trim a repeated tail from the incoming delta against the rendered text.
 *
 * DeepSeek occasionally re-emits the previous segment's tail at the start
 * of the next delta. This runs ONCE per flush on the pending chunk
 * (O(delta × 30)) instead of the renderer scanning the whole text — the
 * renderer's job is append-only. */
export function trimStreamOverlap(prev: string, delta: string): string {
  if (delta.length < MIN_OVERLAP_LENGTH) return delta;
  const head = delta.slice(0, Math.min(MAX_OVERLAP_CHECK, delta.length));
  const max = Math.min(head.length, prev.length);
  for (let len = max; len >= MIN_OVERLAP_LENGTH; len--) {
    if (prev.endsWith(head.slice(0, len))) return delta.slice(len);
  }
  return delta;
}

/** Progress kind — matches the 4 ToolProgress variants from the backend. */
export type ProgressKind = "text" | "content" | "custom" | "partial_result";

/** State of a single tool call within an assistant message. */
export interface ToolCallState {
  id: string;
  name: string;
  arguments: string;
  status: "running" | "done" | "error";
  /** Local wall-clock start (ms) — elapsed-time display on the card. */
  startedAt?: number;
  /** Concurrent-run batch id — tools that overlap in time share it, so the
   *  UI can fold a parallel run into one group (Claude-style "N tools"). */
  parallelBatch?: number;

  // ── Progress (single content channel: kind + delta + total_bytes) ─────
  progressKind?: ProgressKind;
  /** Accumulated output delta (bash stdout/stderr, custom JSON payloads). */
  progressDelta?: string;
  /** Total bytes seen so far. */
  progressTotalBytes?: number;

  result?: string;

  // ── MCP Apps (interactive UI attached to MCP tool results) ────────
  /** Rendered HTML app from an MCP server (`ui://` resource). When present,
   *  the tool card shows it in a sandboxed iframe. */
  mcpApp?: {
    server: string;
    resource_uri: string;
    html: string;
    is_error: boolean;
    /** CSP domains declared by the server (`_meta.ui.csp`). */
    csp?: Record<string, unknown>;
  };
}

/** A single block within a message — either text or a tool call.
 *  Blocks are ordered, so tool calls can be interleaved with text. */
export type MessageBlock =
  // `streamId` marks blocks owned by the live stream workspace — the
  // reducer patches them in place on each commit while injected blocks
  // (subagent narrative, error, changes summary) keep their identity.
  | { type: "text"; content: string; streamId?: string }
  // deepseek-native: reasoning content from thinking mode stream
  | { type: "reasoning"; content: string; streamId?: string }
  | { type: "tool_call"; tool: ToolCallState }
  // turn-level failure (backend StreamEvent::Error) — rendered as a distinct
  // error banner, never mixed into markdown text
  | { type: "error"; content: string }
  // Document artifact produced this turn (a docx/pptx/xlsx/html/pdf path
  // extracted from a tool result) — rendered as a product card with an
  // open-in-preview action, in flow position right after the tool that made it.
  | { type: "artifact"; id: string; path: string; name: string }
  // End-of-turn change summary — the aggregated file edits of this turn
  // (one entry per touched file, last edit wins), rendered as a compact
  // file-list + expandable diffs card instead of digging through tool cards.
  | { type: "changes_summary"; changes: FileChange[] };

/** One file change aggregated from the turn's write tools. */
export interface FileChange {
  path: string;
  /** The old content (empty for a brand-new file / write_file). */
  oldText: string;
  /** The new content. */
  newText: string;
}

/** Whether a block type carries a file change we aggregate into the summary. */
const EDIT_TOOLS = new Set(["edit_file", "search_replace", "write_file"]);

/**
 * Aggregate the turn's file edits into a per-file change list.
 *
 * Reads the `old_text`/`new_text`/`content` from each write tool's
 * arguments; same-file edits collapse to the LAST one (the summary shows
 * the final state, matching the file on disk). Entries whose old and new
 * text are identical are dropped. Pure function — unit-testable.
 */
export function collectChanges(blocks: MessageBlock[]): FileChange[] {
  const byPath = new Map<string, FileChange>();
  for (const block of blocks) {
    if (block.type !== "tool_call") continue;
    const tool = block.tool;
    if (!EDIT_TOOLS.has(tool.name)) continue;
    let parsed: Record<string, unknown>;
    try {
      parsed = tool.arguments ? JSON.parse(tool.arguments) : {};
    } catch {
      continue;
    }
    const path = typeof parsed.path === "string" ? parsed.path : null;
    if (!path) continue;

    let oldText = "";
    let newText: string;
    if (tool.name === "write_file") {
      newText = typeof parsed.content === "string" ? parsed.content : "";
    } else {
      oldText = typeof parsed.old_text === "string" ? parsed.old_text : "";
      newText = typeof parsed.new_text === "string" ? parsed.new_text : "";
    }
    if (oldText === newText) continue;
    byPath.set(path, { path, oldText, newText });
  }
  return [...byPath.values()];
}

/** UI-only message type (includes local state not sent to backend). */
export interface UIMessage {
  id: string;
  role: "user" | "assistant";
  blocks: MessageBlock[];
  model?: string;
  tokenUsage?: { prompt: number; completion: number };
  timestamp: number;
  isStreaming?: boolean;
  /** Terminal outcome of the turn that produced this message (from
   *  turn_end.status / snapshot). Absent for restored/historical messages. */
  turnOutcome?: TurnOutcome;
  /** Context chips attached to this message (only for user messages). */
  contextChips?: ContextChip[];
}

/** Live state of one spawned subagent, updated from stream events. Kept OUT
 *  of the message blocks — subagents are surfaced in the dedicated right-panel
 *  subagent pane and on their linked agent tool card, not as injected text
 *  (the parent message only gets a one-line result summary when the subagent
 *  finishes). */
export interface SubagentUIRecord {
  subagent_id: string;
  task: string;
  agent_type: string;
  /** The parent's `agent` tool call id ("" when unknown, e.g. decompose). */
  tool_call_id: string | null;
  status: "running" | "done" | "failed";
  turn: number;
  total_turns: number;
  lastMessage: string;
  /** Full subagent result (stored as-received so the panel can expand it;
   *  the parent message still gets a summarized narrative). */
  result: string;
  /** Local wall-clock start (ms) — fallback when the worker registry is unreachable. */
  startedAt: number;
}

/** Strip provider tool-call protocol markup from assistant text.
 *  The backend sanitizes before persisting; this is the display-side last
 *  line of defense for history written by older builds (real session
 *  dc4989af stored raw `<tool_calls>` blocks in message 39; real session
 *  7a1dd319 stored DeepSeek DSML `<｜DSML｜tool_calls>` blocks in message
 *  143). */
export function stripToolCallMarkup(text: string): string {
    // DeepSeek DSML variants: fullwidth `｜DSML｜` bars and ASCII `||DSML||`.
    // deepseek-v4-flash also emits DOUBLE fullwidth bars (`＜＜DSML＞＞`,
    // two U+FF5C per side) — normalize to the canonical single-bar form so
    // every block-stripping regex below catches it (real session 2026-08-11).
    const dsml = text
      .replace(/\|\|DSML\|\|/g, "｜DSML｜")
      .replace(/＜＜DSML＞＞/g, "｜DSML｜")
      .replace(/〡DSML〡/g, "｜DSML｜");
  // Harness frames (system-reminder / app-guidance / task-notification /
  // evaluator & goal review blocks) must never render in the chat — they are
  // instructions for the model, not conversation content (display-side
  // belt-and-braces; the backend strips them before persistence too).
  const strippedFrames = dsml.replace(
    /<(?:system-reminder|app-guidance|task-notification|evaluator-review|goal-review|coordinator_phase|current-goal|environment-context)[^>]*>[\s\S]*?<\/(?:system-reminder|app-guidance|task-notification|evaluator-review|goal-review|coordinator_phase|current-goal|environment-context)>/gi,
    " ",
  );
  return strippedFrames
    .replace(/<｜DSML｜tool_calls>[\s\S]*?<\/｜DSML｜tool_calls>/gi, " ")
    .replace(/<｜DSML｜function_calls>[\s\S]*?<\/｜DSML｜function_calls>/gi, " ")
    .replace(/<｜DSML｜invoke[^>]*>[\s\S]*?<\/｜DSML｜invoke>/gi, " ")
    .replace(/<｜DSML｜invoke[^>]*\/>/gi, " ")
    .replace(/<｜DSML｜parameter[^>]*>[\s\S]*?<\/｜DSML｜parameter>/gi, " ")
    .replace(/<\/?｜DSML｜[^>]*>/gi, " ")
    .replace(/<tool_calls>[\s\S]*?<\/tool_calls>/gi, " ")
    .replace(/<tool_call>[\s\S]*?<\/tool_call>/gi, " ")
    .replace(/<\/?tool_calls?[^>]*>/gi, " ");
}

/** Shorten a subagent result for the one-line message summary. */
export function summarizeSubagentResult(result: string, maxLen = 120): string {
  const cleaned = stripToolCallMarkup(result);
  const oneLine = cleaned.replace(/\s+/g, " ").trim();
  return oneLine.length > maxLen ? `${oneLine.slice(0, maxLen).trimEnd()}…` : oneLine;
}

/** How tool calls are handled during a conversation turn.
 *  Maps to backend PermissionMode via ModeComboBox (per-session scope):
 *    confirm       → manual   (depwork: 更改前提问 — write ops prompt)
 *    plan          → plan     (code: 计划模式 — read-only planning)
 *    chat_only     → chat_only (depwork: 纯聊天 — no tool execution)
 *    accept_edits  → accept_edits (code: 接受编辑 — edits auto-approved)
 *    auto          → bypass   (完全访问/完全放行 — everything approved)
 *  "analysis"/"chat" remain as LEGACY persisted values (depwork pre-#120);
 *  the UI and mode detection no longer produce them.
 *  "auto" here is the interaction-mode name; it is NOT the DeepSeek reasoning
 *  auto mode. */
export type InteractionMode = "read_only" | "accept_edits" | "full_access";

/** A removable context chip attached to the chat input.
 *  `paper` is depwork-only (a document the agent generated/opened); the
 *  depwork send path maps it to a `file` chip before the API call. */
export interface ContextChip {
  id: string;
  type: "file" | "folder" | "url" | "paper";
  name: string;
  /** Full filesystem path (for file/folder) or URL string (for url). */
  path: string;
  /**
   * Image bytes as a `data:<mime>;base64,...` URL — present only for
   * pasted/picked pictures. The backend transcribes the image to text via
   * the vision model; images never reach the model as paths.
   */
  dataUrl?: string;
}
