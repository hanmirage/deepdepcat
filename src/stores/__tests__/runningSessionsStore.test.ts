/**
 * Running-sessions store tests — background turn registry mirror.
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { useRunningSessionsStore } from "@/stores/runningSessionsStore";

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    runningSessionsApi: {
      list: vi.fn().mockResolvedValue([]),
    },
  };
});

const TURN = {
  session_id: "s1",
  turn_id: "t1",
  started_at_ms: 1_000,
  message_preview: "refactor auth",
  work_mode: "code" as const,
  status: "running" as const,
};

describe("running sessions store", () => {
  beforeEach(() => {
    useRunningSessionsStore.setState({ running: [], completed: [] });
  });

  it("pushCompleted removes the turn from running and records completion", () => {
    useRunningSessionsStore.setState({ running: [TURN] });
    useRunningSessionsStore
      .getState()
      .pushCompleted({ session_id: "s1", turn_id: "t1", status: "completed" });

    const s = useRunningSessionsStore.getState();
    expect(s.running).toEqual([]);
    expect(s.completed).toHaveLength(1);
    expect(s.completed[0]).toMatchObject({
      session_id: "s1",
      turn_id: "t1",
      status: "completed",
    });
  });

  it("refresh replaces the running list from the backend", async () => {
    const { runningSessionsApi } = await import("@/lib/tauri");
    vi.mocked(runningSessionsApi.list).mockResolvedValue([TURN]);
    await useRunningSessionsStore.getState().refresh();

    expect(useRunningSessionsStore.getState().running).toEqual([TURN]);
  });

  it("clear empties both lists", () => {
    useRunningSessionsStore.setState({
      running: [TURN],
      completed: [{ session_id: "s1", turn_id: "t1", status: "done", completed_at_ms: 1 }],
    });
    useRunningSessionsStore.getState().clear();

    const s = useRunningSessionsStore.getState();
    expect(s.running).toEqual([]);
    expect(s.completed).toEqual([]);
  });
});
