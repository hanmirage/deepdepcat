/**
 * useRightPanelActivity — event-driven right-panel signal.
 *
 * Watches the active mode's streaming state, todo list and agent status.
 * On a quiet→active transition it pings the right-panel store, which
 * auto-opens the activity pane once (unless the user dismissed the drawer
 * earlier in this run). On the reverse active→idle transition it schedules a
 * clear so the transient activity pane closes — dispatch info lingers only
 * while the agent is dispatching.
 */

import { useEffect, useRef } from "react";
import type { AppMode } from "@/config/constants";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useTodoStore, selectSessionTodos } from "@/stores/todoStore";

/** How long dispatch info stays after the agent returns to idle, before the
 *  activity pane auto-cleans back to the resident command console. */
const ACTIVITY_IDLE_CLEAR_MS = 5000;

export function useRightPanelActivity(mode: AppMode) {
  const codeStreaming = useChatStore((s) => s.isStreaming);
  const depworkStreaming = useDepworkChatStore((s) => s.isStreaming);
  const codeSessionId = useChatStore((s) => s.currentSessionId);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);
  const notifyActivity = useRightPanelStore((s) => s.notifyActivity);
  const clearActivity = useRightPanelStore((s) => s.clearActivity);

  const streaming = mode === "depwork" ? depworkStreaming : codeStreaming;
  const sessionId = mode === "depwork" ? depworkSessionId : codeSessionId;
  const todos = useTodoStore(selectSessionTodos(sessionId));

  const prevStreaming = useRef(streaming);
  const prevTodoCount = useRef(todos.length);
  const clearTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const streamStarted = streaming && !prevStreaming.current;
    const todosAppeared = todos.length > 0 && prevTodoCount.current === 0;
    // A dispatch ended only when THIS mode's stream sealed — per-mode signal,
    // never the global agent status (a background mode's turn ending would
    // otherwise clear the pane the user is watching).
    const dispatchEnded = !streaming && prevStreaming.current;
    prevStreaming.current = streaming;
    prevTodoCount.current = todos.length;

    if (streamStarted || todosAppeared) {
      // A new dispatch begins — cancel any pending clear and surface it.
      if (clearTimer.current) {
        clearTimeout(clearTimer.current);
        clearTimer.current = null;
      }
      notifyActivity(mode);
      return;
    }

    if (dispatchEnded && !clearTimer.current) {
      clearTimer.current = setTimeout(() => {
        clearTimer.current = null;
        clearActivity(mode);
      }, ACTIVITY_IDLE_CLEAR_MS);
    }
  }, [streaming, todos.length, mode, notifyActivity, clearActivity]);

  // Drop any pending clear timer on unmount.
  useEffect(
    () => () => {
      if (clearTimer.current) clearTimeout(clearTimer.current);
    },
    [],
  );
}
