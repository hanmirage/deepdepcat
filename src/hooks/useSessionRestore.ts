/**
 * useSessionRestore — loads a session's message history from the backend
 * and injects it into the correct mode's chat store.
 *
 * Components call this hook; they never touch sessionApi directly.
 *
 * Returns a `selectSession` callback that:
 * 1. Sets the session ID in the mode's store (code/depwork per `work_mode`)
 * 2. Switches app mode to the session's mode
 * 3. Flips the session-loading flag (skeleton UI)
 * 4. Fetches the session's conversation history from the backend
 * 5. Converts ConversationItem[] → UIMessage[]/DepworkMessage[] and loads
 *    them into the matching store
 */

import { useCallback, useRef } from "react";
import { sessionApi } from "@/lib/tauri";
import { conversationItemsToUIMessages } from "@/lib/conversationConvert";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useAppStore } from "@/stores/appStore";
import type { Session } from "@/types";

export function useSessionRestore() {
  const setSessionId = useChatStore((s) => s.setSessionId);
  const setSessionTitle = useChatStore((s) => s.setSessionTitle);
  const setMessages = useChatStore((s) => s.setMessages);
  const setSessionLoading = useChatStore((s) => s.setSessionLoading);
  const depworkSetSessionId = useDepworkChatStore((s) => s.setSessionId);
  const depworkSetSessionTitle = useDepworkChatStore((s) => s.setSessionTitle);
  const depworkSetMessages = useDepworkChatStore((s) => s.setMessages);
  const depworkSetSessionLoading = useDepworkChatStore((s) => s.setSessionLoading);
  const setMode = useAppStore((s) => s.setMode);

  // Generation guard — clicking session B while session A's history is still
  // loading must not let A's late response overwrite B.
  const restoreGen = useRef(0);
  // Per-mode loading-flag ownership: which selection generation last flipped
  // each mode's loading flag. A superseded selection still clears its OWN
  // mode's flag (its late response must not leave the skeleton stuck), but
  // only if no newer selection re-flipped that mode.
  const loadingGen = useRef<{ code: number; depwork: number }>({ code: 0, depwork: 0 });
  // Per-mode "committed" session: the id/title whose messages are actually
  // loaded in that store. An eagerly-committed selection (id/title set before
  // history arrives) that gets superseded cross-mode must roll back to this,
  // not leave a foreign id paired with the previous session's stale messages.
  const committedRef = useRef<{
    code: { id: string | null; title: string | null };
    depwork: { id: string | null; title: string | null };
  }>({ code: { id: null, title: null }, depwork: { id: null, title: null } });

  const selectSession = useCallback(
    async (session: Session) => {
      const gen = ++restoreGen.current;
      const isDepwork = session.work_mode === "depwork";
      const modeKey = isDepwork ? "depwork" : "code";
      // This selection now owns its mode's loading flag.
      loadingGen.current[modeKey] = gen;

      // Sessions belong to a product mode — restore into that mode instead
      // of always falling back to Code (a depwork session must not reopen
      // in the code view).
      setMode(isDepwork ? "depwork" : "code");
      // Reflect the selection immediately (list highlight) — history fills in.
      if (isDepwork) {
        depworkSetSessionLoading(true);
        depworkSetSessionId(session.id);
        depworkSetSessionTitle(session.title);
      } else {
        setSessionLoading(true);
        setSessionId(session.id);
        setSessionTitle(session.title);
      }

      try {
        const history = await sessionApi.getSessionMessages(session.id);
        if (gen !== restoreGen.current) return; // a newer selection superseded this one
        const uiMessages = conversationItemsToUIMessages(history);
        if (isDepwork) {
          depworkSetMessages(uiMessages);
        } else {
          setMessages(uiMessages);
        }
        // Only a successful load makes the id/title authoritative — record it
        // as the committed session so a superseded eager commit can roll back.
        committedRef.current[modeKey] = { id: session.id, title: session.title };
      } catch {
        if (gen !== restoreGen.current) return;
        if (isDepwork) {
          depworkSetMessages([]);
        } else {
          setMessages([]);
        }
      } finally {
        // Clear THIS mode's loading flag unless a newer selection re-flipped
        // it (e.g. a same-mode supersede). The global gen guard was the bug:
        // a cross-mode switch left the abandoned mode stuck loading forever.
        if (loadingGen.current[modeKey] === gen) {
          if (gen !== restoreGen.current) {
            // Cross-mode superseded before history arrived: the eagerly set
            // id/title point at a session whose messages never loaded. Roll
            // back to the last committed session (whose messages are what the
            // store still holds) instead of showing a foreign id + stale body.
            const prev = committedRef.current[modeKey];
            if (isDepwork) {
              depworkSetSessionId(prev.id);
              depworkSetSessionTitle(prev.title ?? "");
            } else {
              setSessionId(prev.id);
              setSessionTitle(prev.title ?? "");
            }
          }
          if (isDepwork) depworkSetSessionLoading(false);
          else setSessionLoading(false);
        }
      }
    },
    [
      setSessionId,
      setSessionTitle,
      setMessages,
      setSessionLoading,
      depworkSetSessionId,
      depworkSetSessionTitle,
      depworkSetMessages,
      depworkSetSessionLoading,
      setMode,
    ],
  );

  // Select a session by ID (used by cross-session notifications — the
  // bell only knows the session id, not the full Session object).
  const selectSessionById = useCallback(
    async (sessionId: string) => {
      try {
        const session = await sessionApi.getSession(sessionId);
        await selectSession(session);
      } catch {
        // Session may have been deleted meanwhile — silently ignore.
      }
    },
    [selectSession],
  );

  return { selectSession, selectSessionById };
}
