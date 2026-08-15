/**
 * useScheduledEvents — subscribes to the scheduled-task backend pipeline.
 *
 * Mount: pulls tasks + runs once. Live rows arrive through
 * `scheduled-task-updated` / `scheduled-run-updated` (full serde rows).
 */

import { useEffect } from "react";
import { isTauri } from "@/lib/tauri";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { useScheduledStore } from "@/stores/scheduledStore";
import type { ScheduledRun, ScheduledTask } from "@/types/scheduled";

export function useScheduledEvents() {
  useEffect(() => {
    if (!isTauri) return;
    void useScheduledStore.getState().load();
  }, []);

  useTauriEvent<ScheduledTask>("scheduled-task-updated", (task) => {
    useScheduledStore.getState().upsertTask(task);
  });

  useTauriEvent<ScheduledRun>("scheduled-run-updated", (run) => {
    useScheduledStore.getState().upsertRun(run);
  });
}
