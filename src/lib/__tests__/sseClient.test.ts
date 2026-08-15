import { describe, it, expect, vi, beforeEach } from "vitest";

const invokeMock = vi.fn();
const onEventMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@/lib/tauri/core", () => ({
  isTauri: true,
  onEvent: (...args: unknown[]) => onEventMock(...args),
}));

class FakeEventSource {
  static instances: FakeEventSource[] = [];
  url: string;
  listeners: Record<string, ((e: { data: string }) => void)[]> = {};
  onerror: (() => void) | null = null;
  onopen: (() => void) | null = null;
  closed = false;

  constructor(url: string) {
    this.url = url;
    FakeEventSource.instances.push(this);
  }

  addEventListener(type: string, fn: (e: { data: string }) => void) {
    (this.listeners[type] ??= []).push(fn);
  }

  emit(type: string, data: string) {
    for (const fn of this.listeners[type] ?? []) fn({ data });
  }

  close() {
    this.closed = true;
  }
}

describe("connectChatStream (real SSE)", () => {
  beforeEach(() => {
    FakeEventSource.instances = [];
    invokeMock.mockReset();
    onEventMock.mockReset();
    vi.stubGlobal("EventSource", FakeEventSource);
  });

  async function load() {
    vi.resetModules();
    return (await import("@/lib/tauri/sse")).connectChatStream;
  }

  it("connects to the backend SSE endpoint and forwards events", async () => {
    invokeMock.mockResolvedValue(12345);
    const connectChatStream = await load();
    const handler = vi.fn();
    const unlisten = await connectChatStream(handler);

    expect(FakeEventSource.instances).toHaveLength(1);
    expect(FakeEventSource.instances[0].url).toBe(
      "http://127.0.0.1:12345/sse/chat-stream",
    );

    FakeEventSource.instances[0].emit(
      "chat-stream",
      JSON.stringify({ type: "text_delta", text: "hi" }),
    );
    expect(handler).toHaveBeenCalledWith({ type: "text_delta", text: "hi" });

    unlisten();
    expect(FakeEventSource.instances[0].closed).toBe(true);
    expect(onEventMock).not.toHaveBeenCalled();
  });

  it("falls back to the Tauri event bus when the port is unavailable", async () => {
    invokeMock.mockRejectedValue(new Error("not ready"));
    onEventMock.mockResolvedValue(() => {});
    const connectChatStream = await load();
    await connectChatStream(vi.fn());
    expect(onEventMock).toHaveBeenCalledWith("chat-stream", expect.any(Function));
  });

  it("falls back when the SSE stream never delivers", async () => {
    invokeMock.mockResolvedValue(9999);
    onEventMock.mockResolvedValue(() => {});
    const connectChatStream = await load();
    await connectChatStream(vi.fn());
    FakeEventSource.instances[0].onerror?.();
    // Allow the promise chain to attach the fallback listener.
    await new Promise((r) => setTimeout(r, 0));
    expect(onEventMock).toHaveBeenCalledWith("chat-stream", expect.any(Function));
  });

  it("fires onReconnect only after a drop from an already-delivering connection", async () => {
    invokeMock.mockResolvedValue(7777);
    const connectChatStream = await load();
    const reconnect = vi.fn();
    await connectChatStream(vi.fn(), { onReconnect: reconnect });

    // Initial open: never delivered yet — no reconnect signal.
    FakeEventSource.instances[0].onopen?.();
    expect(reconnect).not.toHaveBeenCalled();

    // Deliver one event, then drop: the auto-reconnect fires the callback.
    FakeEventSource.instances[0].emit(
      "chat-stream",
      JSON.stringify({ type: "text_delta", text: "hi" }),
    );
    expect(reconnect).not.toHaveBeenCalled();
    FakeEventSource.instances[0].onerror?.();
    FakeEventSource.instances[0].onopen?.();
    expect(reconnect).toHaveBeenCalledTimes(1);
  });
});
