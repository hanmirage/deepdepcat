/**
 * Plan-approval store (Zustand).
 *
 * Holds the parked plan-approval request (backend `plan-approval-request`
 * event) and the per-session "waiting for you" interaction list (backend
 * `pending-interactions` event).
 *
 * The panel reads `approval`; when the user decides, `respond` sends
 * approve/reject (+feedback) to the backend, then clears the request.
 */

import { create } from "zustand";
import { logError } from "@/lib/logger";
import type { PlanApprovalRequest, PendingInteraction } from "@/types";
import { permissionApi } from "@/lib/tauri";

interface PlanState {
  /** The plan currently awaiting the user's decision (null when none). */
  approval: PlanApprovalRequest | null;

  /** The approved plan's raw Markdown, retained through execution until the
   *  backend archives it (`plan-archived`, emitted on run end). The plan
   *  pane renders this; unlike `approval` it is NOT cleared by `respond`. */
  currentPlan: { sessionId: string; plan: string } | null;

  /** Per-session pending interactions — surfaced as a status indicator. */
  interactions: Record<string, PendingInteraction[]>;

  /** True while a decision is being sent — ignores further responses. */
  responding: boolean;

  /** Sessions currently in plan mode (keyed by session id). Driven by the
   *  backend `plan-mode-changed` event so the input bar can show a live
   *  "plan mode" posture while the agent explores and drafts a plan. */
  planModeSessions: Record<string, true>;

  /** Set the parked plan request (called by the event listener hook). */
  setApproval: (req: PlanApprovalRequest | null) => void;

  /** Set/clear the retained approved plan (event listener hook). */
  setCurrentPlan: (entry: { sessionId: string; plan: string } | null) => void;

  /** Replace the interaction list for a session (event listener hook). */
  setInteractions: (sessionId: string, list: PendingInteraction[]) => void;

  /** Mark a session as in/out of plan mode (event listener hook). */
  setSessionPlanMode: (sessionId: string, inPlanMode: boolean) => void;

  /** Send the user's decision for the parked plan and clear it. */
  respond: (decision: "approve" | "reject", feedback?: string) => Promise<void>;
}

export const usePlanStore = create<PlanState>((set, get) => ({
  approval: null,
  currentPlan: null,
  interactions: {},
  responding: false,
  planModeSessions: {},

  setApproval: (req) =>
    set((s) => {
      if (s.approval?.request_id === req?.request_id) return s;
      return { approval: req };
    }),

  setCurrentPlan: (entry) => set({ currentPlan: entry }),

  setInteractions: (sessionId, list) =>
    set((s) => ({ interactions: { ...s.interactions, [sessionId]: list } })),

  setSessionPlanMode: (sessionId, inPlanMode) =>
    set((s) => {
      const planModeSessions = { ...s.planModeSessions };
      if (inPlanMode) {
        planModeSessions[sessionId] = true;
      } else {
        delete planModeSessions[sessionId];
      }
      return { planModeSessions };
    }),

  respond: async (decision, feedback) => {
    const req = get().approval;
    if (!req || get().responding) return;

    set({ responding: true });
    try {
      await permissionApi.respondPlanApproval(req.request_id, decision, feedback);
    } catch (e) {
      logError("planStore", "Failed to send plan decision:", e);
    } finally {
      set((s) => ({
        approval: s.approval?.request_id === req.request_id ? null : s.approval,
        // A rejected plan won't be executed — drop the retained copy so the
        // plan pane doesn't linger on a discarded plan. An approved plan is
        // kept through execution and cleared by `plan-archived` on run end.
        currentPlan:
          decision === "reject" && s.currentPlan?.sessionId === req.session_id
            ? null
            : s.currentPlan,
        responding: false,
      }));
    }
  },
}));

/** Derived selector — the plan request to display. */
export function useCurrentPlanApproval(): PlanApprovalRequest | null {
  return usePlanStore((s) => s.approval);
}

/** Derived selector — the retained approved plan (rendered by the plan pane). */
export function useCurrentPlan(): { sessionId: string; plan: string } | null {
  return usePlanStore((s) => s.currentPlan);
}

/** Derived selector — the session's pending interactions. */
export function usePendingInteractions(
  sessionId: string | null | undefined,
): PendingInteraction[] {
  return usePlanStore((s) => (sessionId ? s.interactions[sessionId] : undefined) ?? EMPTY_INTERACTIONS);
}

/** Stable empty list — selectors must return referentially stable values. */
const EMPTY_INTERACTIONS: PendingInteraction[] = [];

/** Derived selector — whether a session is currently in plan mode. */
export function useIsSessionInPlanMode(sessionId: string | null | undefined): boolean {
  return usePlanStore((s) => (sessionId ? s.planModeSessions[sessionId] === true : false));
}
