/**
 * usePlanSummaryPane — opens/closes the right-panel "plan" pane across the
 * full plan lifecycle: entering plan mode opens it, an approved plan KEEPS it
 * open through execution (the pane renders the plan MD), and it collapses
 * when the plan is archived (backend `plan-archived`, run end) or the mode
 * exits without an approved plan.
 */

import { useEffect, useRef } from "react";
import type { AppMode } from "@/config/constants";
import { useAppStore } from "@/stores/appStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useCurrentPlan, useIsSessionInPlanMode } from "@/stores/planStore";

export function usePlanSummaryPane(mode: AppMode) {
  const codeSessionId = useChatStore((s) => s.currentSessionId);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);
  const sessionId = mode === "depwork" ? depworkSessionId : codeSessionId;
  const inPlanMode = useIsSessionInPlanMode(sessionId);
  const currentPlan = useCurrentPlan();
  const hasCurrentPlan = currentPlan?.sessionId === sessionId;
  const openPane = useRightPanelStore((s) => s.openPane);
  const closePane = useRightPanelStore((s) => s.closePane);

  const prevInPlanMode = useRef(inPlanMode);
  const prevHasPlan = useRef(hasCurrentPlan);
  useEffect(() => {
    const planEntered = inPlanMode && !prevInPlanMode.current;
    const planExited = !inPlanMode && prevInPlanMode.current;
    const planAppeared = hasCurrentPlan && !prevHasPlan.current;
    const planArchived = !hasCurrentPlan && prevHasPlan.current;
    prevInPlanMode.current = inPlanMode;
    prevHasPlan.current = hasCurrentPlan;

    // Open when entering plan mode or when an approved plan becomes live.
    if (planEntered || planAppeared) {
      openPane(mode, "plan");
      return;
    }
    // Close when the plan is archived (run end) — approval already restored
    // the execution mode, so this is the ONLY path that drops a live plan.
    if (planArchived) {
      closePane(mode, "plan");
      return;
    }
    // Leaving plan mode WITHOUT an approved plan (abandoned) closes too.
    if (planExited && !hasCurrentPlan) {
      closePane(mode, "plan");
    }
  }, [inPlanMode, hasCurrentPlan, sessionId, mode, openPane, closePane]);
}
