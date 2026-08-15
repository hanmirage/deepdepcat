/**
 * Running-sessions store — background main-agent turns.
 *
 * A turn keeps executing after the user switches away (the loop lives in
 * the backend `send_chat_message` invocation). This store mirrors the
 * backend registry: in-flight turns for the activity panel, plus a bounded
 * completion history for notifications.
 */

import { create } from "zustand";
import {
  runningSessionsApi,
  type RunningTurnInfo,
  type TurnCompletedPayload,
} from "@/lib/tauri";

export interface CompletedTurn {
  session_id: string;
  turn_id: string;
  status: string;
  completed_at_ms: number;
}

const MAX_COMPLETED = 20;

interface RunningSessionsState {
  running: RunningTurnInfo[];
  completed: CompletedTurn[];
  /** Pull the backend registry (polled + event-triggered). */
  refresh: () => Promise<void>;
  setRunning: (running: RunningTurnInfo[]) => void;
  pushCompleted: (payload: TurnCompletedPayload) => void;
  clear: () => void;
}

export const useRunningSessionsStore = create<RunningSessionsState>((set) => ({
  running: [],
  completed: [],

  refresh: async () => {
    try {
      const list = await runningSessionsApi.list();
      set({ running: list });
    } catch {
      // Backend unavailable — the event listeners will populate later.
    }
  },

  setRunning: (running) => set({ running }),

  pushCompleted: (payload) =>
    set((s) => ({
      completed: [
        {
          session_id: payload.session_id,
          turn_id: payload.turn_id,
          status: payload.status,
          completed_at_ms: Date.now(),
        },
        ...s.completed,
      ].slice(0, MAX_COMPLETED),
      running: s.running.filter((r) => r.session_id !== payload.session_id),
    })),

  clear: () => set({ running: [], completed: [] }),
}));
