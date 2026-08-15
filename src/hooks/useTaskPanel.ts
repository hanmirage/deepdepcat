/**
 * useTaskPanel — auto-show/clear the "任务" pane (Code todo plan / Depwork
 * tool steps).
 *
 * Code is driven by the active session's todo list: the pane appears when the
 * agent's task plan first appears (todos exist), auto-clears when the plan is
 * cleared or the run returns to idle. Depwork has no todos — its task signal is
 * a NEW tool step appearing (the agent executing document processing). Tracking
 * unseen tool ids (not a boolean "any tool exists") lets the pane reopen on
 * each run of the same session.
 *
 * A user dismiss suppresses auto-show for the run, matching notifyActivity's
 * contract.
 */

import { useEffect, useRef } from "react";
import type { AppMode } from "@/config/constants";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useTodoStore, selectSessionTodos } from "@/stores/todoStore";
import type { DepworkMessage } from "@/types/depwork";

/** How long the task pane lingers after the run returns to idle. */
const TASK_IDLE_CLEAR_MS = 5000;

/** Tool ids currently present across depwork assistant messages. */
function depworkToolIds(messages: DepworkMessage[]): Set<string> {
  const ids = new Set<string>();
  for (const m of messages) {
    if (m.role !== "assistant") continue;
    for (const b of m.blocks) {
      if (b.type === "tool_call") ids.add(b.tool.id);
    }
  }
  return ids;
}

export function useTaskPanel(mode: AppMode) {
  const codeSessionId = useChatStore((s) => s.currentSessionId);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);
  const codeStreaming = useChatStore((s) => s.isStreaming);
  const depworkStreaming = useDepworkChatStore((s) => s.isStreaming);
  const sessionId = mode === "depwork" ? depworkSessionId : codeSessionId;
  const todos = useTodoStore(selectSessionTodos(sessionId));
  const depworkMessages = useDepworkChatStore((s) => s.messages);
  const notifyTask = useRightPanelStore((s) => s.notifyTask);
  const clearTask = useRightPanelStore((s) => s.clearTask);

  const streaming = mode === "depwork" ? depworkStreaming : codeStreaming;
  const prevCount = useRef(0);
  const prevStreaming = useRef(streaming);
  const seenToolIds = useRef<Set<string>>(new Set());
  const clearTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    const isDepwork = mode === "depwork";

    let appeared = false;
    let cleared = false;
    if (isDepwork) {
      const ids = depworkToolIds(depworkMessages);
      // A tool id we've never seen → the agent started a (new) step. The
      // seen set grows across runs, so each run reopens the pane.
      appeared = [...ids].some((id) => !seenToolIds.current.has(id));
      if (appeared) ids.forEach((id) => seenToolIds.current.add(id));
      // Tools vanished entirely (session switch / clear) → drop immediately.
      cleared = ids.size === 0 && seenToolIds.current.size > 0;
      if (cleared) seenToolIds.current.clear();
    } else {
      appeared = todos.length > 0 && prevCount.current === 0;
      cleared = todos.length === 0 && prevCount.current > 0;
      prevCount.current = todos.length;
    }

    // Run went idle when THIS mode's stream sealed — per-mode signal, never
    // the global agent status (a background mode's turn ending would
    // otherwise clear the task pane the user is reviewing).
    const runWentIdle = !streaming && prevStreaming.current;
    prevStreaming.current = streaming;

    if (appeared) {
      if (clearTimer.current) {
        clearTimeout(clearTimer.current);
        clearTimer.current = null;
      }
      notifyTask(mode);
      return;
    }
    if (cleared) {
      // Plan cleared (session switch / clear) — drop the pane immediately.
      if (clearTimer.current) {
        clearTimeout(clearTimer.current);
        clearTimer.current = null;
      }
      clearTask(mode);
      return;
    }
    // Run went idle with the plan still present — let it linger briefly
    // for review, then clear.
    if (runWentIdle && !clearTimer.current) {
      clearTimer.current = setTimeout(() => {
        clearTimer.current = null;
        clearTask(mode);
      }, TASK_IDLE_CLEAR_MS);
    }
  }, [todos.length, depworkMessages, streaming, mode, notifyTask, clearTask]);

  // Drop any pending clear timer on unmount.
  useEffect(
    () => () => {
      if (clearTimer.current) clearTimeout(clearTimer.current);
    },
    [],
  );
}
