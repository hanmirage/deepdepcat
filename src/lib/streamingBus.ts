/**
 * streamingBus — cross-store notification of session streaming state.
 *
 * The chat stores keep per-session stream state in a private map that
 * React cannot observe, which is why the sidebar used to poll both stores
 * every 500ms. The stores now push every streaming flip into this tiny
 * external store; UI subscribes with `useStreamingSessions()` and renders
 * without any polling.
 */

import { useSyncExternalStore } from "react";

let snapshot: ReadonlySet<string> = new Set();
const listeners = new Set<() => void>();

function emit(): void {
  for (const fn of listeners) fn();
}

/** Mark a session's stream active/inactive (called by the chat stores). */
export function setSessionStreaming(sessionId: string, active: boolean): void {
  const next = new Set(snapshot);
  const changed = active ? next.add(sessionId) : next.delete(sessionId);
  if (changed) {
    snapshot = next;
    emit();
  }
}

/** Whether the given session currently has an active stream. */
export function isSessionStreaming(sessionId: string): boolean {
  return snapshot.has(sessionId);
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): ReadonlySet<string> {
  return snapshot;
}

/** React hook — re-renders when any session's streaming flag flips. */
export function useStreamingSessions(): ReadonlySet<string> {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
