/**
 * useTaskEvents — subscribes to the backend task pipeline.
 *
 * - Mount: pulls the current task list once (`list_tasks`).
 * - `task-update`: the task-manage tool emitted a full snapshot.
 * - `scheduler-task-created`: the scheduler registered a new task.
 *
 * All writes go through taskStore so the sidebar task section renders the
 * live state.
 */

import { useEffect } from "react";
import { isTauri, taskApi } from "@/lib/tauri";
import { useTaskStore } from "@/stores/taskStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";

/** TaskItem shape from the task-update snapshot (task_manage.rs). */
interface TaskItem {
  id: string;
  content: string;
  status: string;
  active: boolean;
}

/** ScheduledTask shape from scheduler-task-created (scheduler.rs). */
interface ScheduledTask {
  id: string;
  name: string;
  command: string;
  interval_secs: number;
  active: boolean;
  run_count: number;
}

export function useTaskEvents() {
  useEffect(() => {
    const store = useTaskStore.getState();
    store.clear();

    if (!isTauri) return;

    // One-shot pull — maps CoworkTask (description) to the task row shape.
    void (async () => {
      try {
        const tasks = await taskApi.listTasks();
        store.setTasks(tasks.map((t) => ({ id: t.id, content: t.description, status: t.status })));
      } catch {
        // Backend unavailable — the event listeners will populate later.
      }
    })();

  }, []);

  useTauriEvent<TaskItem[]>("task-update", (e) => {
    useTaskStore
      .getState()
      .setTasks(e.map((t) => ({ id: t.id, content: t.content, status: t.status })));
  });

  useTauriEvent<ScheduledTask>("scheduler-task-created", (e) => {
    useTaskStore.getState().upsertTask({
      id: e.id,
      content: e.name,
      status: e.active ? "scheduled" : "cancelled",
    });
  });
}
