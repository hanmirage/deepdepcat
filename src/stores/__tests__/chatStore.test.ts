import { describe, it, expect, beforeEach, vi } from "vitest";
import { useChatStore } from "@/stores/chatStore";
import { streamStates } from "@/stores/chatStore/streamState";
import { chatApi, sessionApi } from "@/lib/tauri";
import { useSettingsStore } from "@/stores/settingsStore";
import {
  trimStreamOverlap,
  inferStreamPhase,
  summarizeSubagentResult,
  collectChanges,
  type MessageBlock,
} from "@/types/chat";
import type { ChatStreamEvent, SendChatResult, StreamEventShape } from "@/lib/tauri";

// Captured chat-stream handler — tests drive the store by emitting events.
const { capturedHandler } = vi.hoisted(() => ({
  capturedHandler: { current: null as ((payload: unknown) => void) | null },
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    chatApi: {
      ...actual.chatApi,
      sendMessage: vi.fn(async () => ({
        kind: "accepted",
        prompt_id: null,
        turn_id: "t1",
      })),
      getTurnSnapshot: vi.fn(async () => null),
    },
    sessionApi: {
      ...actual.sessionApi,
      createSession: vi.fn(async () => ({ id: "s1" } as never)),
      updateSessionTitle: vi.fn(async () => {}),
    },
  };
});

// The SSE transport subscribes via core's onEvent (not the barrel), so the
// capture mock must live on the core module to intercept chat-stream events.
vi.mock("@/lib/tauri/core", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri/core")>();
  return {
    ...actual,
    onEvent: vi.fn(async (_name: string, handler: (payload: unknown) => void) => {
      capturedHandler.current = handler;
      return () => {};
    }),
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

/** Monotonic mock wire seq — reset per test so fixtures stay deterministic. */
let mockSeq = 0;

/** Envelope a raw event body with the next wire seq (mirrors the backend). */
function evt(body: StreamEventShape): ChatStreamEvent {
  mockSeq += 1;
  return { seq: mockSeq, ...body };
}

/** Send a message through the real sendMessage pipeline (mocked backend)
 *  and wait until the turn has been fully set up. */
async function sendSetup(message = "hello") {
  useChatStore.setState({
    currentSessionId: "s1",
    selectedModel: MOCK_MODEL as never,
    inputText: message,
  });
  const sendPromise = useChatStore.getState().sendMessage();
  await sendPromise;
  expect(capturedHandler.current).not.toBeNull();
  return (payload: ChatStreamEvent) => capturedHandler.current!(payload);
}

function turnEvents(turnId = "t1", sessionId = "s1") {
  return {
    turnStart: evt({ type: "turn_start", turn_id: turnId, session_id: sessionId, model: "deepseek-chat" }),
    reasoning: evt({ type: "reasoning_delta", turn_id: turnId, text: "let me think" }),
    text: evt({ type: "text_delta", turn_id: turnId, text: "hello" }),
    toolStart: evt({ type: "tool_call_start", turn_id: turnId, call_id: "c1", name: "grep" }),
    turnEnd: evt({ type: "turn_end", turn_id: turnId, session_id: sessionId, reason: "stop" }),
  };
}

describe("chatStore", () => {
  beforeEach(() => {
    mockSeq = 0;
    // A prior test's session state (incl. its message buffer) must not bleed
    // into the next sendSetup — sendMessage reuses the same streamState("s1").
    streamStates.clear();
    // Reset to initial state before each test
    useChatStore.setState({
      messages: [],
      currentSessionId: null,
      isStreaming: false,
      streamPhase: "idle",
      inputText: "",
      notification: null,
      totalTokens: { prompt: 0, completion: 0, cacheHit: 0, cacheMiss: 0, cachedRead: 0, reasoning: 0 },
    });
  });

  describe("initial state", () => {
    it("starts with no messages", () => {
      expect(useChatStore.getState().messages).toEqual([]);
    });

    it("starts with no session", () => {
      expect(useChatStore.getState().currentSessionId).toBeNull();
    });

    it("starts not streaming", () => {
      expect(useChatStore.getState().isStreaming).toBe(false);
    });

    it("starts with empty input", () => {
      expect(useChatStore.getState().inputText).toBe("");
    });

    it("starts with zero tokens", () => {
      expect(useChatStore.getState().totalTokens).toEqual({ prompt: 0, completion: 0, cacheHit: 0, cacheMiss: 0, cachedRead: 0, reasoning: 0 });
    });
  });

  describe("setInputText", () => {
    it("updates the input text", () => {
      useChatStore.getState().setInputText("hello world");
      expect(useChatStore.getState().inputText).toBe("hello world");
    });
  });

  describe("dismissNotification", () => {
    it("clears the notification", () => {
      useChatStore.setState({ notification: "test notification" });
      useChatStore.getState().dismissNotification();
      expect(useChatStore.getState().notification).toBeNull();
    });
  });

  describe("clearMessages", () => {
    it("clears messages and resets session", () => {
      useChatStore.setState({
        messages: [
          { id: "1", role: "user", blocks: [{ type: "text", content: "test" }], timestamp: 0 },
        ],
        currentSessionId: "test-session",
        totalTokens: { prompt: 100, completion: 50, cacheHit: 0, cacheMiss: 0, cachedRead: 0, reasoning: 0 },
      });

      useChatStore.getState().clearMessages();

      expect(useChatStore.getState().messages).toEqual([]);
      expect(useChatStore.getState().currentSessionId).toBeNull();
      expect(useChatStore.getState().totalTokens).toEqual({ prompt: 0, completion: 0, cacheHit: 0, cacheMiss: 0, cachedRead: 0, reasoning: 0 });
    });
  });

  describe("stream phase tracking", () => {
    it("tracks connecting → thinking → generating → tool → idle", async () => {
      const emit = await sendSetup();
      const ev = turnEvents();

      emit(ev.turnStart);
      expect(useChatStore.getState().streamPhase).toBe("connecting");

      emit(ev.reasoning);
      expect(useChatStore.getState().streamPhase).toBe("thinking");

      emit(ev.text);
      expect(useChatStore.getState().streamPhase).toBe("generating");

      emit(ev.toolStart);
      expect(useChatStore.getState().streamPhase).toBe("tool_running");

      // Text resumes after the tool → back to generating.
      emit(ev.text);
      expect(useChatStore.getState().streamPhase).toBe("generating");

      emit(ev.turnEnd);
      expect(useChatStore.getState().streamPhase).toBe("idle");
    });

    it("resets to idle on turn error", async () => {
      const emit = await sendSetup();
      const ev = turnEvents("t2");

      emit(ev.turnStart);
      emit(ev.text);
      expect(useChatStore.getState().streamPhase).toBe("generating");

  emit(evt({ type: "error", turn_id: "t2", session_id: "s1", message: "boom" }));
      expect(useChatStore.getState().streamPhase).toBe("idle");
    });

    it("ignores events from other sessions", async () => {
      const emit = await sendSetup();
      const ev = turnEvents("t3");

      emit({ ...ev.turnStart, session_id: "other-session" } as ChatStreamEvent);
      expect(useChatStore.getState().streamPhase).toBe("idle");

      emit(ev.turnStart);
      expect(useChatStore.getState().streamPhase).toBe("connecting");
    });

    it("rejects replayed events from an already-consumed turn (stop→resend)", async () => {
      // A previous listener consumed turn t4; its in-flight events (stale
      // turn_start, deltas, turn_end) must not bleed into the new listener —
      // they'd hijack expectedTurnId and finalize the new message early
      // (killing the "queued:" replay listener before the replay starts).
      const emit = await sendSetup("first");
      const ev = turnEvents("t4");
      emit(ev.turnStart);
      emit(ev.text);
      emit(ev.turnEnd);

      // Second send on the SAME session — its stream state carries
      // lastTurnId = "t4". Events of the old turn are re-emitted (the
      // backend drains it after a stop→resend) and must be ignored…
      await sendSetup("second");
  emit(evt({ type: "turn_start", turn_id: "t4", session_id: "s1", model: "m" }));
      expect(useChatStore.getState().streamPhase).toBe("idle");
  emit(evt({ type: "text_delta", turn_id: "t4", text: "stale text" }));
      expect(useChatStore.getState().streamPhase).toBe("idle");
  emit(evt({ type: "turn_end", turn_id: "t4", session_id: "s1", reason: "stop" }));

      // …while the replay's FRESH turn id passes through normally — had the
      // stale turn_end been accepted, it would have torn the listener down
      // and this fresh turn_start would never register.
  emit(evt({ type: "turn_start", turn_id: "t5", session_id: "s1", model: "m" }));
      expect(useChatStore.getState().streamPhase).toBe("connecting");
  emit(evt({ type: "text_delta", turn_id: "t5", text: "replay" }));
      expect(useChatStore.getState().streamPhase).toBe("generating");
    });

    it("lets the backend's empty-turn_id error through a replay wait", async () => {
      // The backend notifies replay listeners of a failed backlog turn with
      // an EMPTY turn_id — the stale-turn guard must not swallow it (the
      // replay wait would hang forever).
      const emit = await sendSetup();
      const ev = turnEvents("t6");
      emit(ev.turnStart);
  emit(evt({ type: "error", turn_id: "", session_id: "s1", message: "drain failed" }));
      const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
      const errBlock = msg?.blocks.find((b) => b.type === "error");
      expect(errBlock?.type).toBe("error");
      expect(useChatStore.getState().streamPhase).toBe("idle");
    });

    it("finalizes a replayed turn on the SAME listener (queued replay)", async () => {
      // First turn completes normally on this listener…
      const emit = await sendSetup();
      const ev = turnEvents("t-first");
      emit(ev.turnStart);
      emit(ev.text);
      emit(ev.turnEnd);
      expect(
        useChatStore.getState().messages.find((m) => m.role === "assistant")?.isStreaming,
      ).toBe(false);

      // …then the backend replays the queued prompt on the SAME listener
      // with a fresh turn id — its turn_end must finalize, not be skipped
      // by the first turn's finalize flag.
      emit(evt({ type: "turn_start", turn_id: "t-replay", session_id: "s1", model: "m" }));
      emit(evt({ type: "text_delta", turn_id: "t-replay", text: "replayed" }));
      emit(evt({ type: "turn_end", turn_id: "t-replay", session_id: "s1", reason: "stop" }));
      const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
      expect(msg?.isStreaming).toBe(false);
      // The replay answer lands as its OWN text segment (the first turn's
      // text block keeps its identity; the workspace appends a new one).
      const textBlock = [...(msg?.blocks ?? [])].reverse().find((b) => b.type === "text");
      if (textBlock?.type !== "text") throw new Error("text block missing");
      expect(textBlock.content).toContain("replayed");
    });

    it("still surfaces a session-level drain error after the turn finalized", async () => {
      // The turn ended cleanly; the backend later fails while draining the
      // queued backlog. The empty-turn_id error must reach the listener (a
      // replay wait would hang forever) even though the turn is finalized.
      const emit = await sendSetup();
      const ev = turnEvents("t-drain-late");
      emit(ev.turnStart);
      emit(ev.text);
      emit(ev.turnEnd);
      emit(evt({ type: "error", turn_id: "", session_id: "s1", message: "drain failed" }));
      const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
      const errBlock = msg?.blocks.find((b) => b.type === "error");
      expect(errBlock?.type).toBe("error");
      expect(useChatStore.getState().streamPhase).toBe("idle");
    });

    it("shows the verifying phase when a stop-path gate holds the turn", async () => {
      const emit = await sendSetup();
      const ev = turnEvents("t-verify");

      emit(ev.turnStart);
      emit(ev.text);
      expect(useChatStore.getState().streamPhase).toBe("generating");

      emit(
        evt({
          type: "turn_status",
          turn_id: "t-verify",
          session_id: "s1",
          phase: "verifying",
          reason: "verify",
        }),
      );
      expect(useChatStore.getState().streamPhase).toBe("verifying");

      // The held turn may stream more content (verification output)…
      emit(evt({ type: "text_delta", turn_id: "t-verify", text: " running tests" }));
      expect(useChatStore.getState().streamPhase).toBe("generating");

      // …and finally ends with its terminal status.
      emit(
        evt({
          type: "turn_end",
          turn_id: "t-verify",
          session_id: "s1",
          reason: "stop",
          status: "done",
        }),
      );
      expect(useChatStore.getState().streamPhase).toBe("idle");
    });

    it("records the terminal status on the turn's assistant message", async () => {
      const emit = await sendSetup();
      const ev = turnEvents("t-status");

      emit(ev.turnStart);
      emit(ev.text);
      emit(
        evt({
          type: "turn_end",
          turn_id: "t-status",
          session_id: "s1",
          reason: "length",
          status: "limit",
        }),
      );

      const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
      expect(msg?.turnOutcome).toBe("limit");
      expect(msg?.isStreaming).toBe(false);
    });

    describe("seq gap → snapshot repair", () => {
      it("finalizes from the authoritative snapshot when a delta was lost", async () => {
        const emit = await sendSetup();
        vi.mocked(chatApi.getTurnSnapshot).mockResolvedValue({
          turn_id: "t-gap",
          session_id: "s1",
          status: "done",
          reason: "stop",
          text: "repaired",
          reasoning: "think",
          tool_calls: [
            {
              call_id: "c1",
              name: "grep",
              arguments: "{\"q\":\"x\"}",
              result: "hit",
              is_error: false,
            },
          ],
          mcp_apps: [],
          usage: null,
          trace_id: null,
        });

        emit({ seq: 1, type: "turn_start", turn_id: "t-gap", session_id: "s1", model: "m" });
        emit({ seq: 2, type: "text_delta", turn_id: "t-gap", text: "partial" });
        // seq 3 lost — the next event's gap triggers one snapshot pull.
        emit({ seq: 4, type: "text_delta", turn_id: "t-gap", text: " more" });

        await vi.waitFor(() => {
          expect(vi.mocked(chatApi.getTurnSnapshot)).toHaveBeenCalledWith("s1", "t-gap");
        });
        await vi.waitFor(() => {
          const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
          expect(msg?.isStreaming).toBe(false);
          expect(msg?.turnOutcome).toBe("done");
        });
        const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
        const textBlock = msg?.blocks.find((b) => b.type === "text");
        if (textBlock?.type !== "text") throw new Error("text block missing");
        expect(textBlock.content).toBe("repaired");
        const tool = msg?.blocks.find((b) => b.type === "tool_call");
        if (tool?.type !== "tool_call") throw new Error("tool block missing");
        expect(tool.tool.result).toBe("hit");
        // The repair is single-shot — a second gap must not re-pull.
        vi.mocked(chatApi.getTurnSnapshot).mockClear();
        emit({ seq: 5, type: "turn_end", turn_id: "t-gap", session_id: "s1", reason: "stop" });
        expect(vi.mocked(chatApi.getTurnSnapshot)).not.toHaveBeenCalled();
      });

      it("keeps streaming when the snapshot pull finds no terminal state", async () => {
        const emit = await sendSetup();
        vi.mocked(chatApi.getTurnSnapshot).mockResolvedValue(null);
        emit({ seq: 1, type: "turn_start", turn_id: "t-mid", session_id: "s1", model: "m" });
        emit({ seq: 2, type: "text_delta", turn_id: "t-mid", text: "live" });
        emit({ seq: 4, type: "text_delta", turn_id: "t-mid", text: " still live" });
        await vi.waitFor(() => {
          expect(vi.mocked(chatApi.getTurnSnapshot)).toHaveBeenCalledWith("s1", "t-mid");
        });
        const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
        // Mid-turn snapshot pull returned None — live deltas remain the
        // source of truth (the mock invoke resolves immediately, so the
        // harness message flag is already false; the phase + content prove
        // the listener kept streaming).
        expect(useChatStore.getState().streamPhase).toBe("generating");
        await vi.waitFor(() => {
          const textBlock = useChatStore
            .getState()
            .messages.find((m) => m.role === "assistant")
            ?.blocks.find((b) => b.type === "text");
          expect(textBlock?.type).toBe("text");
          if (textBlock?.type === "text") expect(textBlock.content).toBe("live still live");
        });
        emit({ seq: 5, type: "turn_end", turn_id: "t-mid", session_id: "s1", reason: "stop" });
        expect(useChatStore.getState().messages.find((m) => m.role === "assistant")?.isStreaming).toBe(false);
      });

      it("ignores a trailing snapshot event after turn_end finalized the message", async () => {
        const emit = await sendSetup();
        const ev = turnEvents("t-snap-late");
        emit(ev.turnStart);
        emit(ev.text);
        emit(ev.turnEnd);
        const before = JSON.stringify(
          useChatStore.getState().messages.find((m) => m.role === "assistant")?.blocks,
        );
        emit(
          evt({
            type: "snapshot",
            snapshot: {
              turn_id: "t-snap-late",
              session_id: "s1",
              status: "done",
              reason: "stop",
              text: "should-not-replace",
              reasoning: "",
              tool_calls: [],
              mcp_apps: [],
            },
          }),
        );
        const after = JSON.stringify(
          useChatStore.getState().messages.find((m) => m.role === "assistant")?.blocks,
        );
        expect(after).toBe(before);
      });
    });

    it("flushes streamed text and reasoning into the assistant message", async () => {
      // Regression: the flush accumulator must write into the SAME closure
      // state that flushPending reads — a by-value return would silently
      // drop every text/reasoning delta (tools still rendered, text never
      // appeared).
      const emit = await sendSetup();
      const ev = turnEvents("t-text");

      emit(ev.turnStart);
      emit(ev.reasoning);
      emit(ev.text);
      emit(evt({ type: "text_delta", turn_id: "t-text", text: " world" }));
      emit(ev.turnEnd);

      const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
      expect(msg).toBeDefined();
      const textBlock = msg?.blocks.find((b) => b.type === "text");
      const reasoningBlock = msg?.blocks.find((b) => b.type === "reasoning");
      expect(reasoningBlock?.type).toBe("reasoning");
      if (reasoningBlock?.type === "reasoning") {
        expect(reasoningBlock.content).toBe("let me think");
      }
      expect(textBlock?.type).toBe("text");
      if (textBlock?.type === "text") {
        expect(textBlock.content).toBe("hello world");
      }
      expect(msg?.isStreaming).toBe(false);
    });

    it("keeps a late reasoning delta ahead of the text blocks", async () => {
      // Multi-provider streams can emit text BEFORE the reasoning tail (the
      // backend emits reasoning first, but interleaved streams don't
      // guarantee it). The flush must insert late reasoning ahead of the
      // answer instead of appending it below.
      const emit = await sendSetup();
      const ev = turnEvents("t-late-reasoning");

      emit(ev.turnStart);
      emit(evt({ type: "text_delta", turn_id: "t-late-reasoning", text: "hello" }));
      emit(evt({ type: "reasoning_delta", turn_id: "t-late-reasoning", text: "late think" }));
      emit(ev.turnEnd);

      const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
      expect(msg).toBeDefined();
      const textIdx = msg?.blocks.findIndex((b) => b.type === "text") ?? -1;
      const reasoningIdx = msg?.blocks.findIndex((b) => b.type === "reasoning") ?? -1;
      expect(reasoningIdx).toBeGreaterThanOrEqual(0);
      expect(reasoningIdx).toBeLessThan(textIdx);
      const reasoningBlock = msg?.blocks[reasoningIdx];
      if (reasoningBlock?.type === "reasoning") {
        expect(reasoningBlock.content).toBe("late think");
      }
    });
  });

  describe("MCP Apps", () => {
    it("attaches the mcp_app payload to its tool call block", async () => {
      const emit = await sendSetup();
      const ev = turnEvents("t7");

      emit(ev.turnStart);
      emit(evt({ type: "tool_call_start", turn_id: "t7", call_id: "c1", name: "mcp__dashboard" }));
      emit(evt({ type: "tool_call_result", turn_id: "t7", call_id: "c1", name: "mcp__dashboard", result: "ok", is_error: false }));
      emit(
        evt({
          type: "mcp_app",
          turn_id: "t7",
          call_id: "c1",
          name: "mcp__dashboard",
          server: "charts",
          resource_uri: "ui://app/dashboard",
          html: "<!DOCTYPE html><h1>dash</h1>",
          is_error: false,
        }),
      );

      const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
      const block = msg?.blocks.find((b) => b.type === "tool_call" && b.tool.id === "c1");
      expect(block?.type).toBe("tool_call");
      if (block?.type !== "tool_call") return;
      expect(block.tool.mcpApp).toEqual({
        server: "charts",
        resource_uri: "ui://app/dashboard",
        html: "<!DOCTYPE html><h1>dash</h1>",
        is_error: false,
      });
    });

    it("ignores mcp_app for unknown call ids", async () => {
      const emit = await sendSetup();
      const ev = turnEvents("t8");

      emit(ev.turnStart);
      emit(evt({ type: "tool_call_start", turn_id: "t8", call_id: "c1", name: "grep" }));
      emit(
        evt({
          type: "mcp_app",
          turn_id: "t8",
          call_id: "ghost",
          name: "n",
          server: "s",
          resource_uri: "ui://x",
          html: "<h1/>",
          is_error: false,
        }),
      );

      const msg = useChatStore.getState().messages.find((m) => m.role === "assistant");
      const block = msg?.blocks.find((b) => b.type === "tool_call" && b.tool.id === "c1");
      expect(block?.type).toBe("tool_call");
      if (block?.type !== "tool_call") return;
      expect(block.tool.mcpApp).toBeUndefined();
    });
  });

  describe("queue-and-send while streaming", () => {
    it("queues a message sent mid-stream and auto-sends it after turn_end", async () => {
      // Turn A is streaming (the invoke stays pending until turn_end, like
      // the real backend — the default mock resolves instantly and would
      // tear the listener down before the second send).
      useChatStore.setState({
        currentSessionId: "s1",
        selectedModel: MOCK_MODEL as never,
        inputText: "first",
      });
      let resolveSendA!: (v: SendChatResult) => void;
      vi.mocked(chatApi.sendMessage).mockImplementationOnce(
        () => new Promise<SendChatResult>((r) => { resolveSendA = r; }),
      );
      const sendCallsBefore = vi.mocked(chatApi.sendMessage).mock.calls.length;
      const sendPromiseA = useChatStore.getState().sendMessage();
      // Wait until sendMessage registers the stream listener and starts the
      // backend call (it awaits ensureSession, then connectChatStream, which
      // falls back to the event bus after the port probe fails) — the
      // listener must exist and the session must be marked streaming.
      await vi.waitFor(() => {
        expect(vi.mocked(chatApi.sendMessage).mock.calls.length).toBe(sendCallsBefore + 1);
      });
      const emitA = capturedHandler.current!;
      const evA = turnEvents("q1");
      emitA(evA.turnStart);
      emitA(evA.text);

      // The user sends message B mid-stream — it queues (chip + cleared
      // input), never blocks or drops.
      useChatStore.setState({ inputText: "second" });
      const sendPromiseB = useChatStore.getState().sendMessage("queue");
      expect(useChatStore.getState().queuedText).toBe("second");
      expect(useChatStore.getState().inputText).toBe("");
      expect(useChatStore.getState().isStreaming).toBe(true);

      // Turn A ends → the queued message auto-sends as turn B.
      emitA(evA.turnEnd);
      resolveSendA({ kind: "accepted", prompt_id: null, turn_id: "q1" });
      await sendPromiseA;
      await sendPromiseB;
      await new Promise((r) => setTimeout(r, 10));

      const emitB = capturedHandler.current!;
      const evB = turnEvents("q2");
      emitB(evB.turnStart);
      emitB(evB.text);
      emitB(evB.turnEnd);

      const texts = useChatStore
        .getState()
        .messages.filter((m) => m.role === "user")
        .map((m) => m.blocks.map((b) => (b.type === "text" ? b.content : "")).join(""));
      expect(texts).toEqual(["first", "second"]);
      expect(useChatStore.getState().queuedText).toBeNull();
    });

    it("restores the queued text into the input when the turn errors", async () => {
      useChatStore.setState({
        currentSessionId: "s1",
        selectedModel: MOCK_MODEL as never,
        inputText: "first",
      });
      let resolveSendA!: (v: SendChatResult) => void;
      vi.mocked(chatApi.sendMessage).mockImplementationOnce(
        () => new Promise<SendChatResult>((r) => { resolveSendA = r; }),
      );
      const sendCallsBefore = vi.mocked(chatApi.sendMessage).mock.calls.length;
      const sendPromiseA = useChatStore.getState().sendMessage();
      // Wait until sendMessage registers the stream listener and starts the
      // backend call (see the queue-and-send test above for the timing note).
      await vi.waitFor(() => {
        expect(vi.mocked(chatApi.sendMessage).mock.calls.length).toBe(sendCallsBefore + 1);
      });
      const emitA = capturedHandler.current!;
      const evA = turnEvents("q3");
      emitA(evA.turnStart);
      emitA(evA.text);

      useChatStore.setState({ inputText: "second" });
      const sendPromiseB = useChatStore.getState().sendMessage("queue");
      expect(useChatStore.getState().queuedText).toBe("second");

      emitA(evt({ type: "error", turn_id: "q3", session_id: "s1", message: "boom" }));
      resolveSendA({ kind: "accepted", prompt_id: null, turn_id: "q3" });
      await sendPromiseA;
      await sendPromiseB;

      // The queued message must not be lost — it goes back to the input.
      expect(useChatStore.getState().queuedText).toBeNull();
      expect(useChatStore.getState().inputText).toBe("second");
    });
  });

  describe("trimStreamOverlap", () => {
    it("trims a repeated tail from the incoming delta", () => {
      expect(trimStreamOverlap("hello world, and beyond", "hello world, and beyond, more")).toBe(", more");
    });

    it("keeps the delta when there is no meaningful overlap", () => {
      expect(trimStreamOverlap("hello world", "completely new")).toBe("completely new");
    });

    it("keeps short deltas untouched (no false trims)", () => {
      expect(trimStreamOverlap("hello world", "hi")).toBe("hi");
    });

    it("trims the full delta when it repeats entirely", () => {
      expect(trimStreamOverlap("hello world", "hello world!")).toBe("!");
    });

    it("does not trim sub-10-char overlaps (anti-false-trim floor)", () => {
      expect(trimStreamOverlap("this world", "world and beyond")).toBe("world and beyond");
    });
  });

  describe("summarizeSubagentResult", () => {
    it("strips tool-call protocol markup from the result", () => {
      const result = "文件分析完成。 <tool_calls> <bash>grep -o</tool_calls> 结尾";
      expect(summarizeSubagentResult(result)).toBe("文件分析完成。 结尾");
    });

    it("strips orphan openers and closers (truncated block leaves are the backend's job)", () => {
      // The orphan `<tool_calls>` opener is removed; a cut-off block's
      // `<bash>` leaf would already be gone in backend-sanitized input.
      expect(summarizeSubagentResult("need more <tool_calls> <bash>grep")).toBe("need more <bash>grep");
      expect(summarizeSubagentResult("done </tool_calls>")).toBe("done");
    });

    it("caps at maxLen and keeps plain text", () => {
      expect(summarizeSubagentResult("plain result")).toBe("plain result");
      const long = "a".repeat(200);
      const summary = summarizeSubagentResult(long);
      expect(summary.length).toBeLessThanOrEqual(121);
      expect(summary.endsWith("…")).toBe(true);
    });

    it("strips harness frames (system-reminder etc.) from visible summaries", () => {
      // Session 2d02f3dc: a skill-injection reminder was echoed into the
      // visible stream. Display-side stripping is the last line of defense.
      const leaked =
        "这个不需要你区弄 <system-reminder> Active project skills apply to this work: " +
        "## Skill: Code Review ... </system-reminder> 回到你的问题。";
      expect(summarizeSubagentResult(leaked)).toBe("这个不需要你区弄 回到你的问题。");
    });
  });

  describe("collectChanges", () => {
    const tool = (name: string, args: string) =>
      ({
        type: "tool_call",
        tool: { id: "t1", name, arguments: args, status: "done" },
      }) as MessageBlock;

    it("aggregates edit_file into per-file changes", () => {
      const blocks: MessageBlock[] = [
        tool(
          "edit_file",
          JSON.stringify({ path: "src/a.ts", old_text: "old", new_text: "new" }),
        ),
        tool(
          "search_replace",
          JSON.stringify({ path: "src/b.ts", old_text: "x", new_text: "y" }),
        ),
      ];
      const changes = collectChanges(blocks);
      expect(changes).toHaveLength(2);
      expect(changes[0]).toEqual({ path: "src/a.ts", oldText: "old", newText: "new" });
      expect(changes[1].path).toBe("src/b.ts");
    });

    it("treats write_file as a full-file addition", () => {
      const changes = collectChanges([
        tool("write_file", JSON.stringify({ path: "src/new.rs", content: "fn main() {}" })),
      ]);
      expect(changes).toHaveLength(1);
      expect(changes[0]).toEqual({ path: "src/new.rs", oldText: "", newText: "fn main() {}" });
    });

    it("collapses same-file edits to the LAST one", () => {
      const changes = collectChanges([
        tool(
          "edit_file",
          JSON.stringify({ path: "src/a.ts", old_text: "v1", new_text: "v2" }),
        ),
        tool(
          "edit_file",
          JSON.stringify({ path: "src/a.ts", old_text: "v2", new_text: "v3" }),
        ),
      ]);
      expect(changes).toHaveLength(1);
      expect(changes[0].newText).toBe("v3");
    });

    it("drops no-op and non-edit tools", () => {
      const changes = collectChanges([
        tool("edit_file", JSON.stringify({ path: "src/a.ts", old_text: "same", new_text: "same" })),
        tool("bash", JSON.stringify({ command: "ls" })),
        tool("grep", JSON.stringify({ pattern: "x" })),
      ]);
      expect(changes).toHaveLength(0);
    });

    it("skips malformed arguments gracefully", () => {
      const changes = collectChanges([
        tool("edit_file", "not-json{{{"),
        tool("write_file", JSON.stringify({})),
      ]);
      expect(changes).toHaveLength(0);
    });
  });

  describe("inferStreamPhase", () => {
    it("maps every phase-changing event type", () => {
      expect(inferStreamPhase(evt({ type: "turn_start", turn_id: "t", session_id: "s", model: "m" }))).toBe("connecting");
      expect(inferStreamPhase(evt({ type: "reasoning_delta", turn_id: "t", text: "x" }))).toBe("thinking");
      expect(inferStreamPhase(evt({ type: "text_delta", turn_id: "t", text: "x" }))).toBe("generating");
      expect(inferStreamPhase(evt({ type: "tool_call_start", turn_id: "t", call_id: "c", name: "n" }))).toBe("tool_running");
      expect(inferStreamPhase(evt({ type: "tool_call_progress", turn_id: "t", call_id: "c", name: "n", kind: "partial_result", delta: "x" }))).toBe("tool_running");
      expect(inferStreamPhase(evt({ type: "tool_call_result", turn_id: "t", call_id: "c", name: "n", result: "r", is_error: false }))).toBe("tool_running");
      expect(inferStreamPhase(evt({ type: "mcp_app", turn_id: "t", call_id: "c", name: "n", server: "s", resource_uri: "ui://x", html: "<h1/>", is_error: false }))).toBe("tool_running");
      expect(inferStreamPhase(evt({ type: "turn_end", turn_id: "t", session_id: "s", reason: "stop" }))).toBe("idle");
      expect(inferStreamPhase(evt({ type: "error", turn_id: "t", session_id: "s", message: "e" }))).toBe("idle");
      expect(inferStreamPhase(evt({ type: "usage", turn_id: "t", usage: { prompt_tokens: 1, completion_tokens: 1 } }))).toBeNull();
      expect(inferStreamPhase(evt({ type: "turn_status", turn_id: "t", session_id: "s", phase: "verifying", reason: "gate" }))).toBe("verifying");
      expect(inferStreamPhase(evt({ type: "snapshot", snapshot: { turn_id: "t", session_id: "s", status: "done", reason: "stop", text: "", reasoning: "", tool_calls: [], mcp_apps: [] } }))).toBeNull();
    });
  });

  it("creates sessions with the backend provider id, not the display name", async () => {
    const createSession = vi.mocked(sessionApi.createSession);
    createSession.mockClear();
    useChatStore.setState({
      currentSessionId: null,
      selectedModel: {
        ...MOCK_MODEL,
        provider: "Moonshot",
        providerId: "provider-1",
      } as never,
      inputText: "hello",
    });
    const sendPromise = useChatStore.getState().sendMessage();
    await sendPromise;
    expect(createSession).toHaveBeenCalledWith(
      "deepseek-chat",
      "provider-1",
      undefined,
      "code",
      64000,
      expect.any(String),
    );
  });

  it("rebuilds the picker model list when settings providers change", () => {
    useSettingsStore.setState({
      providers: [
        {
          id: "relay",
          name: "Relay",
          baseUrl: "https://relay.example.com/v1",
          apiKey: "sk-test",
          apiFormat: "openai",
          models: [{ id: "kimi-k3", name: "Kimi K3", contextWindow: 256_000 }],
          enabled: true,
        },
      ],
    });

    const state = useChatStore.getState();
    expect(state.models).toHaveLength(1);
    expect(state.models[0].providerId).toBe("relay");
    // Fresh context window from the settings edit reaches the input bar.
    expect(state.models[0].context_window).toBe(256_000);
  });
});
