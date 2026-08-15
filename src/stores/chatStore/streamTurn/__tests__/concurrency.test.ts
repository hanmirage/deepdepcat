/**
 * Multi-session concurrency: two sessions stream on the SAME channel. Each
 * listener must route only its own session's events — a foreign turn's
 * deltas/terminal events must never bleed into another session's message
 * or tear down its stream.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { useChatStore } from "@/stores/chatStore";
import { buildStreamListener } from "../listener";
import { streamState, streamStates } from "../../streamState";
import { chatApi } from "@/lib/tauri";
import type { ChatStreamEvent, StreamEventShape } from "@/lib/tauri";
import type { ChatState, ChatWorkMode } from "../../types";

let seq = 0;
function evt(body: StreamEventShape): ChatStreamEvent {
  seq += 1;
  return { seq, ...body };
}

function makeListener(sessionId: string, assistantId: string) {
  const st = streamState(sessionId);
  st.gen += 1;
  const turn = buildStreamListener({
    get: () => useChatStore.getState(),
    set: (partial) => useChatStore.setState(partial as Partial<ChatState>),
    st,
    assistantId,
    expectedSessionId: sessionId,
    gen: st.gen,
    mode: "code" as ChatWorkMode,
    finalizedRef: { current: false },
    unlistenRef: { current: null },
  });
  return turn;
}

describe("multi-session stream isolation", () => {
  beforeEach(() => {
    seq = 0;
    for (const key of [...streamStates.keys()]) streamStates.delete(key);
    useChatStore.setState({
      messages: [],
      currentSessionId: "s-a",
      isStreaming: true,
      streamPhase: "idle",
      inputText: "",
      notification: null,
      totalTokens: {
        prompt: 0,
        completion: 0,
        cacheHit: 0,
        cacheMiss: 0,
        cachedRead: 0,
        reasoning: 0,
      },
      subagents: {},
    });
  });

  it("routes interleaved events per session without cross-session bleed", () => {
    const turnA = makeListener("s-a", "a-1");
    const turnB = makeListener("s-b", "b-1");
    // Seed each session's OWN buffer; the store shows only the current one.
    streamState("s-a").messages = [
      { id: "a-1", role: "assistant", blocks: [], timestamp: 0, isStreaming: true },
    ];
    streamState("s-b").messages = [
      { id: "b-1", role: "assistant", blocks: [], timestamp: 0, isStreaming: true },
    ];
    useChatStore.setState({ messages: streamState("s-a").messages });

    turnA.handler(evt({ type: "turn_start", turn_id: "ta", session_id: "s-a", model: "m" }));
    turnB.handler(evt({ type: "turn_start", turn_id: "tb", session_id: "s-b", model: "m" }));
    // Interleaved deltas — each must land in its own message.
    turnA.handler(evt({ type: "text_delta", turn_id: "ta", text: "A1" }));
    turnB.handler(evt({ type: "text_delta", turn_id: "tb", text: "B1" }));
    turnA.handler(
      evt({ type: "tool_call_start", turn_id: "ta", call_id: "c1", name: "grep" }),
    );
    turnA.handler(evt({ type: "text_delta", turn_id: "ta", text: "A2" }));

    // B ends first — must finalize ONLY B; A keeps streaming.
    turnB.handler(evt({ type: "turn_end", turn_id: "tb", session_id: "s-b", reason: "stop" }));
    turnA.handler(evt({ type: "turn_end", turn_id: "ta", session_id: "s-a", reason: "stop" }));

    // A is the CURRENT session — its buffer syncs to store.messages.
    const msgA = useChatStore.getState().messages.find((m) => m.id === "a-1");
    // B is a background session — its buffer lives on the session, not the store.
    const msgB = streamState("s-b").messages.find((m) => m.id === "b-1");
    const textsA = msgA?.blocks
      .filter((b) => b.type === "text")
      .map((b) => (b.type === "text" ? b.content : ""));
    const textsB = msgB?.blocks
      .filter((b) => b.type === "text")
      .map((b) => (b.type === "text" ? b.content : ""));
    expect(textsA).toEqual(["A1", "A2"]);
    expect(textsB).toEqual(["B1"]);
    expect(msgA?.isStreaming).toBe(false);
    expect(msgB?.isStreaming).toBe(false);
    // The tool row landed in A only.
    expect(msgA?.blocks.some((b) => b.type === "tool_call")).toBe(true);
    expect(msgB?.blocks.some((b) => b.type === "tool_call")).toBe(false);
  });

  it("ignores a foreign session's terminal event for the active turn", () => {
    const turnA = makeListener("s-a", "a-2");
    streamState("s-a").messages = [{ id: "a-2", role: "assistant", blocks: [], timestamp: 0, isStreaming: true }];
    useChatStore.setState({ messages: streamState("s-a").messages });
    turnA.handler(evt({ type: "turn_start", turn_id: "ta", session_id: "s-a", model: "m" }));
    turnA.handler(evt({ type: "text_delta", turn_id: "ta", text: "mine" }));
    // A background subagent's turn on the same channel ends — it must not
    // finalize A's message or change its phase.
    turnA.handler(evt({ type: "turn_end", turn_id: "sub-1", session_id: "sub-1", reason: "stop" }));
    const msg = useChatStore.getState().messages.find((m) => m.id === "a-2");
    expect(msg?.isStreaming).toBe(true);
    turnA.handler(evt({ type: "turn_end", turn_id: "ta", session_id: "s-a", reason: "stop" }));
    expect(useChatStore.getState().messages.find((m) => m.id === "a-2")?.isStreaming).toBe(false);
  });

  it("repairs an empty turn when turn_start was lost (SSE lag)", async () => {
    const turn = makeListener("s-a", "a-3");
    streamState("s-a").messages = [{ id: "a-3", role: "assistant", blocks: [], timestamp: 0, isStreaming: true }];
    useChatStore.setState({ messages: streamState("s-a").messages });
    const getTurnSnapshot = vi
      .spyOn(chatApi, "getTurnSnapshot")
      .mockResolvedValue({
        turn_id: "ta",
        session_id: "s-a",
        status: "done",
        reason: "stop",
        text: "完整的最终回答",
        reasoning: "",
        tool_calls: [],
        mcp_apps: [],
        usage: null,
      });

    // turn_start + all deltas were dropped (broadcast lag) — only the
    // terminal events survive. Previously this finalized an EMPTY message
    // and rejected the repair snapshot ("model message not displayed").
    turn.handler(evt({ type: "turn_end", turn_id: "ta", session_id: "s-a", reason: "stop" }));
    turn.handler(
      evt({
        type: "snapshot",
        snapshot: {
          turn_id: "ta",
          session_id: "s-a",
          status: "done",
          reason: "stop",
          text: "完整的最终回答",
          reasoning: "",
          tool_calls: [],
          mcp_apps: [],
          usage: null,
        },
      }),
    );

    await vi.waitFor(() => {
      const msg = useChatStore.getState().messages.find((m) => m.id === "a-3");
      expect(msg?.isStreaming).toBe(false);
    });
    const msg = useChatStore.getState().messages.find((m) => m.id === "a-3");
    const texts = msg?.blocks
      .filter((b) => b.type === "text")
      .map((b) => (b.type === "text" ? b.content : ""));
    expect(texts).toEqual(["完整的最终回答"]);
    expect(getTurnSnapshot).toHaveBeenCalledWith("s-a", "ta");
  });

  it("falls back to finalizing when the snapshot repair fails", async () => {
    vi.useFakeTimers();
    const turn = makeListener("s-a", "a-4");
    streamState("s-a").messages = [{ id: "a-4", role: "assistant", blocks: [], timestamp: 0, isStreaming: true }];
    useChatStore.setState({ messages: streamState("s-a").messages });
    vi.spyOn(chatApi, "getTurnSnapshot").mockRejectedValue(new Error("pull failed"));

    turn.handler(evt({ type: "turn_end", turn_id: "tb", session_id: "s-a", reason: "stop" }));
    // The repair pull failed — after the safety-net delay the turn must
    // finalize anyway (never hang, never leave a stuck spinner).
    await vi.advanceTimersByTimeAsync(2500);
    const msg = useChatStore.getState().messages.find((m) => m.id === "a-4");
    expect(msg?.isStreaming).toBe(false);
    vi.useRealTimers();
  });

  it("does not treat a subagent interleave as a lost delta (global seq)", () => {
    const turn = makeListener("s-a", "a-5");
    streamState("s-a").messages = [{ id: "a-5", role: "assistant", blocks: [], timestamp: 0, isStreaming: true }];
    useChatStore.setState({ messages: streamState("s-a").messages });
    const getTurnSnapshot = vi.spyOn(chatApi, "getTurnSnapshot").mockResolvedValue(null);
    // Prior tests in this file also spy on getTurnSnapshot; clear the
    // accumulated call history so this test only sees its own.
    getTurnSnapshot.mockClear();

    turn.handler(evt({ type: "turn_start", turn_id: "ta", session_id: "s-a", model: "m" }));
    // A background subagent emits events on the same channel — they advance
    // the GLOBAL seq but belong to a different turn. Gap detection must track
    // the global seq, not just this turn's, or the next own-turn delta looks
    // like a lost event and burns the once-per-turn snapshot pull.
    turn.handler(
      evt({ type: "subagent_start", subagent_id: "sub", task: "t", agent_type: "general", session_id: "s-a" }),
    );
    turn.handler(
      evt({ type: "subagent_progress", subagent_id: "sub", message: "m", turn: 1, total_turns: 1, session_id: "s-a" }),
    );
    turn.handler(evt({ type: "text_delta", turn_id: "ta", text: "after subagent" }));

    expect(getTurnSnapshot).not.toHaveBeenCalled();
  });

  it("keeps a mid-turn session in its buffer across a switch back", () => {
    // The regression: switching away (setSessionId) replaces store.messages
    // with the new session's view, and the backend has NOT persisted the
    // in-flight turn — so the mid-turn reply must survive in the session's
    // OWN buffer and be restored intact when switching back.
    const turnA = makeListener("s-a", "a-sw");
    streamState("s-a").messages = [
      { id: "a-sw", role: "assistant", blocks: [], timestamp: 0, isStreaming: true },
    ];
    useChatStore.setState({ messages: streamState("s-a").messages });

    turnA.handler(evt({ type: "turn_start", turn_id: "ta", session_id: "s-a", model: "m" }));
    turnA.handler(evt({ type: "text_delta", turn_id: "ta", text: "part1" }));
    turnA.flushPending();
    const hasText = (m: { blocks: { type: string }[] }) =>
      m.blocks.some((b) => b.type === "text");
    expect(hasText(useChatStore.getState().messages.find((m) => m.id === "a-sw")!)).toBe(true);

    // Switch away — the store now shows s-b's (empty) view; s-a's in-flight
    // turn stays in ITS buffer.
    useChatStore.getState().setSessionId("s-b");
    expect(useChatStore.getState().messages).toEqual([]);
    const buffered = streamState("s-a").messages.find((m) => m.id === "a-sw");
    expect(buffered?.isStreaming).toBe(true);

    // The background turn keeps streaming into its buffer — NOT the store.
    turnA.handler(evt({ type: "text_delta", turn_id: "ta", text: "part2" }));
    turnA.flushPending();
    expect(useChatStore.getState().messages).toEqual([]);
    const bufferedTexts = streamState("s-a")
      .messages.find((m) => m.id === "a-sw")
      ?.blocks.filter((b) => b.type === "text")
      .map((b) => (b.type === "text" ? b.content : ""));
    // Consecutive text deltas merge into one block (appendText).
    expect(bufferedTexts).toEqual(["part1part2"]);

    // Switch back — the in-flight turn is restored intact, then finalizes.
    useChatStore.getState().setSessionId("s-a");
    const restored = useChatStore.getState().messages.find((m) => m.id === "a-sw");
    expect(restored?.isStreaming).toBe(true);
    expect(
      restored?.blocks.filter((b) => b.type === "text").map((b) => (b.type === "text" ? b.content : "")),
    ).toEqual(["part1part2"]);

    turnA.handler(evt({ type: "turn_end", turn_id: "ta", session_id: "s-a", reason: "stop" }));
    expect(useChatStore.getState().messages.find((m) => m.id === "a-sw")?.isStreaming).toBe(false);
  });

  it("setMessages merges restored history with an in-flight buffer (no wipe)", () => {
    // Session restore calls setSessionId then setMessages(history). History
    // does not contain the mid-stream turn (backend persists only at turn
    // end) — setMessages must keep the in-flight message instead of wiping it.
    const st = streamState("s-a");
    st.messages = [
      { id: "a-inflight", role: "assistant", blocks: [{ type: "text", content: "mid" }], timestamp: 0, isStreaming: true },
      { id: "u-old", role: "user", blocks: [{ type: "text", content: "old" }], timestamp: 0 },
    ];
    useChatStore.setState({ currentSessionId: "s-a" });
    useChatStore.getState().setMessages([
      { id: "u-old", role: "user", blocks: [{ type: "text", content: "old" }], timestamp: 0 },
      { id: "a-old", role: "assistant", blocks: [{ type: "text", content: "done" }], timestamp: 1 },
    ]);
    const messages = useChatStore.getState().messages;
    expect(messages.find((m) => m.id === "a-inflight")).toBeDefined();
    expect(messages.find((m) => m.id === "a-old")?.blocks).toBeDefined();
    // u-old appears once (history won over the buffer duplicate).
    expect(messages.filter((m) => m.id === "u-old")).toHaveLength(1);
  });
});
