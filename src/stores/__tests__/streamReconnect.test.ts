/**
 * Transport-reconnect robustness: the SSE connection drops mid-turn, and
 * the missed window may have ended the turn. The listener probes the
 * terminal snapshot on reconnect and converges the message.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { useChatStore } from "@/stores/chatStore";
import { streamStates } from "@/stores/chatStore/streamState";
import type { ChatStreamEvent, SendChatResult, StreamEventShape, TurnSnapshot } from "@/lib/tauri";

const hoisted = vi.hoisted(() => ({
  streamHandler: { current: null as ((p: ChatStreamEvent) => void) | null },
  onReconnect: { current: null as (() => void) | null },
  getTurnSnapshot: { current: null as ((s: string, t: string) => Promise<TurnSnapshot | null>) | null },
}));

vi.mock("@/lib/tauri/sse", () => ({
  connectChatStream: vi.fn(
    async (
      handler: (p: ChatStreamEvent) => void,
      opts?: { onReconnect?: () => void },
    ) => {
      hoisted.streamHandler.current = handler;
      hoisted.onReconnect.current = opts?.onReconnect ?? null;
      return () => {};
    },
  ),
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
      getTurnSnapshot: vi.fn(
        async (sessionId: string, turnId: string) =>
          hoisted.getTurnSnapshot.current?.(sessionId, turnId) ?? null,
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

async function sendSetup() {
  useChatStore.setState({
    currentSessionId: "s1",
    selectedModel: MOCK_MODEL as never,
    inputText: "hello",
  });
  const sendPromise = useChatStore.getState().sendMessage();
  await sendPromise;
  return (payload: ChatStreamEvent) => hoisted.streamHandler.current!(payload);
}

describe("stream reconnect probe", () => {
  beforeEach(() => {
    seq = 0;
    hoisted.streamHandler.current = null;
    hoisted.onReconnect.current = null;
    hoisted.getTurnSnapshot.current = null;
    // The prior test's session buffer must not bleed into the next sendSetup.
    streamStates.clear();
    useChatStore.setState({
      messages: [],
      currentSessionId: null,
      isStreaming: false,
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
    });
  });

  it("repairs a turn that ended while the connection was down", async () => {
    const emit = await sendSetup();
    emit(evt({ type: "turn_start", turn_id: "t-rc", session_id: "s1", model: "m" }));
    emit(evt({ type: "text_delta", turn_id: "t-rc", text: "partial" }));
    // The turn ended while disconnected — its turn_end/snapshot never
    // arrived; the reconnect probe pulls the authoritative state.
    hoisted.getTurnSnapshot.current = async () => ({
      turn_id: "t-rc",
      session_id: "s1",
      status: "done",
      reason: "stop",
      text: "repaired",
      reasoning: "",
      tool_calls: [],
      mcp_apps: [],
      usage: null,
      trace_id: null,
    });
    hoisted.onReconnect.current?.();

    await vi.waitFor(() => {
      const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
      expect(msg?.isStreaming).toBe(false);
      expect(msg?.turnOutcome).toBe("done");
    });
    const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
    const text = msg?.blocks.find((b) => b.type === "text");
    if (text?.type !== "text") throw new Error("text block missing");
    expect(text.content).toBe("repaired");
  });

  it("keeps the live stream when the probe finds no terminal state", async () => {
    const emit = await sendSetup();
    emit(evt({ type: "turn_start", turn_id: "t-live", session_id: "s1", model: "m" }));
    emit(evt({ type: "text_delta", turn_id: "t-live", text: "live" }));
    hoisted.getTurnSnapshot.current = async () => null;
    hoisted.onReconnect.current?.();

    await vi.waitFor(() => {
      const text = useChatStore
        .getState()
        .messages.find((m) => m.role === "assistant")
        ?.blocks.find((b) => b.type === "text");
      if (text?.type === "text") expect(text.content).toBe("live");
    });
    expect(useChatStore.getState().streamPhase).toBe("generating");
    emit(evt({ type: "turn_end", turn_id: "t-live", session_id: "s1", reason: "stop" }));
    expect(
      useChatStore.getState().messages.find((m) => m.role === "assistant")?.isStreaming,
    ).toBe(false);
  });
});
