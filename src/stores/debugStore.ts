/**
 * Debug state (Zustand).
 *
 * Manages:
 * - Debug event log (capped at 500 entries, FIFO)
 * - Active filters (by event type)
 * - Pause/resume capture
 *
 * This store is independent from chatStore/appStore to avoid
 * cross-contamination. The `debugMode` toggle lives in appStore
 * (it's app-level visibility state); event data lives here.
 */

import { create } from "zustand";
import type { DebugEvent, DebugEventType } from "@/types";

const MAX_EVENTS = 500;

interface DebugState {
  events: DebugEvent[];
  paused: boolean;
  activeFilters: Set<DebugEventType>;

  // ── Actions ────────────────────────────────────────────────
  addEvent: (event: DebugEvent) => void;
  clearEvents: () => void;
  togglePause: () => void;
  toggleFilter: (type: DebugEventType) => void;
  clearFilters: () => void;
}

export const useDebugStore = create<DebugState>((set, get) => ({
  events: [],
  paused: false,
  activeFilters: new Set(),

  addEvent: (event) => {
    if (get().paused) return;
    set((s) => ({
      events: s.events.length >= MAX_EVENTS
        ? [...s.events.slice(1), event]
        : [...s.events, event],
    }));
  },

  clearEvents: () => set({ events: [] }),

  togglePause: () => set((s) => ({ paused: !s.paused })),

  toggleFilter: (type) =>
    set((s) => {
      const next = new Set(s.activeFilters);
      if (next.has(type)) next.delete(type);
      else next.add(type);
      return { activeFilters: next };
    }),

  clearFilters: () => set({ activeFilters: new Set() }),
}));
