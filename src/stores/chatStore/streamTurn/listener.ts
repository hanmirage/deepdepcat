/**
 * buildStreamListener — the chat-stream turn listener, extracted from
 * sendMessage. Owns the per-turn event state machine (turn_start through
 * turn_end/error) plus the RAF-batched flush scheduler.
 *
 * The handler is a thin dispatcher: each event type's logic lives in a
 * module-level handler so the state machine stays reviewable and each
 * function stays within the repo's 80-line budget.
 */

import type { ChatStreamEvent, TurnSnapshot } from "@/lib/tauri";
import { chatApi, type TokenUsageEvent } from "@/lib/tauri";
import { createWorkspace, mergeWorkspaceBlocks, reduceWorkspace, type TurnWorkspace } from "./workspace";
import { updateSessionMessages } from "../sessionMessages";
import { collectChanges, summarizeSubagentResult } from "@/types/chat";
import { agentTypeLabelKey } from "@/config/toolNarrative";
import i18n from "@/i18n";
import { syncStreamingBus } from "../streamState";
import { extractDocumentPath, basenameOf } from "../docDispatch";
import type { ChatState, ChatWorkMode } from "../types";
import type { StreamState } from "../streamState";
import type { StreamPhase, MessageBlock } from "@/types";
import type { TurnOutcome } from "@/types";

/** Max compaction records kept per store (context panel list). */
const MAX_COMPACTIONS = 5;

export interface StreamTurnOptions {
  get: () => ChatState;
  set: (partial: Partial<ChatState> | ((state: ChatState) => Partial<ChatState>)) => void;
  st: StreamState;
  assistantId: string;
  expectedSessionId: string;
  gen: number;
  mode: ChatWorkMode;
  finalizedRef: { current: boolean };
  unlistenRef: { current: (() => void) | null };
}

/** RAF-batched workspace committer — coalesces every delta of a 66ms
 *  window into ONE store write that patches blocks by identity. */
export interface StreamCommitter {
  schedule: () => void;
  /** Commit NOW (turn end / error / snapshot — nothing may stay buffered). */
  flush: () => void;
}

/** Everything a per-event handler needs — created once per turn. */
interface TurnContext {
  get: () => ChatState;
  set: (partial: Partial<ChatState> | ((state: ChatState) => Partial<ChatState>)) => void;
  st: StreamState;
  assistantId: string;
  expectedSessionId: string;
  gen: number;
  mode: ChatWorkMode;
  finalizedRef: { current: boolean };
  unlistenRef: { current: (() => void) | null };
  workspace: { current: TurnWorkspace | null };
  commit: StreamCommitter;
  /** Mutable holder for the backend turn id accepted by turn_start. */
  expectedTurnId: { current: string | null };
  /** Wall-clock at turn_start — first-token latency is measured from here. */
  turnStartTime: { current: number | null };
  /** Last wire seq consumed for OUR turn — gap detection feeds snapshot repair. */
  lastSeq: { current: number | null };
  /** One snapshot pull per turn (dedupes concurrent gap-triggered requests). */
  snapshotRequested: { current: boolean };
  /** Terminal event awaiting snapshot repair (lost turn_start) — the
   *  fallback finalizes with streamed content if the pull never lands. */
  pendingTerminal: { current: ChatStreamEvent | null };
  setPhase: (phase: StreamPhase) => void;
}

/** True when an event belongs to our session (or carries no session id). */
function ownedBySession(expectedSessionId: string, sid?: string | null): boolean {
  return !sid || sid === expectedSessionId;
}

function handleTurnStart(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "turn_start" }>): void {
  const { st, expectedSessionId, expectedTurnId, setPhase } = ctx;
  // Only accept the session's OWN turns (subagent turns share the channel).
  if (event.session_id !== expectedSessionId) return;
  // A turn_start carrying a turn id this session already consumed belongs to
  // a torn-down listener's turn (stop→resend while the backend still drains
  // it). Accepting it would hijack expectedTurnId and let its deltas render
  // into the NEW assistant message. Replays always get a FRESH turn id.
  if (st.lastTurnId !== null && event.turn_id === st.lastTurnId) return;
  expectedTurnId.current = event.turn_id;
  st.lastTurnId = event.turn_id;
  // Start the first-token clock here — a replay turn restarts it.
  ctx.turnStartTime.current = Date.now();
  // A replay turn runs on the SAME listener — reset the per-turn finalize
  // flag so its turn_end finalizes normally (not skipped as "already done").
  ctx.finalizedRef.current = false;
  ctx.snapshotRequested.current = false;
  const ws = createWorkspace(event.turn_id);
  ctx.workspace.current = ws;
  setPhase("connecting");
}

function handleCompaction(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "compaction" }>): void {
  const { set, expectedSessionId, st } = ctx;
  if (event.session_id !== expectedSessionId) return;
  if (st.compactionTimer) clearTimeout(st.compactionTimer);
  set((s) => ({
    notification: i18n.t("notifications.compaction", {
      tokens: event.compacted_tokens,
      defaultValue: `上下文已压缩：节省 ${event.compacted_tokens} tokens`,
    }),
    compactions: [
      {
        tokens: event.compacted_tokens,
        summary: event.summary,
        at: Date.now(),
      },
      ...s.compactions,
    ].slice(0, MAX_COMPACTIONS),
  }));
  st.compactionTimer = setTimeout(() => {
    set({ notification: null });
    st.compactionTimer = null;
  }, 4000);
}

function handleSubagentStart(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "subagent_start" }>): void {
  const { set, expectedSessionId } = ctx;
  if (!ownedBySession(expectedSessionId, event.session_id)) return;
  // No text injection — the subagent lives in the activity panel and on its
  // linked agent tool card. Recorded here so the card shows live progress
  // without a poll round-trip.
  set((s) => ({
    subagents: {
      ...s.subagents,
      [event.subagent_id]: {
        subagent_id: event.subagent_id,
        task: event.task,
        agent_type: event.agent_type,
        tool_call_id: event.tool_call_id ?? null,
        status: "running",
        turn: 0,
        total_turns: 0,
        lastMessage: "",
        result: "",
        startedAt: Date.now(),
      },
    },
  }));
}

function handleSubagentProgress(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "subagent_progress" }>): void {
  const { set, expectedSessionId } = ctx;
  if (!ownedBySession(expectedSessionId, event.session_id)) return;
  set((s) => {
    const prev = s.subagents[event.subagent_id];
    if (!prev) return {};
    return {
      subagents: {
        ...s.subagents,
        [event.subagent_id]: {
          ...prev,
          turn: event.turn,
          total_turns: event.total_turns,
          lastMessage: event.message,
        },
      },
    };
  });
}

function handleSubagentResult(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "subagent_result" }>): void {
  const { get, set, assistantId, expectedSessionId } = ctx;
  if (!ownedBySession(expectedSessionId, event.session_id)) return;
  const summary = summarizeSubagentResult(event.result);
  // Natural-lifecycle narrative — "✓ 子代理「探查」完成：…" (agent_type from
  // the subagent_start record on the store).
  const typeKey = agentTypeLabelKey(get().subagents[event.subagent_id]?.agent_type ?? "general");
  const narrative = i18n.t(event.success ? "chat.subagentDone" : "chat.subagentFailed", {
    type: i18n.t(typeKey),
    summary,
  });
  set((s) => ({
    subagents: {
      ...s.subagents,
      [event.subagent_id]: {
        ...(s.subagents[event.subagent_id] ?? {
          subagent_id: event.subagent_id,
          task: "",
          agent_type: "",
          tool_call_id: event.tool_call_id ?? null,
          status: "running",
          turn: 0,
          total_turns: 0,
          lastMessage: "",
          result: "",
          startedAt: Date.now(),
        }),
        status: event.success ? "done" : "failed",
        // Full result for the right-panel subagent pane (expandable); the
        // narrative below stays a summarized one-liner.
        result: event.result,
      },
    },
  }));
  // One-line result summary stays in the message flow so the conversation
  // history keeps the outcome readable.
  updateSessionMessages(
    ctx.st,
    expectedSessionId,
    (msgs) =>
      msgs.map((m) =>
        m.id === assistantId
          ? {
              ...m,
              blocks: [...m.blocks, { type: "text" as const, content: `\n${narrative}\n` }],
            }
          : m,
      ),
    get,
    set,
  );
}

function handleElicitation(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "elicitation" }>): void {
  ctx.set({
    pendingElicitation: {
      elicitationId: event.elicitation_id,
      serverName: event.server_name,
      message: event.message,
    },
  });
}

function handleMemoryInjected(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "memory_injected" }>): void {
  const { set, expectedSessionId } = ctx;
  if (event.session_id !== expectedSessionId) return;
  set({ memoryRef: { count: event.count, snippet: event.snippet } });
}

/** Build the RAF-batched workspace committer for one assistant message.
 *  Every commit patches blocks BY IDENTITY via `mergeWorkspaceBlocks` and
 *  applies the cumulative-usage delta exactly once. */
function createCommitter(options: {
  assistantId: string;
  workspaceRef: { current: TurnWorkspace | null };
  st: StreamState;
  expectedSessionId: string;
  get: () => ChatState;
  set: (partial: Partial<ChatState>) => void;
}): StreamCommitter {
  const { assistantId, workspaceRef, st, expectedSessionId, get, set } = options;
  let scheduled = false;
  let rafId: number | null = null;
  let appliedUsage: TokenUsageEvent | null = null;
  let appliedTurnId: string | null = null;

  const commit = () => {
    scheduled = false;
    rafId = null;
    const current = workspaceRef.current;
    if (!current) return;
    let usageDelta: TokenUsageEvent | null = null;
    if (current.usage) {
      // A replay turn runs on the SAME committer — fresh turn resets the
      // applied-usage baseline so its usage is not double-counted.
      if (appliedTurnId !== current.turnId) {
        appliedUsage = null;
        appliedTurnId = current.turnId;
      }
      const prev = appliedUsage;
      usageDelta = prev
        ? {
            prompt_tokens: current.usage.prompt_tokens - prev.prompt_tokens,
            completion_tokens:
              current.usage.completion_tokens - prev.completion_tokens,
            cached_read_tokens:
              (current.usage.cached_read_tokens ?? 0) -
              (prev.cached_read_tokens ?? 0),
            reasoning_tokens:
              (current.usage.reasoning_tokens ?? 0) -
              (prev.reasoning_tokens ?? 0),
            prompt_cache_hit_tokens:
              (current.usage.prompt_cache_hit_tokens ?? 0) -
              (prev.prompt_cache_hit_tokens ?? 0),
            prompt_cache_miss_tokens:
              (current.usage.prompt_cache_miss_tokens ?? 0) -
              (prev.prompt_cache_miss_tokens ?? 0),
          }
        : current.usage;
      appliedUsage = current.usage;
    }
    const s = get();
    updateSessionMessages(
      st,
      expectedSessionId,
      (msgs) =>
        msgs.map((m) =>
          m.id === assistantId
            ? {
                ...m,
                blocks: mergeWorkspaceBlocks(current, m.blocks),
                tokenUsage: current.usage
                  ? {
                      prompt: current.usage.prompt_tokens,
                      completion: current.usage.completion_tokens,
                    }
                  : m.tokenUsage,
              }
            : m,
        ),
      get,
      set,
      usageDelta
        ? {
            totalTokens: {
              prompt: s.totalTokens.prompt + usageDelta.prompt_tokens,
              completion: s.totalTokens.completion + usageDelta.completion_tokens,
              cacheHit:
                s.totalTokens.cacheHit + (usageDelta.prompt_cache_hit_tokens ?? 0),
              cacheMiss:
                s.totalTokens.cacheMiss + (usageDelta.prompt_cache_miss_tokens ?? 0),
              cachedRead:
                s.totalTokens.cachedRead + (usageDelta.cached_read_tokens ?? 0),
              reasoning: s.totalTokens.reasoning + (usageDelta.reasoning_tokens ?? 0),
            },
          }
        : undefined,
    );
  };

  return {
    schedule: () => {
      if (scheduled) return;
      scheduled = true;
      rafId = requestAnimationFrame(() => {
        scheduled = false;
        rafId = null;
        commit();
      });
    },
    flush: () => {
      if (rafId !== null) cancelAnimationFrame(rafId);
      rafId = null;
      scheduled = false;
      commit();
    },
  };
}

/** Depwork document-dispatch side effects of a finished tool call (the block
 *  update itself lives in the workspace reducer). */
function applyToolResultEffects(
  ctx: TurnContext,
  event: Extract<ChatStreamEvent, { type: "tool_call_result" }>,
): void {
  const { mode } = ctx;
  // Document dispatch (depwork only): a successfully generated document is
  // selected for the workspace preview and the user is told where it went.
  // The depwork store is imported lazily (dynamic import) so the factory
  // module never takes a circular-import edge (appStore → depworkChatStore
  // → chatStore chain).
  if (mode === "depwork" && !event.is_error) {
    const path = extractDocumentPath(event.result);
    if (path) {
      const name = basenameOf(path);
      void import("@/stores/depworkStore").then((m) => {
        m.useDepworkStore.getState().selectFile({ name, path, isDir: false, size: null });
      });
      ctx.set({
        notification: i18n.t("notifications.fileGenerated", {
          name,
          defaultValue: `已生成 ${name}（右侧「工作区」可查看）`,
        }),
      });
    }
  }
}

/** One-shot first-token latency measurement — turn_start → first streamed
 *  token. Fires on the first reasoning/text delta, writes to the store, and
 *  disarms itself (a replay turn restarts the clock via turn_start). */
function recordFirstToken(ctx: TurnContext): void {
  if (ctx.turnStartTime.current === null) return;
  const latency = Date.now() - ctx.turnStartTime.current;
  ctx.turnStartTime.current = null;
  ctx.set({ firstTokenLatencyMs: latency });
}

/** Fold one stream delta into the workspace and schedule a commit.
 *  Structural events (tool start/result, mcp app, usage) commit
 *  synchronously — the old imperative handlers wrote them immediately;
 *  text/reasoning/progress/args ride the RAF batch. */
function handleStreamDelta(ctx: TurnContext, event: ChatStreamEvent): void {
  switch (event.type) {
    case "reasoning_delta":
      ctx.setPhase("thinking");
      recordFirstToken(ctx);
      break;
    case "text_delta":
      ctx.setPhase("generating");
      recordFirstToken(ctx);
      break;
    case "tool_call_start":
    case "tool_call_delta":
    case "tool_call_progress":
    case "tool_call_result":
    case "mcp_app":
      ctx.setPhase("tool_running");
      break;
    case "usage":
      break;
    default:
      return;
  }
  const ws = ctx.workspace.current;
  if (!ws) return;
  const next = reduceWorkspace(ws, event, ctx.mode);
  if (next === ws) return;
  ctx.workspace.current = next;
  if (event.type === "usage") {
    ctx.commit.flush();
    return;
  }
  if (
    event.type === "tool_call_start" ||
    event.type === "tool_call_result" ||
    event.type === "mcp_app"
  ) {
    ctx.commit.flush();
  } else {
    ctx.commit.schedule();
  }
  if (event.type === "tool_call_result") {
    applyToolResultEffects(ctx, event);
  }
}

function handleTurnStatus(
  ctx: TurnContext,
  event: Extract<ChatStreamEvent, { type: "turn_status" }>,
): void {
  // A stop-path gate held the turn (verification pending / evaluator
  // review / discipline nudge) — the streamed text so far is NOT final.
  // The phase event is the live signal; the turn may still stream more.
  if (event.phase === "verifying") ctx.setPhase("verifying");
}

function handleTurnEnd(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "turn_end" }>): void {
  const { st, expectedSessionId, gen, commit, finalizedRef, expectedTurnId } = ctx;
  commit.flush();
  // Not our session (e.g. a background subagent's turn)? Ignore.
  if (event.session_id !== expectedSessionId) return;
  // Stale turn? (a new sendMessage on THIS session bumped its generation) —
  // ignore so it can't flip the new turn out of streaming.
  if (gen !== st.gen) return;
  // Already finalized (a snapshot repaired the turn first)? Idempotent skip.
  if (finalizedRef.current) return;
  // Lost turn_start (SSE lag dropped it): the backend still emitted the
  // full turn plus a terminal snapshot, but the snapshot event is rejected
  // while expectedTurnId is null — leaving an EMPTY assistant message
  // ("model message not displayed"). Adopt the turn id from the terminal
  // event and pull the authoritative snapshot instead of finalizing empty.
  if (expectedTurnId.current === null) {
    expectedTurnId.current = event.turn_id;
    ctx.lastSeq.current = event.seq;
    ctx.pendingTerminal.current = event;
    requestTurnSnapshot(ctx);
    // Safety net: if the repair pull fails, finalize with whatever content
    // streamed so the turn never hangs. Same-turn only — a stop→resend bumps
    // gen, and this stale fallback must not finalize (it would null the new
    // listener's st.unlisten and flip the streaming bus off mid-turn).
    setTimeout(() => {
      const pending = ctx.pendingTerminal.current;
      ctx.pendingTerminal.current = null;
      if (pending && !ctx.finalizedRef.current && ctx.gen === ctx.st.gen) {
        finalizeTurnEnd(ctx, pending as Extract<ChatStreamEvent, { type: "turn_end" }>);
      }
    }, 2000);
    return;
  }
  finalizeTurnEnd(ctx, event);
}

/** The normal turn-end finalization — marks the message done and attaches
 *  the change summary. Shared by the direct path and the snapshot-repair
 *  fallback. */
function finalizeTurnEnd(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "turn_end" }>): void {
  const { get, set, st, assistantId, expectedSessionId, setPhase, unlistenRef, finalizedRef } = ctx;
  ctx.pendingTerminal.current = null;
  setPhase("idle");
  st.unlisten = null;
  syncStreamingBus(expectedSessionId, st);
  const s = get();
  updateSessionMessages(
    st,
    expectedSessionId,
    (msgs) =>
      msgs.map((m) => {
        if (m.id !== assistantId) return m;
        // Aggregate the turn's file edits into an end-of-turn change summary
        // (one entry per file, last edit wins). Generated once: a replayed or
        // stale turn_end never duplicates the block.
        if (m.blocks.some((b) => b.type === "changes_summary")) {
          return { ...m, isStreaming: false };
        }
        const changes = collectChanges(m.blocks);
        return {
          ...m,
          isStreaming: false,
          // `done` is the terminal accepted state; anything else is surfaced
          // as an explicit non-normal outcome (limit/cancelled/denied/failed).
          turnOutcome: (event.status ?? "done") as TurnOutcome,
          blocks: changes.length > 0 ? [...m.blocks, { type: "changes_summary", changes }] : m.blocks,
        };
      }),
    get,
    set,
    s.currentSessionId === expectedSessionId
      ? { memoryRef: null, firstTokenLatencyMs: null, isStreaming: false, isPaused: false, contextChips: [] }
      : undefined,
  );
  unlistenRef.current?.();
  // Replay finished — a future queued send starts its own listener.
  st.replayActive = false;
  // The finally block below must NOT re-run cleanup (double unlisten +
  // redundant state writes) — everything is finalized here.
  finalizedRef.current = true;
  // Auto-send any message queued for THIS session once its stream has fully
  // ended. The work mode was pinned when the message was queued — the user
  // may have switched surfaces mid-turn. When the session is NOT shown, the
  // queued text stays bound to its session (setSessionId replays it).
  const queued = st.queuedText;
  if (queued && get().currentSessionId === expectedSessionId) {
    st.queuedText = null;
    set({ queuedText: null, inputText: queued });
    setTimeout(() => {
      void get().sendMessage("queue");
    }, 0);
  }
}

function handleError(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "error" }>): void {
  const { st, expectedSessionId, gen, commit, finalizedRef, expectedTurnId } = ctx;
  commit.flush();
  if (event.session_id !== expectedSessionId) return;
  if (gen !== st.gen) return;
  // Already finalized? A turn-scoped error is redundant (turn_end carried
  // the terminal outcome). The EMPTY-turn_id session-level error signals a
  // failed replay backlog and must reach the listener or a replay wait hangs.
  // But if the turn ALREADY ended with an error block, a second drain error
  // must only release the stream — never append a duplicate error block.
  if (finalizedRef.current) {
    if (event.turn_id !== "") return;
    const alreadyError = st
      .messages.find((m) => m.id === ctx.assistantId)
      ?.blocks.some((b) => b.type === "error");
    if (alreadyError) {
      st.unlisten = null;
      st.replayActive = false;
      syncStreamingBus(expectedSessionId, st);
      ctx.setPhase("idle");
      return;
    }
    // Fall through — a cleanly-ended turn surfaces the late drain error.
  }
  // Same lost-turn_start repair as turn_end (turn-scoped errors only — the
  // empty-turn_id session-level error has no snapshot to pull).
  if (expectedTurnId.current === null && event.turn_id !== "") {
    expectedTurnId.current = event.turn_id;
    ctx.lastSeq.current = event.seq;
    ctx.pendingTerminal.current = event;
    requestTurnSnapshot(ctx);
    setTimeout(() => {
      const pending = ctx.pendingTerminal.current;
      ctx.pendingTerminal.current = null;
      if (pending && !ctx.finalizedRef.current && ctx.gen === ctx.st.gen) {
        finalizeError(ctx, pending as Extract<ChatStreamEvent, { type: "error" }>);
      }
    }, 2000);
    return;
  }
  finalizeError(ctx, event);
}

/** The normal error finalization — appends the error block and stops. */
function finalizeError(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "error" }>): void {
  const { get, set, st, assistantId, expectedSessionId, setPhase, unlistenRef, finalizedRef } = ctx;
  ctx.pendingTerminal.current = null;
  setPhase("idle");
  st.unlisten = null;
  syncStreamingBus(expectedSessionId, st);
  st.replayActive = false;
  const s = get();
  updateSessionMessages(
    st,
    expectedSessionId,
    (msgs) =>
      msgs.map((m) =>
        m.id === assistantId
          ? { ...m, blocks: [...m.blocks, { type: "error" as const, content: event.message }], isStreaming: false }
          : m,
      ),
    get,
    set,
    s.currentSessionId === expectedSessionId
      ? { memoryRef: null, firstTokenLatencyMs: null, isStreaming: false, isPaused: false, contextChips: [] }
      : undefined,
  );
  unlistenRef.current?.();
  // The finally block must not re-run cleanup — finalized here.
  finalizedRef.current = true;
  // A queued message must not be lost when the turn errors — restore it to
  // the input (when the session is shown) instead of auto-sending.
  const queuedOnError = st.queuedText;
  if (queuedOnError) {
    st.queuedText = null;
    set((s) => ({
      queuedText: s.currentSessionId === expectedSessionId ? null : s.queuedText,
      inputText: s.currentSessionId === expectedSessionId ? queuedOnError : s.inputText,
    }));
  }
}

/** Build canonical final message blocks from an authoritative snapshot.
 *  Reasoning first, text second, then each tool call in stream order with
 *  its terminal result and any attached MCP app. */
function blocksFromSnapshot(snapshot: TurnSnapshot): MessageBlock[] {
  const blocks: MessageBlock[] = [];
  if (snapshot.reasoning) blocks.push({ type: "reasoning", content: snapshot.reasoning });
  if (snapshot.text) blocks.push({ type: "text", content: snapshot.text });
  for (const t of snapshot.tool_calls) {
    const app = snapshot.mcp_apps.find((a) => a.call_id === t.call_id);
    blocks.push({
      type: "tool_call",
      tool: {
        id: t.call_id,
        name: t.name,
        arguments: t.arguments,
        status: t.is_error ? "error" : t.result != null ? "done" : "running",
        result: t.result ?? undefined,
        mcpApp: app
          ? {
              server: app.server,
              resource_uri: app.resource_uri,
              html: app.html,
              is_error: app.is_error,
              csp: app.csp as Record<string, unknown> | undefined,
            }
          : undefined,
      },
    });
  }
  return blocks;
}

/** Apply an authoritative snapshot to the turn's assistant message —
 *  idempotent: skips once the message is already finalized (normal path:
 *  turn_end finalized first, the trailing snapshot event is ignored). */
function applySnapshot(ctx: TurnContext, snapshot: TurnSnapshot): void {
  const { get, set, st, assistantId, expectedSessionId, gen, setPhase, commit, unlistenRef, finalizedRef } = ctx;
  if (snapshot.session_id !== expectedSessionId) return;
  if (gen !== st.gen) return;
  if (finalizedRef.current) return;
  // The repair landed — the fallback finalizer must not run afterwards.
  ctx.pendingTerminal.current = null;
  commit.flush();
  setPhase("idle");
  st.unlisten = null;
  syncStreamingBus(expectedSessionId, st);
  st.replayActive = false;
  const s = get();
  const blocks = blocksFromSnapshot(snapshot);
  const changes = collectChanges(blocks);
  updateSessionMessages(
    st,
    expectedSessionId,
    (msgs) =>
      msgs.map((m) =>
        m.id === assistantId
          ? {
              ...m,
              isStreaming: false,
              turnOutcome: snapshot.status,
              tokenUsage: snapshot.usage
                ? {
                    prompt: snapshot.usage.prompt_tokens,
                    completion: snapshot.usage.completion_tokens,
                  }
                : m.tokenUsage,
              blocks:
                changes.length > 0
                  ? [...blocks, { type: "changes_summary", changes }]
                  : blocks,
            }
          : m,
      ),
    get,
    set,
    s.currentSessionId === expectedSessionId
      ? { memoryRef: null, firstTokenLatencyMs: null, isStreaming: false, isPaused: false, contextChips: [] }
      : undefined,
  );
  unlistenRef.current?.();
  finalizedRef.current = true;
  // A queued message must not be lost when the turn was repaired — auto-send
  // exactly like a normal turn_end (only when the session is shown).
  const queued = st.queuedText;
  if (queued && get().currentSessionId === expectedSessionId) {
    st.queuedText = null;
    set({ queuedText: null, inputText: queued });
    setTimeout(() => {
      void get().sendMessage("queue");
    }, 0);
  }
}

/** A lost delta was detected — pull the authoritative terminal snapshot
  *  once per turn (mid-turn requests return None; live deltas keep flowing).
  *  Best-effort: a failed pull leaves the stream on live deltas. */
function requestTurnSnapshot(ctx: TurnContext): void {
  if (ctx.snapshotRequested.current) return;
  ctx.snapshotRequested.current = true;
  const turnId = ctx.expectedTurnId.current;
  if (!turnId) return;
  void chatApi
    .getTurnSnapshot(ctx.expectedSessionId, turnId)
    .then((snapshot) => {
      if (snapshot) applySnapshot(ctx, snapshot);
    })
    .catch(() => {
      // Best-effort repair — a failure leaves the stream on live deltas.
    });
}

/** Transport reconnect probe — the missed window may have ended the turn
 *  (its terminal events were never delivered), so pull the snapshot even
 *  when no seq gap was observed. A mid-turn probe returns None and the
 *  live stream simply resumes. */
function probeTurnSnapshot(ctx: TurnContext): void {
  if (!ctx.expectedTurnId.current) return;
  if (ctx.finalizedRef.current) return;
  // Allow a fresh pull per reconnect (the one-shot guard is per gap).
  ctx.snapshotRequested.current = false;
  requestTurnSnapshot(ctx);
}

/** Track the GLOBAL wire seq for every event and report whether a gap
 *  appeared. The backend seq is a single global counter (all sessions +
 *  subagents share the channel), so gap detection must compare against the
 *  last seq WE SAW — not the last seq of OUR turn. Per-turn tracking let a
 *  subagent/other-session event advance the global seq without updating
 *  lastSeq, so the next own-turn event looked like a lost delta and burned
 *  the once-per-turn snapshot pull. */
function checkSeqGap(ctx: TurnContext, event: ChatStreamEvent): boolean {
  const last = ctx.lastSeq.current;
  const gap = last !== null && event.seq > last + 1;
  ctx.lastSeq.current = last === null ? event.seq : Math.max(last, event.seq);
  return gap;
}

function handleSnapshot(ctx: TurnContext, event: Extract<ChatStreamEvent, { type: "snapshot" }>): void {
  if (event.snapshot.turn_id !== ctx.expectedTurnId.current) return;
  applySnapshot(ctx, event.snapshot);
}

/**
 * Turn-id guards shared by every streaming/tool/usage event. The backend's
 * cancel-notification Error carries an EMPTY turn_id (a turn failed before
 * its backlog drained — chat.rs); it is a session-level signal for replay
 * listeners, not a turn event, so it passes through.
 */
function passesTurnGuards(ctx: TurnContext, event: ChatStreamEvent): boolean {
  const { expectedTurnId, st } = ctx;
  if (expectedTurnId.current !== null && "turn_id" in event && event.turn_id !== expectedTurnId.current) {
    if (!(event.type === "error" && event.turn_id === "")) return false;
  }
  // Stale-turn guard: deltas of a turn this session already consumed (its
  // listener was torn down — stop→resend) can outlive their turn_start.
  // Without this check they'd bleed into the new listener's assistant
  // message, and the stale turn_end would cut the stream short.
  if ("turn_id" in event && event.turn_id === st.lastTurnId && expectedTurnId.current !== event.turn_id) {
    return false;
  }
  return true;
}

function dispatchTurnEvent(ctx: TurnContext, event: ChatStreamEvent): void {
  switch (event.type) {
    case "turn_status":
      return handleTurnStatus(ctx, event);
    case "snapshot":
      return handleSnapshot(ctx, event);
    case "turn_end":
      return handleTurnEnd(ctx, event);
    case "error":
      return handleError(ctx, event);
    default:
      return handleStreamDelta(ctx, event);
  }
}

export function buildStreamListener(options: StreamTurnOptions) {
  const { get, set, st, assistantId, expectedSessionId, gen, mode, finalizedRef, unlistenRef } = options;
  const workspace: { current: TurnWorkspace | null } = { current: null };
  const commit = createCommitter({ assistantId, workspaceRef: workspace, st, expectedSessionId, get, set });
  const expectedTurnId: { current: string | null } = { current: null };
  const turnStartTime: { current: number | null } = { current: null };
  const lastSeq: { current: number | null } = { current: null };
  const snapshotRequested: { current: boolean } = { current: false };
  const pendingTerminal: { current: ChatStreamEvent | null } = { current: null };
  // Turn phase tracker — inferred from the turn's OWN events (never the
  // global agent-status channel: it is a single slot and multi-session
  // concurrency would clobber it). Drives the StreamStatusLine.
  const setPhase = (phase: StreamPhase) => {
    st.phase = phase;
    if (get().currentSessionId === expectedSessionId) {
      set({ streamPhase: phase });
    }
  };
  const ctx: TurnContext = {
    get,
    set,
    st,
    assistantId,
    expectedSessionId,
    gen,
    mode,
    finalizedRef,
    unlistenRef,
    workspace,
    commit,
    expectedTurnId,
    turnStartTime,
    lastSeq,
    snapshotRequested,
    pendingTerminal,
    setPhase,
  };

  const handler = (event: ChatStreamEvent) => {
    // Track the global seq for EVERY event BEFORE the switch — subagent and
    // other-session events advance it too, and only this global comparison
    // can tell a real lost delta apart from a concurrent interleave.
    const gap = checkSeqGap(ctx, event);
    switch (event.type) {
      case "turn_start":
        return handleTurnStart(ctx, event);
      case "compaction":
        return handleCompaction(ctx, event);
      case "subagent_start":
        return handleSubagentStart(ctx, event);
      case "subagent_progress":
        return handleSubagentProgress(ctx, event);
      case "subagent_result":
        return handleSubagentResult(ctx, event);
      case "elicitation":
        return handleElicitation(ctx, event);
      case "memory_injected":
        return handleMemoryInjected(ctx, event);
      default:
        if (!passesTurnGuards(ctx, event)) return;
        if (gap) requestTurnSnapshot(ctx);
        return dispatchTurnEvent(ctx, event);
    }
  };

  return {
    handler,
    flushPending: commit.flush,
    flushProgress: commit.flush,
    /** Transport re-established after a drop — probe the terminal snapshot
     *  so a turn that ended while disconnected converges. */
    onTransportReconnect: () => probeTurnSnapshot(ctx),
  };
}
