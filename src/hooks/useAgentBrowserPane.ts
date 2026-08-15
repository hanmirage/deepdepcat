/**
 * useAgentBrowserPane — auto-open the live "browser" pane when the agent
 * starts its real browser (`browser_control`), and dismiss it when it stops.
 *
 * Distinct from useRightPanelBrowser (which opens the "preview" pane for
 * generated artifacts): the browser pane mirrors the agent's real Chromium
 * via the backend `browser-status-changed` event, scoped to the CURRENT
 * session's browser profile (`session-<id>`).
 */

import { useAppStore } from "@/stores/appStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import {
  sessionBrowserProfile,
  BROWSER_STATUS_CHANGED_EVENT,
  type BrowserStatusChangedEvent,
} from "@/lib/tauri";

export function useAgentBrowserPane() {
  const mode = useAppStore((s) => s.mode);
  const codeSessionId = useChatStore((s) => s.currentSessionId);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);
  const sessionId = mode === "depwork" ? depworkSessionId : codeSessionId;
  const openPane = useRightPanelStore((s) => s.openPane);
  const closePane = useRightPanelStore((s) => s.closePane);

  useTauriEvent<BrowserStatusChangedEvent>(BROWSER_STATUS_CHANGED_EVENT, (e) => {
    const profile = sessionBrowserProfile(sessionId);
    if (!profile || e.profile !== profile) return;
    if (e.running) {
      openPane(mode, "browser");
    } else {
      closePane(mode, "browser");
    }
  });
}
