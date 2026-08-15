/**
 * Task-turn streaming probe — "写官网" style turns (text → tool → text) must
 * keep the streaming text in the store mid-turn across a tool call. Guards
 * the regression where simple Q&A streamed but multi-step turns showed
 * nothing until the reply was fully done.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { useChatStore } from "@/stores/chatStore";
import { streamStates } from "@/stores/chatStore/streamState";
import type { ChatStreamEvent, SendChatResult, StreamEventShape } from "@/lib/tauri";

const hoisted = vi.hoisted(() => ({
  streamHandler: { current: null as ((p: ChatStreamEvent) => void) | null },
}));

vi.mock("@/lib/tauri/sse", () => ({
  connectChatStream: vi.fn(async (handler: (p: ChatStreamEvent) => void) => {
    hoisted.streamHandler.current = handler;
    // Real unlisten semantics — tearing down removes the listener. This is
    // what makes the invoke-resolve race real: a turn_end arriving after
    // unlisten() would never reach the handler.
    return () => {
      hoisted.streamHandler.current = null;
    };
  }),
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    chatApi: {
      ...actual.chatApi,
      sendMessage: vi.fn(
        async () =>
          ({ kind: "accepted", prompt_id: null, turn_id: "t1" }) as SendChatResult,
      ),
    },
    sessionApi: {
      ...actual.sessionApi,
      createSession: vi.fn(async () => ({ id: "s1" })),
      updateSessionTitle: vi.fn(async () => {}),
    },
  };
});

const MOCK_MODEL = {
  id: "deepseek-chat",
  name: "DeepSeek Chat",
  provider: "deepseek",
  providerId: "deepseek",
  description: "",
  context_window: 64000,
};

let seq = 0;
function evt(body: StreamEventShape): ChatStreamEvent {
  seq += 1;
  return { seq, ...body };
}

function flushCommit() {
  return new Promise<void>((r) => {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => r());
    });
  });
}

describe("task-turn streaming store", () => {
  beforeEach(() => {
    seq = 0;
    hoisted.streamHandler.current = null;
    // A prior test's turn never emitted turn_end — its listener stays armed
    // (the new finally keeps it alive for the trailing event). Clear the
    // per-session stream state so each test starts with a clean send path.
    streamStates.clear();
    useChatStore.setState({
      messages: [],
      currentSessionId: null,
      isStreaming: false,
      streamPhase: "idle",
      firstTokenLatencyMs: null,
      inputText: "",
      notification: null,
      totalTokens: { prompt: 0, completion: 0, cacheHit: 0, cacheMiss: 0, cachedRead: 0, reasoning: 0 },
    });
  });

  it("keeps streaming text in the store mid-turn across a tool call", async () => {
    useChatStore.setState({
      currentSessionId: "s1",
      selectedModel: MOCK_MODEL as never,
      inputText: "以你的能力来写一个官网",
    });
    const sendPromise = useChatStore.getState().sendMessage();
    await sendPromise;
    const emit = (p: ChatStreamEvent) => {
      const h = hoisted.streamHandler.current;
      if (!h) throw new Error("listener torn down before the terminal event");
      h(p);
    };

    emit(evt({ type: "turn_start", turn_id: "t1", session_id: "s1", model: "m" }));
    emit(evt({ type: "text_delta", turn_id: "t1", text: "好的，我来写一个官网。" }));
    await flushCommit();

    // A tool runs, then the reply continues — text before AND after the tool
    // must both be in the store mid-turn.
    emit(
      evt({
        type: "tool_call_start",
        turn_id: "t1",
        call_id: "c1",
        name: "write_file",
      }),
    );
    emit(
      evt({
        type: "tool_call_result",
        turn_id: "t1",
        call_id: "c1",
        name: "write_file",
        result: "wrote index.html",
        is_error: false,
      }),
    );
    emit(evt({ type: "text_delta", turn_id: "t1", text: "\n\n首页已写好。" }));
    await flushCommit();

    const msg = () =>
      useChatStore.getState().messages.find((m) => m.role === "assistant");
    const blocks = msg()?.blocks ?? [];
    const texts = blocks
      .filter((b) => b.type === "text")
      .map((b) => (b.type === "text" ? b.content : ""));
    expect(texts.some((t) => t.includes("官网"))).toBe(true);
    expect(texts.some((t) => t.includes("首页已写好"))).toBe(true);
    // The tool row landed between the two text blocks.
    expect(blocks.some((b) => b.type === "tool_call")).toBe(true);
  });

  it("turn_end landing AFTER the invoke resolves still finalizes (change summary + outcome)", async () => {
    // The invoke and the SSE channel are separate transports — the terminal
    // event can arrive after sendMessage() resolves. The finally block must
    // keep the listener armed so the trailing turn_end finalizes normally;
    // tearing it down would drop the change summary and turn outcome.
    useChatStore.setState({
      currentSessionId: "s1",
      selectedModel: MOCK_MODEL as never,
      inputText: "写一个文件",
    });
    const sendPromise = useChatStore.getState().sendMessage();
    await sendPromise; // invoke resolved BEFORE turn_end — the race window
    const emit = (p: ChatStreamEvent) => {
      const h = hoisted.streamHandler.current;
      if (!h) throw new Error("listener torn down before the terminal event");
      h(p);
    };

    emit(evt({ type: "turn_start", turn_id: "t1", session_id: "s1", model: "m" }));
    emit(evt({ type: "text_delta", turn_id: "t1", text: "好的。" }));
    emit(evt({ type: "tool_call_start", turn_id: "t1", call_id: "c1", name: "write_file" }));
    emit(
      evt({
        type: "tool_call_delta",
        turn_id: "t1",
        call_id: "c1",
        arguments: JSON.stringify({ path: "src/a.ts", content: "hi" }),
      }),
    );
    emit(
      evt({
        type: "tool_call_result",
        turn_id: "t1",
        call_id: "c1",
        name: "write_file",
        result: "wrote",
        is_error: false,
      }),
    );
    await flushCommit();

    // The terminal event lands now — after the invoke already resolved.
    emit(evt({ type: "turn_end", turn_id: "t1", session_id: "s1", reason: "stop" }));
    await flushCommit();

    const msg = () =>
      useChatStore.getState().messages.find((m) => m.role === "assistant");
    expect(msg()?.isStreaming).toBe(false);
    expect(msg()?.turnOutcome).toBe("done");
    // finalizeTurnEnd collects the turn's write_file edits into a summary.
    const summary = msg()?.blocks.find((b) => b.type === "changes_summary");
    expect(summary?.type).toBe("changes_summary");
  });

  it("records first-token latency from turn_start to the first delta", async () => {
    useChatStore.setState({
      currentSessionId: "s1",
      selectedModel: MOCK_MODEL as never,
      inputText: "hi",
      firstTokenLatencyMs: null,
    });
    const sendPromise = useChatStore.getState().sendMessage();
    await sendPromise;
    const emit = (p: ChatStreamEvent) => {
      const h = hoisted.streamHandler.current;
      if (!h) throw new Error("listener torn down before turn_start");
      h(p);
    };

    emit(evt({ type: "turn_start", turn_id: "t1", session_id: "s1", model: "m" }));
    // No token yet — the latency stays unset.
    expect(useChatStore.getState().firstTokenLatencyMs).toBeNull();
    emit(evt({ type: "text_delta", turn_id: "t1", text: "好的。" }));
    await flushCommit();
    // Measured once on the first delta; turn_start started the clock.
    const latency = useChatStore.getState().firstTokenLatencyMs;
    expect(latency).not.toBeNull();
    expect(latency!).toBeGreaterThanOrEqual(0);
  });
});
