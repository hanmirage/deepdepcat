/**
 * Session tracking — remembers the last active session and restores it after
 * a crash.
 *
 * Two pieces:
 * - `startSessionTracking()` — subscribes to both chat stores and persists
 *   `{ mode, sessionId }` to localStorage whenever the active session changes.
 *   Any activation path (session selector, ensureSession, restore) is captured
 *   automatically without touching the stores themselves. The stores are
 *   imported lazily (see the function) to keep the module graph acyclic.
 * - `prepareCrashRecovery()` — called by the crash dialog when a pending crash
 *   exists at startup. Restores the remembered session into the correct mode's
 *   store, or silently no-ops when there's nothing to restore.
 */

/** localStorage key for the last active session ref. */
export const PREF_LAST_SESSION = "deepdepcat.lastSessionId";
/** A remembered session: which mode it belongs to + its id. */
export interface LastSessionRef {
  mode: "code" | "depwork";
  sessionId: string;
}

function loadLastSession(): LastSessionRef | null {
  try {
    const raw = localStorage.getItem(PREF_LAST_SESSION);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as LastSessionRef;
    return parsed && typeof parsed.sessionId === "string" ? parsed : null;
  } catch {
    return null;
  }
}

function saveLastSession(ref: LastSessionRef | null): void {
  try {
    if (ref === null) {
      localStorage.removeItem(PREF_LAST_SESSION);
    } else {
      localStorage.setItem(PREF_LAST_SESSION, JSON.stringify(ref));
    }
  } catch {
    /* storage may be unavailable */
  }
}

let trackingStarted = false;
/** Unsubscribe handles — retained so tests can reset between cases. */
let unsubscribe: Array<() => void> = [];

/**
 * Register the two session-trackers (idempotent). Safe to call from both
 * the crash dialog (belt) and appStore.initSystem (suspenders).
 *
 * The stores are imported LAZILY: sessionTracker is reached from appStore,
 * which sits at the top of the module graph and is imported by the chat
 * stores themselves. A static edge here would recreate the circular import
 * (appStore → sessionTracker → chatStore → appStore) and TDZ-break module
 * initialization — depworkChatStore's factory instance is created before
 * chatStore's module body finishes executing.
 */
export async function startSessionTracking(): Promise<void> {
  if (trackingStarted) return;
  trackingStarted = true;

  const [chat, depwork] = await Promise.all([
    import("@/stores/chatStore"),
    import("@/stores/depworkChatStore"),
  ]);
  unsubscribe.push(
    chat.useChatStore.subscribe((state, prevState) => {
      const cur = state.currentSessionId;
      if (cur && cur !== prevState.currentSessionId) {
        saveLastSession({ mode: "code", sessionId: cur });
      }
    }),
    depwork.useDepworkChatStore.subscribe((state, prevState) => {
      const cur = state.currentSessionId;
      if (cur && cur !== prevState.currentSessionId) {
        saveLastSession({ mode: "depwork", sessionId: cur });
      }
    }),
  );
}

/** Test hook — reset subscription state between test cases. Not exported in
 *  production code paths (only reachable via direct module import). */
export function _resetSessionTrackingForTest(): void {
  for (const unsub of unsubscribe) unsub();
  unsubscribe = [];
  trackingStarted = false;
}

/** Restore the remembered session after a crash. Returns true when a session
 *  was restored. Never throws — a stale/deleted session degrades silently.
 *
 *  Naturally idempotent for a given crash: the key is cleared before the
 *  restore, so a second invocation (e.g. React StrictMode double-mount) reads
 *  nothing and returns false. */
export async function prepareCrashRecovery(
  selectById: (id: string) => Promise<void>,
): Promise<boolean> {
  const last = loadLastSession();
  if (!last) return false;

  // Clear first so a deleted session isn't retried on every crash. A
  // successful restore re-saves the session via the tracker subscription.
  saveLastSession(null);
  try {
    await selectById(last.sessionId);
  } catch {
    return false;
  }
  return true;
}
