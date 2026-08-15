/**
 * ChatView — thin wrapper for the Code chat view.
 *
 * Selects code-mode data from chatStore and passes it to the shared ChatViewShell.
 */

import { useChatStore } from "@/stores/chatStore";
import { ChatViewShell } from "./ChatViewShell";
import { focusChatTextarea } from "@/lib/refineSelection";
import { AnnouncementBanner } from "@/components/chat/AnnouncementBanner";

export function ChatView() {
  const messages = useChatStore((s) => s.messages);
  const compactions = useChatStore((s) => s.compactions);
  const sessionLoading = useChatStore((s) => s.sessionLoading);
  const setInputText = useChatStore((s) => s.setInputText);
  const notification = useChatStore((s) => s.notification);
  const dismissNotification = useChatStore((s) => s.dismissNotification);
  const pendingElicitation = useChatStore((s) => s.pendingElicitation);
  const respondElicitation = useChatStore((s) => s.respondElicitation);
  const currentSessionId = useChatStore((s) => s.currentSessionId);

  return (
    <>
      <AnnouncementBanner />
      <ChatViewShell
        mode="code"
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
    </>
  );
}
