/**
 * useAskUserEvents — subscribes to backend "ask-user" events.
 *
 * The agent's ask_user tool emits this event and then blocks waiting
 * for a reply. The payload is routed to the store of the session that
 * owns the request:
 *   - current depwork session  → depworkChatStore (its own dialog)
 *   - anything else            → chatStore (code mode, unchanged behavior)
 *
 * Routing by session_id keeps two concurrent sessions from answering
 * each other's questions from the wrong store.
 */

import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { UserAskRequest } from "@/lib/tauri";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useAppStore } from "@/stores/appStore";

export function useAskUserEvents() {
  useTauriEvent<UserAskRequest>("ask-user", (req) => {
    // Route to the store the current surface's dialog actually reads. The
    // backend resumes the blocked agent by request_id, not by session, so a
    // background session's ask is still answerable once it lands in the
    // visible store — otherwise it would be invisible and the agent would
    // hang for its 5-minute timeout.
    const mode = useAppStore.getState().mode;
    if (mode === "depwork") {
      useDepworkChatStore.getState().setPendingAskUser(req);
    } else {
      useChatStore.getState().setPendingAskUser(req);
    }
  });
}
