/**
 * Tauri API bridge — scheduled agent tasks (定时任务).
 *
 * Command names mirror `src-tauri/src/commands/automation_cmd.rs`.
 */

import { invoke } from "@tauri-apps/api/core";
import { isTauri } from "../core";
import type { ScheduledRun, ScheduledTask, ScheduleSpec } from "@/types/scheduled";

export interface CreateScheduledTaskInput {
  name: string;
  prompt: string;
  schedule: ScheduleSpec;
  projectPath?: string;
  useWorktree?: boolean;
  workMode?: "code" | "depwork";
  model?: string;
  /** Persistent mode: the agent reuses one session across fires (常驻). */
  persistent?: boolean;
}

export const automationApi = {
  listTasks: (): Promise<ScheduledTask[]> =>
    isTauri ? invoke<ScheduledTask[]>("list_scheduled_tasks") : Promise.resolve([]),

  createTask: (input: CreateScheduledTaskInput): Promise<ScheduledTask> => {
    if (!isTauri) {
      return Promise.reject(new Error("定时任务仅在桌面应用中可用"));
    }
    const base = {
      name: input.name,
      prompt: input.prompt,
      scheduleKind: input.schedule.kind,
      everySecs: input.schedule.kind === "interval" ? input.schedule.every_secs : null,
      dailyTime: input.schedule.kind === "daily" ? input.schedule.time : null,
      projectPath: input.projectPath ?? null,
      useWorktree: input.useWorktree ?? false,
      workMode: input.workMode ?? "code",
      model: input.model ?? null,
      persistent: input.persistent ?? false,
    };
    return invoke<ScheduledTask>("create_scheduled_task", base);
  },

  updateTask: (
    id: string,
    patch: Partial<{
      name: string;
      prompt: string;
      schedule: ScheduleSpec;
      projectPath: string;
      useWorktree: boolean;
      workMode: "code" | "depwork";
      model: string;
      active: boolean;
      persistent: boolean;
    }>,
  ): Promise<ScheduledTask> => {
    if (!isTauri) return Promise.reject(new Error("定时任务仅在桌面应用中可用"));
    return invoke<ScheduledTask>("update_scheduled_task", {
      id,
      name: patch.name ?? null,
      prompt: patch.prompt ?? null,
      scheduleKind: patch.schedule?.kind ?? null,
      everySecs: patch.schedule?.kind === "interval" ? patch.schedule.every_secs : null,
      dailyTime: patch.schedule?.kind === "daily" ? patch.schedule.time : null,
      projectPath: patch.projectPath ?? null,
      useWorktree: patch.useWorktree ?? null,
      workMode: patch.workMode ?? null,
      model: patch.model ?? null,
      active: patch.active ?? null,
      persistent: patch.persistent ?? null,
    });
  },

  deleteTask: (id: string): Promise<void> =>
    isTauri ? invoke<void>("delete_scheduled_task", { id }) : Promise.resolve(),

  listRuns: (taskId?: string, limit?: number): Promise<ScheduledRun[]> =>
    isTauri
      ? invoke<ScheduledRun[]>("list_scheduled_runs", { taskId: taskId ?? null, limit: limit ?? 50 })
      : Promise.resolve([]),

  deleteRun: (runId: string): Promise<void> =>
    isTauri ? invoke<void>("delete_scheduled_run", { runId }) : Promise.resolve(),

  runNow: (taskId: string): Promise<string> =>
    isTauri ? invoke<string>("run_scheduled_task_now", { taskId }) : Promise.reject(new Error("定时任务仅在桌面应用中可用")),

  cancelRun: (runId: string): Promise<void> =>
    isTauri ? invoke<void>("cancel_scheduled_run", { runId }) : Promise.resolve(),

  cleanupWorktree: (runId: string): Promise<string> =>
    isTauri ? invoke<string>("cleanup_scheduled_worktree", { runId }) : Promise.reject(new Error("定时任务仅在桌面应用中可用")),
};
