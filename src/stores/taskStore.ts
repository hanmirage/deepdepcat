/**
 * Task store (Zustand) — live task list for the sidebar.
 *
 * Fed by the backend `task-update` / `scheduler-task-created` events (and a
 * one-shot `list_tasks` pull on mount). Both the depwork task pipeline and
 * the scheduler emit here, so tasks created by the agent or the scheduler
 * are visible across modes.
 */

import { create } from "zustand";

/** Normalized task row (wire shapes differ between sources). */
export interface TaskRow {
  id: string;
  content: string;
  status: string;
}

interface TaskState {
  tasks: TaskRow[];
  /** Replace the whole list (task-update snapshot / initial pull). */
  setTasks: (tasks: TaskRow[]) => void;
  /** Upsert a single task (scheduler-task-created). */
  upsertTask: (task: TaskRow) => void;
  clear: () => void;
}

export const useTaskStore = create<TaskState>((set) => ({
  tasks: [],
  setTasks: (tasks) => set({ tasks }),
  upsertTask: (task) =>
    set((s) => {
      const existing = s.tasks.some((t) => t.id === task.id);
      return {
        tasks: existing
          ? s.tasks.map((t) => (t.id === task.id ? task : t))
          : [...s.tasks, task],
      };
    }),
  clear: () => set({ tasks: [] }),
}));
