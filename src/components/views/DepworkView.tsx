/**
 * DepworkView — Depwork (document) workspace.
 *
 * Chat-first, identical to Code (verified against the 2.0 build: the depwork
 * view is a plain conversation area with the input docked at the bottom and
 * the shared welcome page when empty). File browsing, document preview and
 * the dev browser live in the right drawer (RightPanel) on demand.
 */

import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { ChatViewShell } from "./ChatViewShell";
import { focusChatTextarea } from "@/lib/refineSelection";

export function DepworkView() {
  const messages = useDepworkChatStore((s) => s.messages);
  const compactions = useDepworkChatStore((s) => s.compactions);
  const sessionLoading = useDepworkChatStore((s) => s.sessionLoading);
  const setInputText = useDepworkChatStore((s) => s.setInputText);
  const notification = useDepworkChatStore((s) => s.notification);
  const dismissNotification = useDepworkChatStore((s) => s.dismissNotification);
  const pendingElicitation = useDepworkChatStore((s) => s.pendingElicitation);
  const respondElicitation = useDepworkChatStore((s) => s.respondElicitation);
  const currentSessionId = useDepworkChatStore((s) => s.currentSessionId);

  return (
    <ChatViewShell
      mode="depwork"
      messages={messages}
      compactions={compactions}
      isEmpty={messages.length === 0}
      loading={sessionLoading}
      notification={notification}
      dismissNotification={dismissNotification}
      sessionId={currentSessionId}
      pendingElicitation={pendingElicitation}
      respondElicitation={respondElicitation}
      onRefineSelection={(draft) => {
        setInputText(draft);
        focusChatTextarea();
      }}
    />
  );
}
