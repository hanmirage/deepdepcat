/**
 * useRunningSessions — global persistence watcher.
 *
 * Polls the backend running-turn registry (5s safety net), refreshes on
 * start/completion events, and raises a desktop toast when a background
 * turn finishes while the user is looking at a different session.
 */

import { useEffect } from "react";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useRunningSessionsStore } from "@/stores/runningSessionsStore";
import type { RunningTurnInfo, TurnCompletedPayload } from "@/lib/tauri";

const POLL_INTERVAL_MS = 5_000;

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

function toastBackgroundCompletion(payload: TurnCompletedPayload) {
  const activeId = useChatStore.getState().currentSessionId;
  const depworkId = useDepworkChatStore.getState().currentSessionId;
  const isForegroundSession =
    payload.session_id === activeId || payload.session_id === depworkId;
  // Foreground sessions surface completion in-chat; only cross-session
  // completions (or a hidden window) need a toast.
  if (isForegroundSession && !document.hidden) return;
  const succeeded = payload.status === "completed";
  void ensurePermission()
    .then((granted) => {
      if (!granted) return;
      sendNotification({
        title: succeeded
          ? "Background agent completed"
          : "Background agent stopped",
        body: payload.status,
      });
    })
    .catch(() => {
      // Notification failure is non-fatal.
    });
}

export function useRunningSessions() {
  const refresh = useRunningSessionsStore((s) => s.refresh);
  const pushCompleted = useRunningSessionsStore((s) => s.pushCompleted);

  useTauriEvent<RunningTurnInfo>("agent-turn-started", () => {
    void refresh();
  });

  useTauriEvent<TurnCompletedPayload>("agent-turn-completed", (payload) => {
    pushCompleted(payload);
    void refresh();
    toastBackgroundCompletion(payload);
  });

  useEffect(() => {
    void refresh();
    const iv = setInterval(() => void refresh(), POLL_INTERVAL_MS);
    return () => clearInterval(iv);
  }, [refresh]);
}
