/**
 * Chat store — split by concern (types / prefs / mode detection / stream state).
 */

import { setSessionStreaming } from "@/lib/streamingBus";
import type { StreamPhase } from "@/types";
import type { UIMessage } from "@/types";

/**
 * Per-session stream state — multi-session concurrency.
 *
 * The backend runs every session's agent loop independently (per-session
 * chat_state checkout, per-session cancellation/pause/prompt queues), so
 * the frontend must NOT share a single isStreaming/unlisten/generation slot
 * across sessions: session A streaming must never block session B's send,
 * stop, or event handling.
 */
export interface StreamState {
  /** Monotonic generation counter for this session's turns. Each sendMessage
   *  bumps it; stale turns (interrupted by stopStreaming + a new send) skip
   *  cleanup so they don't clobber the new turn's state. */
  gen: number;
  /** Double-send lock — prevents a rapid second send while the first is
   *  still awaiting ensureSession (isStreaming isn't set yet). */
  inFlight: boolean;
  /** True while a listener is kept alive waiting for a backend-queued prompt
   *  to be replayed (send_chat_message returned "queued:..."). */
  replayActive: boolean;
  /** Work mode pinned for an auto-send: a queued message must execute under
   *  the mode it was queued in, even if the user switched surfaces while the
   *  previous turn was still streaming. Consumed by the next sendMessage. */
  queuedWorkMode: string | null;
  /** unlisten for this session's active stream listener. */
  unlisten: (() => void) | null;
  /** Text queued for auto-send when THIS session's stream ends. */
  queuedText: string | null;
  /** True while this session's turn is paused (backend watch flipped). */
  paused: boolean;
  /** Live phase of this session's turn, inferred from chat-stream events. */
  phase: StreamPhase;
  /** Turn id of the LAST turn this session's listeners consumed. A turn torn
   *  down mid-flight (stop → resend while the backend still drains it) keeps
   *  emitting events with its OLD id; rejecting that id keeps stale deltas
   *  out of the new listener and — critically — stops the stale turn_end from
   *  killing a "queued:" replay listener. */
  lastTurnId: string | null;
  /** Auto-dismiss timer for THIS session's compaction notification. Lives on
   *  the session (not a module global) so code/depwork listeners — which share
   *  the same listener module — never clear each other's pending dismiss. */
  compactionTimer: ReturnType<typeof setTimeout> | null;
  /** Per-session authoritative message list. The store's `messages` is a
   *  projection of the CURRENT session's array; a background session keeps
   *  streaming into its own buffer so switching back restores it intact
   *  (backend persists a turn only after it ends — mid-turn messages exist
   *  nowhere else). */
  messages: UIMessage[];
}

export const streamStates = new Map<string, StreamState>();

export function streamState(sessionId: string): StreamState {
  let st = streamStates.get(sessionId);
  if (!st) {
    st = {
      gen: 0,
      inFlight: false,
      replayActive: false,
      queuedWorkMode: null,
      unlisten: null,
      queuedText: null,
      paused: false,
      phase: "idle",
      lastTurnId: null,
      compactionTimer: null,
      messages: [],
    };
    streamStates.set(sessionId, st);
  }
  return st;
}

/** Whether the given session currently has an active stream (in-flight send
 *  or a live listener). Independent per session — one session streaming does
 *  not block another. */
export function sessionStreaming(sessionId: string | null): boolean {
  if (!sessionId) return false;
  const st = streamStates.get(sessionId);
  return !!st && (st.inFlight || !!st.unlisten);
}

/** Push the session's live-stream flag to the cross-store streaming bus. */
export function syncStreamingBus(sessionId: string, st: StreamState): void {
  setSessionStreaming(sessionId, st.inFlight || st.unlisten !== null);
}
