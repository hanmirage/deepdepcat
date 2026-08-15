/**
 * highlightClient — main-thread API for the Shiki worker.
 *
 * Lazily boots the worker on first use, tracks per-key generations so
 * stale results are dropped (supersede), and reports failures so callers
 * can fall back to the lightweight tokenizer. In environments without
 * Worker support (jsdom tests) `highlightWorkerAvailable()` is false and
 * callers never invoke this module.
 */

import type { HighlightPayload } from "@/lib/codeTokens";

interface WorkerResult {
  type: "result";
  key: string;
  generation: number;
  text: string;
  language: string;
  tokens?: HighlightPayload["tokens"];
  error?: boolean;
}

type ResolveFn = (payload: HighlightPayload | null) => void;

let worker: Worker | null = null;
let workerBroken = false;
/** Per-key latest generation — results with older gens are stale. */
const generations = new Map<string, number>();
const pending = new Map<string, ResolveFn>();

function boot(): Worker | null {
  if (worker) return worker;
  if (workerBroken) return null;
  if (typeof Worker === "undefined") return null;
  try {
    worker = new Worker(new URL("./highlight.worker.ts", import.meta.url), {
      type: "module",
    });
    worker.onmessage = (e: MessageEvent<WorkerResult>) => {
      const msg = e.data;
      if (!msg || msg.type !== "result") return;
      const latest = generations.get(msg.key);
      if (latest === undefined || msg.generation !== latest) return; // stale
      const resolve = pending.get(msg.key);
      if (resolve) {
        pending.delete(msg.key);
        resolve({
          tokens: msg.tokens ?? [],
          text: msg.text,
          language: msg.language,
          error: msg.error,
        });
      }
    };
    worker.onerror = () => {
      workerBroken = true;
      worker = null;
      // Fail all pending requests so callers fall back.
      for (const resolve of pending.values()) {
        resolve({ tokens: [], text: "", language: "", error: true });
      }
      pending.clear();
      generations.clear();
    };
  } catch {
    workerBroken = true;
    worker = null;
  }
  return worker;
}

/**
 * Highlight a growing code string. Resolves with the token payload, or:
 *  - `null` when the request was superseded by a newer one (ignore)
 *  - `{ error: true }` when the worker is unavailable/broken (caller
 *    falls back to the lightweight tokenizer)
 */
export function highlightStreaming(
  key: string,
  text: string,
  language: string,
  theme: "light" | "dark",
): Promise<HighlightPayload | null> {
  const w = boot();
  if (!w) {
    return Promise.resolve({ tokens: [], text, language, error: true });
  }
  const generation = (generations.get(key) ?? 0) + 1;
  generations.set(key, generation);
  return new Promise((resolve) => {
    // Supersede any in-flight request for this key — resolve it as stale
    // so its caller (an older effect) can drop it.
    const prev = pending.get(key);
    if (prev) prev(null);
    pending.set(key, resolve);
    w.postMessage({ type: "highlight", key, generation, text, language, theme });
  });
}

/** Release a block's worker-side resources (call on unmount / fence close). */
export function disposeHighlight(key: string): void {
  generations.delete(key);
  const prev = pending.get(key);
  if (prev) {
    pending.delete(key);
    prev(null);
  }
  if (worker) {
    worker.postMessage({ type: "dispose", key });
  }
}

/** Whether the Shiki worker is usable in this environment. */
export function highlightWorkerAvailable(): boolean {
  return !workerBroken && typeof Worker !== "undefined";
}
