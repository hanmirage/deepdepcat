/**
 * useSubagentPanel — auto-show/clear the right-panel subagent pane.
 *
 * Event-driven from the active mode's `subagents` map (not a poll): the pane
 * appears on the first running subagent of a dispatch and auto-clears a few
 * seconds after the dispatch fully ends (all records terminal AND the main
 * agent idle). A user dismiss suppresses auto-show for the run, matching
 * notifyActivity's contract.
 */

import { useEffect, useRef } from "react";
import type { AppMode } from "@/config/constants";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import type { SubagentUIRecord } from "@/types";

/** How long the subagents pane lingers after the dispatch fully ends. */
const SUBAGENTS_IDLE_CLEAR_MS = 5000;

/** True when at least one record is still running (a live dispatch). */
function hasRunning(subagents: Record<string, SubagentUIRecord>): boolean {
  return Object.values(subagents).some((s) => s.status === "running");
}

export function useSubagentPanel(mode: AppMode) {
  const codeSubagents = useChatStore((s) => s.subagents);
  const depworkSubagents = useDepworkChatStore((s) => s.subagents);
  const codeStreaming = useChatStore((s) => s.isStreaming);
  const depworkStreaming = useDepworkChatStore((s) => s.isStreaming);
  const notifySubagents = useRightPanelStore((s) => s.notifySubagents);
  const clearSubagents = useRightPanelStore((s) => s.clearSubagents);

  const subagents = mode === "depwork" ? depworkSubagents : codeSubagents;
  const running = hasRunning(subagents);
  const anyRecord = Object.keys(subagents).length > 0;
  const streaming = mode === "depwork" ? depworkStreaming : codeStreaming;

  const prevRunning = useRef(running);
  const prevStreaming = useRef(streaming);
  const clearTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    // A dispatch begins when a running subagent appears while none was
    // running before — covers the first fan-out AND a new wave after all
    // records went terminal (the map accumulates across waves).
    const dispatchStarted = running && !prevRunning.current;
    // A dispatch ends when the last running record went terminal AND THIS
    // mode's stream sealed — per-mode signals only, never the global agent
    // status (a background mode's turn ending would otherwise clear the
    // subagent pane the user is watching).
    const dispatchEnded =
      !running &&
      anyRecord &&
      !streaming &&
      (prevRunning.current || prevStreaming.current);
    prevRunning.current = running;
    prevStreaming.current = streaming;

    if (dispatchStarted) {
      // A new dispatch — cancel any pending clear and surface it.
      if (clearTimer.current) {
        clearTimeout(clearTimer.current);
        clearTimer.current = null;
      }
      notifySubagents(mode);
      return;
    }

    if (dispatchEnded && !clearTimer.current) {
      clearTimer.current = setTimeout(() => {
        clearTimer.current = null;
        clearSubagents(mode);
      }, SUBAGENTS_IDLE_CLEAR_MS);
    }
  }, [running, anyRecord, streaming, mode, notifySubagents, clearSubagents]);

  // Drop any pending clear timer on unmount.
  useEffect(
    () => () => {
      if (clearTimer.current) clearTimeout(clearTimer.current);
    },
    [],
  );
}
