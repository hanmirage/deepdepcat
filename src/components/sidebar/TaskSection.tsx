/**
 * TaskSection — live task list in the sidebar.
 *
 * Rendered between the tabs and the conversation list in both modes. Fed by
 * useTaskEvents (task-update / scheduler-task-created events + list_tasks
 * pull). Shows only while at least one task exists.
 */

import { useTranslation } from "react-i18next";
import { useTaskStore } from "@/stores/taskStore";
import { cn } from "@/lib/utils";

function statusColor(status: string): string {
  switch (status) {
    case "completed":
      return "bg-emerald-500";
    case "running":
    case "in_progress":
    case "scheduled":
      return "bg-primary animate-pulse";
    case "failed":
    case "killed":
    case "cancelled":
      return "bg-destructive";
    default:
      return "bg-muted-foreground/40";
  }
}

export function TaskSection() {
  const { t } = useTranslation();
  const tasks = useTaskStore((s) => s.tasks);

  if (tasks.length === 0) return null;

  return (
    <div className="border-b border-[hsl(var(--sidebar-border))] px-2 py-2">
      <p className="px-2 pb-1 text-[10px] font-semibold uppercase tracking-widest text-muted-foreground/50">
        {t("sidebar.taskTitle")}
      </p>
      <div className="space-y-0.5">
        {tasks.map((task) => (
          <div key={task.id} className="flex items-center gap-2 rounded-md px-2 py-1">
            <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", statusColor(task.status))} />
            <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground" title={task.content}>
              {task.content}
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
