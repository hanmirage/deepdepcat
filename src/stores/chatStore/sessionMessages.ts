/**
 * Per-session message buffer helpers.
 *
 * `messages` moved from the single global store array to a per-session buffer
 * (StreamState.messages) — the store's `messages` is a projection of the
 * CURRENT session's array. A background session keeps streaming into its own
 * buffer; when the user switches back, `setSessionId` projects that buffer so
 * a mid-turn reply survives the switch (the backend persists a turn only
 * after it ends — mid-turn messages exist nowhere else).
 *
 * Rule: the buffer write is always performed; the store sync (`messages`)
 * happens only when `sessionId` is the session being viewed.
 */

import type { UIMessage } from "@/types";
import type { ChatState } from "./types";
import type { StreamState } from "./streamState";

type SetFn = (partial: Partial<ChatState>) => void;

/** Fold `updater` into a session's authoritative buffer and sync the store's
 *  `messages` projection when that session is the one being viewed. `extra`
 *  fields ride the same `set` (e.g. totalTokens), so one update is one write. */
export function updateSessionMessages(
  st: StreamState,
  sessionId: string,
  updater: (messages: UIMessage[]) => UIMessage[],
  get: () => ChatState,
  set: SetFn,
  extra?: Partial<ChatState>,
): UIMessage[] {
  const next = updater(st.messages);
  st.messages = next;
  if (get().currentSessionId === sessionId) {
    set({ messages: next, ...extra });
  }
  return next;
}

/** Replace a session's buffer outright (same sync rule). */
export function setSessionMessages(
  st: StreamState,
  sessionId: string,
  next: UIMessage[],
  get: () => ChatState,
  set: SetFn,
): void {
  st.messages = next;
  if (get().currentSessionId === sessionId) {
    set({ messages: next });
  }
}
