/**
 * planStore tests — plan-mode session tracking.
 *
 * The input-bar "plan mode" indicator is driven by the backend
 * `plan-mode-changed` event, which calls `setSessionPlanMode` here. Covers:
 *  - entering plan mode marks the session
 *  - leaving plan mode clears it
 *  - sessions are tracked independently
 *  - re-entering is idempotent
 */

import { describe, it, expect, beforeEach } from "vitest";
import { usePlanStore } from "@/stores/planStore";

describe("planStore plan-mode tracking", () => {
  beforeEach(() => {
    usePlanStore.setState({
      approval: null,
      interactions: {},
      responding: false,
      planModeSessions: {},
    });
  });

  it("starts with no session in plan mode", () => {
    expect(usePlanStore.getState().planModeSessions).toEqual({});
  });

  it("marks a session in plan mode and clears it on exit", () => {
    const store = usePlanStore.getState();
    store.setSessionPlanMode("s1", true);
    expect(usePlanStore.getState().planModeSessions.s1).toBe(true);

    store.setSessionPlanMode("s1", false);
    expect(usePlanStore.getState().planModeSessions.s1).toBeUndefined();
  });

  it("tracks multiple sessions independently", () => {
    const store = usePlanStore.getState();
    store.setSessionPlanMode("s1", true);
    store.setSessionPlanMode("s2", true);

    expect(usePlanStore.getState().planModeSessions).toEqual({ s1: true, s2: true });

    store.setSessionPlanMode("s1", false);
    expect(usePlanStore.getState().planModeSessions).toEqual({ s2: true });
  });

  it("re-entering plan mode is idempotent", () => {
    const store = usePlanStore.getState();
    store.setSessionPlanMode("s1", true);
    store.setSessionPlanMode("s1", true);
    expect(usePlanStore.getState().planModeSessions).toEqual({ s1: true });
  });
});

describe("planStore currentPlan retention", () => {
  beforeEach(() => {
    usePlanStore.setState({
      approval: null,
      currentPlan: null,
      interactions: {},
      responding: false,
      planModeSessions: {},
    });
  });

  it("stores the approved plan via setCurrentPlan", () => {
    usePlanStore.getState().setCurrentPlan({ sessionId: "s1", plan: "# plan" });
    expect(usePlanStore.getState().currentPlan).toEqual({
      sessionId: "s1",
      plan: "# plan",
    });
  });

  it("approving keeps the retained plan through execution", async () => {
    usePlanStore.setState({
      approval: {
        request_id: "r1",
        session_id: "s1",
        plan: "# plan",
        changed_files: [],
        created_at: 0,
      },
      currentPlan: { sessionId: "s1", plan: "# plan" },
    });
    await usePlanStore.getState().respond("approve");
    expect(usePlanStore.getState().currentPlan).toEqual({
      sessionId: "s1",
      plan: "# plan",
    });
  });

  it("rejecting clears the retained plan", async () => {
    usePlanStore.setState({
      approval: {
        request_id: "r1",
        session_id: "s1",
        plan: "# plan",
        changed_files: [],
        created_at: 0,
      },
      currentPlan: { sessionId: "s1", plan: "# plan" },
    });
    await usePlanStore.getState().respond("reject", "revise it");
    expect(usePlanStore.getState().currentPlan).toBeNull();
  });
});
