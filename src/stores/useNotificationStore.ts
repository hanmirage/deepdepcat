/**
 * useNotificationStore — cross-session completion notification center.
 *
 * The backend emits `task-completed` when a background bash task exits, and
 * `agent-completed` when a scheduled/background AGENT run finishes with a
 * summary. This store keeps a bounded, deduplicated history of both so the
 * user can review completions from other sessions (mirrors Qwen's
 * task-notification center, adapted to a single-desktop app).
 */

import { create } from "zustand";
import { onEvent } from "@/lib/tauri";

export interface TaskNotificationItem {
  task_id: string;
  session_id: string;
  command: string;
  exit_code: number | null;
  status: string;
  createdAt: number;
  read: boolean;
}

export interface AgentNotificationItem {
  agent_id: string;
  session_id: string | null;
  name: string;
  summary: string;
  status: string;
  createdAt: number;
  read: boolean;
}

/** Union of the two notification kinds (bash task vs background agent run). */
export type NotificationItem = TaskNotificationItem | AgentNotificationItem;

interface TaskCompletedPayload {
  task_id: string;
  session_id: string;
  command: string;
  exit_code: number | null;
  status: string;
}

interface AgentCompletedPayload {
  agent_id: string;
  session_id: string | null;
  name: string;
  summary: string;
  status: string;
}

/** The identity field for markRead/dismiss regardless of kind. */
function notificationId(n: NotificationItem): string {
  return "agent_id" in n ? n.agent_id : n.task_id;
}

const MAX_NOTIFICATIONS = 50;

interface NotificationState {
  notifications: NotificationItem[];
  unreadCount: number;
  /** Install the `task-completed` + `agent-completed` listeners (idempotent). */
  subscribe: () => () => void;
  markRead: (id: string) => void;
  markAllRead: () => void;
  dismiss: (id: string) => void;
}

export const useNotificationStore = create<NotificationState>((set) => ({
  notifications: [],
  unreadCount: 0,

  subscribe: () => {
    let cancelled = false;
    const unlisteners: (() => void)[] = [];
    const push = (item: NotificationItem) =>
      set((s) => {
        // Deduplicate re-delivered events for the same task/agent.
        if (s.notifications.some((n) => notificationId(n) === notificationId(item))) return s;
        const next = [item, ...s.notifications].slice(0, MAX_NOTIFICATIONS);
        return { notifications: next, unreadCount: s.unreadCount + 1 };
      });
    const register = (p: Promise<() => void>) => {
      void p.then((fn) => {
        if (cancelled) fn();
        else unlisteners.push(fn);
      });
    };
    register(
      onEvent<TaskCompletedPayload>("task-completed", (payload) => {
        if (cancelled) return;
        push({
          task_id: payload.task_id,
          session_id: payload.session_id,
          command: payload.command,
          exit_code: payload.exit_code,
          status: payload.status,
          createdAt: Date.now(),
          read: false,
        });
      }),
    );
    register(
      onEvent<AgentCompletedPayload>("agent-completed", (payload) => {
        if (cancelled) return;
        push({
          agent_id: payload.agent_id,
          session_id: payload.session_id,
          name: payload.name,
          summary: payload.summary,
          status: payload.status,
          createdAt: Date.now(),
          read: false,
        });
      }),
    );
    return () => {
      cancelled = true;
      for (const fn of unlisteners) fn();
    };
  },

  markRead: (id) =>
    set((s) => {
      const next = s.notifications.map((n) =>
        notificationId(n) === id ? { ...n, read: true } : n,
      );
      return {
        notifications: next,
        unreadCount: next.filter((n) => !n.read).length,
      };
    }),

  markAllRead: () =>
    set((s) => ({
      notifications: s.notifications.map((n) => ({ ...n, read: true })),
      unreadCount: 0,
    })),

  dismiss: (id) =>
    set((s) => {
      const next = s.notifications.filter((n) => notificationId(n) !== id);
      return {
        notifications: next,
        unreadCount: next.filter((n) => !n.read).length,
      };
    }),
}));
