/**
 * Agent status hook — listens for status changes from the Rust backend.
 *
 * The backend emits `agent-status-changed` events when `set_agent_status`
 * is called. This hook subscribes and keeps the store in sync.
 */

import { useAppStore } from "@/stores/appStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { AgentStatus } from "@/lib/tauri";

export function useAgentStatus() {
  const agentStatus = useAppStore((s) => s.agentStatus);
  const setAgentStatus = useAppStore((s) => s.setAgentStatus);

  useTauriEvent<AgentStatus>("agent-status-changed", (status) => {
    setAgentStatus(status);
  });

  return agentStatus;
}
