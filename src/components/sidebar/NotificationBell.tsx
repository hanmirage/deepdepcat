/**
 * NotificationBell — sidebar bell with an unread badge and a dropdown
 * listing recent background-task completions from any session.
 *
 * Clicking an entry switches to the owning session (via the restore hook)
 * and marks it read, mirroring Qwen's task-notification center.
 *
 * Opening the dropdown does NOT mark everything read — "unread" must mean
 * "not seen yet", so the badge only clears when the user actually clicks
 * an entry.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Bell, CheckCircle2, XCircle, X } from "lucide-react";
import { useNotificationStore } from "@/stores/useNotificationStore";
import { useSessionRestore } from "@/hooks/useSessionRestore";
import { cn } from "@/lib/utils";

export function NotificationBell() {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const { notifications, unreadCount, markRead, dismiss } =
    useNotificationStore();
  const { selectSessionById } = useSessionRestore();

  // Close on outside click.
  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    window.addEventListener("mousedown", handler);
    return () => window.removeEventListener("mousedown", handler);
  }, [open]);

  // Escape closes the dropdown.
  useEffect(() => {
    if (!open) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [open]);

  const handleSelect = useCallback(
    (taskId: string, sessionId: string) => {
      markRead(taskId);
      setOpen(false);
      if (sessionId) {
        void selectSessionById(sessionId);
      }
    },
    [markRead, selectSessionById],
  );

  return (
    <div ref={ref} className="relative">
      <button
        onClick={() => setOpen((o) => !o)}
        className="relative flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted/80 hover:text-foreground"
        title={t("notifications.title")}
        aria-label={t("notifications.title")}
        aria-expanded={open}
        aria-haspopup="true"
      >
        <Bell className="h-3.5 w-3.5" />
        {unreadCount > 0 && (
          <span className="absolute -right-0.5 -top-0.5 flex h-3.5 min-w-3.5 items-center justify-center rounded-full bg-destructive px-0.5 text-[9px] font-semibold text-white">
            {unreadCount > 9 ? "9+" : unreadCount}
          </span>
        )}
      </button>

      {open && (
        <div
          className="absolute left-0 top-8 z-50 w-80 max-w-[calc(100vw-2rem)] overflow-hidden rounded-lg border bg-popover text-popover-foreground shadow-lg"
          role="menu"
        >
          <div className="flex items-center justify-between border-b px-3 py-2">
            <p className="text-xs font-medium">{t("notifications.title")}</p>
            <span className="text-[10px] text-muted-foreground">
              {unreadCount > 0 && (
                <span className="mr-1.5 rounded bg-destructive/10 px-1 py-px text-[9px] font-medium text-destructive">
                  {unreadCount} {t("notifications.unread", { defaultValue: "未读" })}
                </span>
              )}
              {notifications.length} / {t("notifications.max", { count: 50 })}
            </span>
          </div>
          <div className="max-h-72 overflow-y-auto">
            {notifications.length === 0 ? (
              <p className="px-3 py-6 text-center text-xs text-muted-foreground">
                {t("notifications.empty")}
              </p>
            ) : (
              notifications.map((n) => {
                const succeeded = n.status === "completed";
                const isAgent = "agent_id" in n;
                const id = isAgent ? n.agent_id : n.task_id;
                const label = isAgent
                  ? n.name
                  : (n as { command: string }).command;
                return (
                  <div
                    key={id}
                    className={cn(
                      "group flex items-start gap-2 border-b px-3 py-2 transition-colors hover:bg-muted/50",
                      !n.read && "bg-primary/5",
                    )}
                  >
                    <button
                      className="flex min-w-0 flex-1 items-start gap-2 text-left"
                      onClick={() => handleSelect(id, n.session_id ?? "")}
                    >
                      {succeeded ? (
                        <CheckCircle2 className="mt-0.5 h-3.5 w-3.5 shrink-0 text-emerald-500" />
                      ) : (
                        <XCircle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-amber-500" />
                      )}
                      <div className="min-w-0 flex-1">
                        <p className="truncate text-xs font-medium">
                          {succeeded
                            ? t("notifications.completed")
                            : t("notifications.failed")}
                        </p>
                        <p className="truncate font-mono text-[10px] text-muted-foreground">
                          {label}
                        </p>
                        {isAgent && n.summary && (
                          <p className="mt-0.5 line-clamp-2 text-[10px] text-muted-foreground/70">
                            {n.summary}
                          </p>
                        )}
                        <p className="mt-0.5 text-[10px] text-muted-foreground/70">
                          {new Date(n.createdAt).toLocaleTimeString(i18n.language)}
                          {" · "}
                          {t("notifications.viewSession")}
                        </p>
                      </div>
                    </button>
                    <button
                      onClick={() => dismiss(id)}
                      className="rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:bg-secondary/60 hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100"
                      aria-label={t("common.dismiss")}
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </div>
                );
              })
            )}
          </div>
        </div>
      )}
    </div>
  );
}
