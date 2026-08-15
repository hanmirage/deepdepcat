/**
 * newChat — "start a new conversation in the CURRENT product mode".
 *
 * Shared by the sidebar's New Chat button and Ctrl/Cmd+N so the shortcut
 * behaves exactly like the button: it never yanks the user into the other
 * product surface. Both chat stores stay mounted, so only the active
 * surface's messages/session are reset; the other product's work continues.
 */

import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";

export function newChatInCurrentMode(): void {
  const mode = useAppStore.getState().mode;
  const store = mode === "depwork" ? useDepworkChatStore : useChatStore;
  store.getState().clearMessages();
  useRightPanelStore.getState().setOpen(false, mode);
}
