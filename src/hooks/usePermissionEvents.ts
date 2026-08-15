/**
 * usePermissionEvents — subscribes to backend permission-request events.
 *
 * The backend emits one event: `permission_request` (snake_case payload,
 * see Rust `PermissionRequest` in core/types/session.rs):
 *   request_id, tool_name, args_summary, session_id.
 *
 * Multiple requests can arrive in quick succession (parallel agents, rapid
 * tool calls) — each is enqueued and shown one at a time by PermissionDialog.
 */

import { usePermissionStore } from "@/stores/permissionStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { PermissionRequest } from "@/types";

export function usePermissionEvents() {
  const enqueue = usePermissionStore((s) => s.enqueue);

  useTauriEvent<PermissionRequest>("permission_request", (event) => {
    enqueue(event);
  });
}
