/**
 * Real SSE transport for `chat-stream` events — SINGLE channel semantics.
 *
 * In Tauri, opens an EventSource against the backend's loopback SSE server
 * (`GET http://127.0.0.1:<port>/sse/chat-stream`) so the UI streams over
 * real HTTP Server-Sent Events.
 *
 * Channel rules:
 * - Until the stream has delivered ONE event, an `error` attaches the Tauri
 *   event bus as a fallback (stale port / server gone) — and detaches it
 *   the instant SSE starts delivering, so the two never double-deliver.
 * - After the stream has delivered an event, SSE is the ONLY channel: an
 *   `error` relies on EventSource's built-in reconnect; any missed deltas
 *   are repaired by the listener's seq-gap detection + turn snapshot.
 * - Browser dev mode keeps the mock event bus.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauri, onEvent } from "./core";
import type { ChatStreamEvent } from "./types";

let cachedPort: number | null = null;

/** Optional transport lifecycle callbacks. */
export interface ChatStreamOptions {
  /** Fired when the SSE connection is RE-established after a drop (the
   *  listener probes the turn snapshot — the missed window may have ended
   *  the turn, and only a snapshot pull can converge it). */
  onReconnect?: () => void;
}

/** Subscribe to agent stream events; returns an unsubscribe function. */
export async function connectChatStream(
  handler: (payload: ChatStreamEvent) => void,
  options?: ChatStreamOptions,
): Promise<() => void> {
  if (!isTauri) {
    // Browser dev mode — mock event bus (no backend process).
    return onEvent<ChatStreamEvent>("chat-stream", handler);
  }

  let port: number;
  try {
    if (cachedPort === null) cachedPort = await invoke<number>("get_sse_port");
    port = cachedPort;
  } catch {
    return onEvent<ChatStreamEvent>("chat-stream", handler);
  }

  const es = new EventSource(`http://127.0.0.1:${port}/sse/chat-stream`);
  let received = false;
  let fallbackUnlisten: (() => void) | null = null;
  let closed = false;

  es.addEventListener("chat-stream", (e) => {
    // SSE delivered — the event bus is never needed; drop it if it was
    // attached during the never-delivered window (prevents double-delivery).
    if (fallbackUnlisten) {
      fallbackUnlisten();
      fallbackUnlisten = null;
    }
    received = true;
    try {
      handler(JSON.parse((e as MessageEvent<string>).data) as ChatStreamEvent);
    } catch {
      // Malformed payload — ignore.
    }
  });

  // Fire ONLY on re-establishment after a drop (a connection that already
  // delivered events): the listener repairs any missed terminal state.
  es.onopen = () => {
    if (received && !closed) options?.onReconnect?.();
  };

  es.onerror = () => {
    if (closed) return;
    if (received) {
      // SSE already proved itself — keep it as the only channel. EventSource
      // auto-reconnects; the listener repairs any missed deltas via seq gaps.
      return;
    }
    // Never delivered — port may be stale (backend restarted). Attach the
    // Tauri event bus once; SSE detaches it as soon as it delivers.
    if (!fallbackUnlisten) {
      void onEvent<ChatStreamEvent>("chat-stream", handler).then((fn) => {
        if (closed || received) {
          fn();
          return;
        }
        fallbackUnlisten = fn;
      });
    }
  };

  return () => {
    closed = true;
    es.close();
    fallbackUnlisten?.();
  };
}
