/**
 * useTaskSystemNotifications — desktop system notifications for background
 * task completion, surfaced only when the task does NOT belong to the
 * currently visible chat session (the in-chat banner already covers that
 * case).
 *
 * Mirrors Qwen's task-notification protocol, simplified for the desktop:
 * the task-completed event is consumed by (1) the in-chat banner when the
 * session is active, and (2) a system toast when the user is elsewhere.
 */

import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";

interface TaskCompletedPayload {
  task_id: string;
  session_id: string;
  command: string;
  exit_code: number | null;
  status: string;
}

let permissionPromise: Promise<boolean> | null = null;

function ensurePermission(): Promise<boolean> {
  if (!permissionPromise) {
    permissionPromise = (async (): Promise<boolean> => {
      try {
        const granted = await isPermissionGranted();
        if (granted) return true;
        const result = await requestPermission();
        return result === "granted";
      } catch {
        return false;
      }
    })();
  }
  return permissionPromise;
}
export function useTaskSystemNotifications() {
  useTauriEvent<TaskCompletedPayload>("task-completed", (payload) => {
    const activeId = useChatStore.getState().currentSessionId;
    const depworkId = useDepworkChatStore.getState().currentSessionId;
    const isForegroundSession =
      payload.session_id === activeId || payload.session_id === depworkId;
    // Same-session completions are surfaced by the in-chat banner; only
    // cross-session (or backgrounded-window) completions need a toast.
    if (isForegroundSession && !document.hidden) return;

    const succeeded = payload.status === "completed";
    void ensurePermission().then((granted) => {
      if (!granted) return;
      const command =
        payload.command.length > 100
          ? `${payload.command.slice(0, 100)}…`
          : payload.command;
      sendNotification({
        title: succeeded ? "Background task completed" : "Background task failed",
        body: `${command}\nexit code: ${payload.exit_code ?? "?"}`,
      });
    }).catch(() => {
      // Notification permission/send failure is non-fatal — the in-chat
      // banner and task panel still carry the completion.
    });
  });
}
