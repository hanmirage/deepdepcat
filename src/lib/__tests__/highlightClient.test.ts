/**
 * highlightClient tests — worker boot, supersede, failure fallback.
 */

import { describe, it, expect, vi, afterEach } from "vitest";

class FakeWorker {
  onmessage: ((e: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  sent: { type: string; key: string; generation: number }[] = [];

  postMessage(msg: { type: string; key: string; generation: number }) {
    this.sent.push(msg);
  }

  /** Test driver: simulate a worker reply. */
  respond(msg: { type: "result"; key: string; generation: number; tokens: unknown[]; text: string; language: string; error?: boolean }) {
    this.onmessage?.({ data: msg });
  }

  fail() {
    this.onerror?.();
  }
}

let lastWorker: FakeWorker | null = null;

async function loadModule() {
  vi.resetModules();
  vi.stubGlobal("Worker", class extends FakeWorker {
    constructor(_url: string | URL, _opts?: unknown) {
      super();
      // eslint-disable-next-line @typescript-eslint/no-this-alias -- test harness records the worker instance
      lastWorker = this;
    }
  });
  return await import("@/lib/highlightClient");
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.resetModules();
  lastWorker = null;
});

describe("highlightClient", () => {
  it("reports error when Worker is unavailable (jsdom)", async () => {
    vi.stubGlobal("Worker", undefined);
    const mod = await import("@/lib/highlightClient");
    expect(mod.highlightWorkerAvailable()).toBe(false);
    const payload = await mod.highlightStreaming("k", "const x", "ts", "light");
    expect(payload?.error).toBe(true);
  });

  it("highlights through the worker and resolves the payload", async () => {
    const mod = await loadModule();
    expect(mod.highlightWorkerAvailable()).toBe(true);
    const promise = mod.highlightStreaming("k", "const x", "ts", "light");
    expect(lastWorker?.sent).toHaveLength(1);
    expect(lastWorker?.sent[0]).toMatchObject({ type: "highlight", key: "k", generation: 1 });
    lastWorker!.respond({
      type: "result",
      key: "k",
      generation: 1,
      text: "const x",
      language: "ts",
      tokens: [{ text: "const", color: "#569cd6" }],
    });
    const payload = await promise;
    expect(payload?.tokens).toEqual([{ text: "const", color: "#569cd6" }]);
    expect(payload?.error).toBeUndefined();
  });

  it("supersedes: a stale result is dropped, only the newest lands", async () => {
    const mod = await loadModule();
    const p1 = mod.highlightStreaming("k", "a", "ts", "light");
    const p2 = mod.highlightStreaming("k", "ab", "ts", "light");
    expect(lastWorker?.sent.map((s) => s.generation)).toEqual([1, 2]);
    // Stale reply (gen 1) arrives late — resolves null, never applies.
    lastWorker!.respond({
      type: "result",
      key: "k",
      generation: 1,
      text: "a",
      language: "ts",
      tokens: [],
    });
    expect(await p1).toBeNull();
    // Newest reply lands.
    lastWorker!.respond({
      type: "result",
      key: "k",
      generation: 2,
      text: "ab",
      language: "ts",
      tokens: [{ text: "ab", color: "#000" }],
    });
    expect(await p2).toMatchObject({ tokens: [{ text: "ab", color: "#000" }] });
  });

  it("dispose releases pending work (resolves stale)", async () => {
    const mod = await loadModule();
    const p = mod.highlightStreaming("k", "a", "ts", "light");
    mod.disposeHighlight("k");
    expect(await p).toBeNull();
    expect(lastWorker?.sent.some((s) => s.type === "dispose" && s.key === "k")).toBe(true);
  });

  it("worker crash fails all pending requests with error (caller falls back)", async () => {
    const mod = await loadModule();
    const p = mod.highlightStreaming("k", "a", "ts", "light");
    lastWorker!.fail();
    const payload = await p;
    expect(payload?.error).toBe(true);
    // Subsequent requests report error too (worker marked broken).
    const next = await mod.highlightStreaming("k2", "b", "ts", "light");
    expect(next?.error).toBe(true);
  });
});
