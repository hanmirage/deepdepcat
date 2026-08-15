/**
 * useAgentNotifications — desktop system toast when a scheduled/background
 * AGENT run finishes with a report. The `agent-completed` event carries a
 * summary (the autonomous agent's report), so the toast is the "the agent
 * did X" notification — surfaced regardless of the foreground session.
 */

import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { useTranslation } from "react-i18next";
import { useTauriEvent } from "@/hooks/useTauriEvent";

interface AgentCompletedPayload {
  agent_id: string;
  session_id: string | null;
  name: string;
  summary: string;
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

export function useAgentNotifications() {
  const { t } = useTranslation();
  useTauriEvent<AgentCompletedPayload>("agent-completed", (payload) => {
    const succeeded = payload.status === "completed";
    void ensurePermission()
      .then((granted) => {
        if (!granted) return;
        const summary =
          payload.summary.length > 120
            ? `${payload.summary.slice(0, 120)}…`
            : payload.summary;
        sendNotification({
          title: succeeded
            ? t("notifications.agentDone", { name: payload.name })
            : t("notifications.agentFailed", { name: payload.name }),
          body: summary || t("notifications.noSummary"),
        });
      })
      .catch(() => {
        // Notification failure is non-fatal.
      });
  });
}
