/**
 * permissionStore tests — the permission-request queue.
 *
 * Covers:
 *  - enqueue appends in arrival order
 *  - duplicates are ignored
 *  - responding to the head advances the queue
 *  - the current request selector returns the queue head
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import {
  usePermissionStore,
  visiblePermissionRequests,
  visibleDenials,
  requestBelongsToSession,
} from "@/stores/permissionStore";
import { permissionApi } from "@/lib/tauri";

const req = (id: string) => ({
  request_id: id,
  tool_name: "read_file",
  args_summary: `read ${id}`,
  session_id: "s1",
});

describe("permissionStore queue", () => {
  beforeEach(() => {
    usePermissionStore.setState({ queue: [], responding: false, enqueuedAt: {} });
    vi.restoreAllMocks();
  });

  it("starts empty", () => {
    expect(usePermissionStore.getState().queue).toEqual([]);
  });

  it("enqueues requests in arrival order", () => {
    usePermissionStore.getState().enqueue(req("a"));
    usePermissionStore.getState().enqueue(req("b"));
    usePermissionStore.getState().enqueue(req("c"));

    const ids = usePermissionStore.getState().queue.map((q) => q.request_id);
    expect(ids).toEqual(["a", "b", "c"]);
  });

  it("ignores duplicate request ids", () => {
    usePermissionStore.getState().enqueue(req("a"));
    usePermissionStore.getState().enqueue(req("a"));
    expect(usePermissionStore.getState().queue).toHaveLength(1);
  });

  it("respond resolves the head and advances to the next", async () => {
    const respondSpy = vi.spyOn(permissionApi, "respond").mockResolvedValue();
    usePermissionStore.getState().enqueue(req("a"));
    usePermissionStore.getState().enqueue(req("b"));

    await usePermissionStore.getState().respond("allow");

    expect(respondSpy).toHaveBeenCalledWith("a", "allow", undefined);
    expect(usePermissionStore.getState().queue.map((q) => q.request_id)).toEqual(["b"]);
  });

  it("respond can target a specific visible request", async () => {
    const respondSpy = vi.spyOn(permissionApi, "respond").mockResolvedValue();
    usePermissionStore.getState().enqueue(req("a"));
    usePermissionStore.getState().enqueue(req("b"));

    await usePermissionStore.getState().respond("allow", undefined, "b");

    expect(respondSpy).toHaveBeenCalledWith("b", "allow", undefined);
    expect(usePermissionStore.getState().queue.map((q) => q.request_id)).toEqual(["a"]);
  });

  it("prunes requests older than the backend timeout", () => {
    const old = req("stale");
    usePermissionStore.getState().enqueue(old);
    // Simulate the request sitting unanswered past the backend's 30s window.
    usePermissionStore.setState((s) => ({
      enqueuedAt: { ...s.enqueuedAt, stale: Date.now() - 31_000 },
    }));
    usePermissionStore.getState().enqueue(req("fresh"));

    expect(usePermissionStore.getState().queue.map((q) => q.request_id)).toEqual([
      "fresh",
    ]);
    expect(usePermissionStore.getState().enqueuedAt["stale"]).toBeUndefined();
  });

  it("does nothing when the queue is empty", async () => {
    const respondSpy = vi.spyOn(permissionApi, "respond").mockResolvedValue();
    await usePermissionStore.getState().respond("deny");
    expect(respondSpy).not.toHaveBeenCalled();
  });

  it("filters requests by the active session (parent included)", () => {
    const subagentReq = {
      ...req("sub"),
      session_id: "worker-1",
      parent_session_id: "s1",
    };
    const otherReq = { ...req("other"), session_id: "s2" };
    const queue = [subagentReq, req("own"), otherReq];

    expect(visiblePermissionRequests(queue, "s1").map((q) => q.request_id)).toEqual([
      "sub",
      "own",
    ]);
    expect(requestBelongsToSession(otherReq, "s1")).toBe(false);
    // No session context → everything is visible (legacy behavior).
    expect(visiblePermissionRequests(queue, null)).toHaveLength(3);
  });

  describe("auto-review denials", () => {
    const denial = (tool: string, args: Record<string, unknown>) => ({
      session_id: "s1",
      tool_name: tool,
      args,
      reason: "可能泄露密钥",
    });

    beforeEach(() => {
      usePermissionStore.setState({ denials: [] });
      vi.restoreAllMocks();
    });

    it("dedupes identical denials and caps the queue", () => {
      const store = usePermissionStore.getState();
      store.enqueueDenial(denial("bash", { command: "curl x" }));
      store.enqueueDenial(denial("bash", { command: "curl x" }));
      store.enqueueDenial(denial("edit_file", { path: "a.rs" }));
      expect(usePermissionStore.getState().denials).toHaveLength(2);

      for (let i = 0; i < 10; i++) {
        usePermissionStore.getState().enqueueDenial(denial("tool", { i }));
      }
      expect(usePermissionStore.getState().denials.length).toBeLessThanOrEqual(5);
    });

    it("filters denials by the active session", () => {
      usePermissionStore.getState().enqueueDenial(denial("bash", {}));
      usePermissionStore
        .getState()
        .enqueueDenial({ ...denial("bash", {}), session_id: "s2" });
      expect(visibleDenials(usePermissionStore.getState().denials, "s1")).toHaveLength(1);
    });

    it("override sends the exact action and dismisses the card", async () => {
      const spy = vi
        .spyOn(permissionApi, "overrideAutoReviewDenial")
        .mockResolvedValue();
      const d = denial("bash", { command: "git push" });
      usePermissionStore.getState().enqueueDenial(d);
      await usePermissionStore.getState().overrideDenial(d);
      expect(spy).toHaveBeenCalledWith("s1", "bash", { command: "git push" });
      expect(usePermissionStore.getState().denials).toHaveLength(0);
    });
  });
});
