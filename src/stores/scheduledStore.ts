/**
 * Scheduled tasks store (Zustand) — 定时任务 page data.
 *
 * Backed by `automationApi`; live updates arrive through
 * `scheduled-task-updated` / `scheduled-run-updated` events (the backend
 * runner and commands emit full rows).
 */

import { create } from "zustand";
import { automationApi, type CreateScheduledTaskInput } from "@/lib/tauri/api/automation";
import type { ScheduledRun, ScheduledTask, ScheduleSpec } from "@/types/scheduled";

interface ScheduledState {
  tasks: ScheduledTask[];
  runs: ScheduledRun[];
  /** Runs currently filtered to one task (null = all). */
  runsTaskId: string | null;
  loading: boolean;
  error: string | null;

  load: () => Promise<void>;
  loadRuns: (taskId?: string | null) => Promise<void>;
  create: (input: CreateScheduledTaskInput) => Promise<void>;
  updateTask: (
    id: string,
    patch: Parameters<typeof automationApi.updateTask>[1],
  ) => Promise<void>;
  remove: (id: string) => Promise<void>;
  runNow: (id: string) => Promise<string>;
  cancelRun: (runId: string) => Promise<void>;
  deleteRun: (runId: string) => Promise<void>;
  cleanupWorktree: (runId: string) => Promise<string>;
  /** Apply a full task row emitted by the backend. */
  upsertTask: (task: ScheduledTask) => void;
  /** Apply a full run row emitted by the backend. */
  upsertRun: (run: ScheduledRun) => void;
  clearError: () => void;
}

export const useScheduledStore = create<ScheduledState>((set, get) => ({
  tasks: [],
  runs: [],
  runsTaskId: null,
  loading: false,
  error: null,

  load: async () => {
    set({ loading: true, error: null });
    try {
      const tasks = await automationApi.listTasks();
      const runs = await automationApi.listRuns(get().runsTaskId ?? undefined, 50);
      set({ tasks, runs, loading: false });
    } catch (e) {
      set({ loading: false, error: String(e) });
    }
  },

  loadRuns: async (taskId) => {
    const next = taskId ?? null;
    set({ runsTaskId: next });
    try {
      const runs = await automationApi.listRuns(next ?? undefined, 50);
      // A newer filter may have landed while this request was in flight — a
      // late response for the previous task must not overwrite the current
      // task's runs (otherwise the list shows B's filter flag with A's rows).
      if (get().runsTaskId !== next) return;
      set({ runs });
    } catch (e) {
      set({ error: String(e) });
    }
  },

  create: async (input) => {
    const task = await automationApi.createTask(input);
    set((s) => ({ tasks: [task, ...s.tasks] }));
  },

  updateTask: async (id, patch) => {
    const task = await automationApi.updateTask(id, patch);
    set((s) => ({ tasks: s.tasks.map((t) => (t.id === id ? task : t)) }));
  },

  remove: async (id) => {
    await automationApi.deleteTask(id);
    set((s) => ({
      tasks: s.tasks.filter((t) => t.id !== id),
      runs: s.runs.filter((r) => r.task_id !== id),
      runsTaskId: s.runsTaskId === id ? null : s.runsTaskId,
    }));
  },

  runNow: (id) => automationApi.runNow(id),

  cancelRun: async (runId) => {
    await automationApi.cancelRun(runId);
  },

  deleteRun: async (runId) => {
    await automationApi.deleteRun(runId);
    set((s) => ({ runs: s.runs.filter((r) => r.id !== runId) }));
  },

  cleanupWorktree: (runId) => automationApi.cleanupWorktree(runId),

  upsertTask: (task) =>
    set((s) => ({
      tasks: s.tasks.some((t) => t.id === task.id)
        ? s.tasks.map((t) => (t.id === task.id ? task : t))
        : [task, ...s.tasks],
    })),

  upsertRun: (run) =>
    set((s) => {
      const filtered = s.runsTaskId !== null && run.task_id !== s.runsTaskId;
      if (filtered) return s;
      const exists = s.runs.some((r) => r.id === run.id);
      return {
        runs: exists
          ? s.runs.map((r) => (r.id === run.id ? run : r))
          : [run, ...s.runs].slice(0, 50),
      };
    }),

  clearError: () => set({ error: null }),
}));

/** Convenience helpers for the view. */
export function describeSchedule(
  schedule: ScheduleSpec,
  t?: (key: string, opts?: Record<string, unknown>) => string,
): string {
  if (schedule.kind === "interval") {
    const mins = Math.max(1, Math.round(schedule.every_secs / 60));
    if (mins >= 60) {
      const h = Math.round(mins / 60);
      return t?.("scheduled.everyHours", { count: h }) ?? `每 ${h} 小时`;
    }
    return t?.("scheduled.everyMinutes", { count: mins }) ?? `每 ${mins} 分钟`;
  }
  return t?.("scheduled.dailyAt", { time: schedule.time }) ?? `每天 ${schedule.time}`;
}
