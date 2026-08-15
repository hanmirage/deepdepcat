/**
 * usePlanApprovalEvents — subscribes to the backend's plan-approval events.
 *
 * - `plan-approval-request` (snake_case payload, see Rust
 *   `PlanApprovalRequest` in permissions/plan.rs): a plan is parked and
 *   the user must decide.
 * - `pending-interactions` (payload { session_id, interactions }): the
 *   full "waiting for you" list for a session — only the current session's
 *   entries are kept.
 */

import { useEffect } from "react";
import { usePlanStore } from "@/stores/planStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { PlanApprovalRequest, PendingInteraction } from "@/types";

export function usePlanApprovalEvents(sessionId?: string | null) {
  const setApproval = usePlanStore((s) => s.setApproval);
  const setCurrentPlan = usePlanStore((s) => s.setCurrentPlan);
  const setInteractions = usePlanStore((s) => s.setInteractions);
  const setSessionPlanMode = usePlanStore((s) => s.setSessionPlanMode);

  // Switching sessions must clear the previous session's "waiting for you"
  // list — it's per-session and the new session may never emit
  // pending-interactions, so without this the plan pane would show stale
  // entries from the session the user left.
  useEffect(() => {
    if (sessionId) setInteractions(sessionId, []);
  }, [sessionId, setInteractions]);

  useTauriEvent<PlanApprovalRequest>("plan-approval-request", (event) => {
    setApproval(event);
    // Retain the plan Markdown through execution — cleared by plan-archived
    // when the run ends and the backend archives it to disk.
    setCurrentPlan({ sessionId: event.session_id, plan: event.plan });
  });

  useTauriEvent<{ session_id: string }>("plan-archived", (event) => {
    const current = usePlanStore.getState().currentPlan;
    if (current?.sessionId === event.session_id) setCurrentPlan(null);
  });

  useTauriEvent<{
    session_id: string;
    interactions: PendingInteraction[];
  }>("pending-interactions", (event) => {
    // Only the active session's interactions are shown.
    if (sessionId && event.session_id !== sessionId) return;
    setInteractions(event.session_id, event.interactions ?? []);
  });

  // The agent self-dispatches into plan mode via enter_plan_mode; the backend
  // broadcasts the session's effective mode on enter/exit/run-end. Track it so
  // the input bar can show a live "plan mode" posture (mode === "read_only").
  useTauriEvent<{ session_id: string; mode: string }>("plan-mode-changed", (event) => {
    setSessionPlanMode(event.session_id, event.mode === "read_only");
  });
}
