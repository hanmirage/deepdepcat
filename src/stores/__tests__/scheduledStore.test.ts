import { describe, expect, it } from "vitest";
import { vi } from "vitest";
import { describeSchedule, useScheduledStore } from "@/stores/scheduledStore";
import type { ScheduledRun, ScheduledTask } from "@/types/scheduled";

vi.mock("@/lib/tauri/api/automation", () => ({
  automationApi: {
    listTasks: vi.fn(async () => []),
    listRuns: vi.fn(async () => []),
    createTask: vi.fn(),
    updateTask: vi.fn(),
    deleteTask: vi.fn(async () => {}),
    runNow: vi.fn(),
    cancelRun: vi.fn(),
    deleteRun: vi.fn(),
    cleanupWorktree: vi.fn(),
  },
}));

function task(id: string): ScheduledTask {
  return {
    id,
    name: "task",
    prompt: "do it",
    schedule: { kind: "interval", every_secs: 3600 },
    project_path: "",
    use_worktree: false,
    persistent: false,
    persistent_session_id: null,
    work_mode: "code",
    model: "",
    active: true,
    last_run_at_ms: null,
    run_count: 0,
    created_at: new Date().toISOString(),
    updated_at: new Date().toISOString(),
  };
}

function run(id: string, taskId: string): ScheduledRun {
  return {
    id,
    task_id: taskId,
    session_id: null,
    status: "running",
    started_at: new Date().toISOString(),
    finished_at: null,
    summary: "",
    error: "",
    worktree_path: "",
  };
}

describe("describeSchedule", () => {
  it("formats intervals in minutes and hours", () => {
    expect(describeSchedule({ kind: "interval", every_secs: 300 })).toBe("每 5 分钟");
    expect(describeSchedule({ kind: "interval", every_secs: 7200 })).toBe("每 2 小时");
  });

  it("formats daily time", () => {
    expect(describeSchedule({ kind: "daily", time: "09:30" })).toBe("每天 09:30");
  });
});

describe("scheduledStore", () => {
  it("upserts tasks and runs and respects the run filter", async () => {
    const store = useScheduledStore.getState();
    store.upsertTask(task("t1"));
    store.upsertTask(task("t2"));
    expect(useScheduledStore.getState().tasks).toHaveLength(2);

    useScheduledStore.setState({ runsTaskId: "t1" });
    useScheduledStore.getState().upsertRun(run("r1", "t1"));
    useScheduledStore.getState().upsertRun(run("r2", "t2"));
    // t2's run is filtered out while the inbox is pinned to t1.
    expect(useScheduledStore.getState().runs.map((r) => r.id)).toEqual(["r1"]);

    // Unpin → both runs visible.
    useScheduledStore.setState({ runsTaskId: null });
    useScheduledStore.getState().upsertRun(run("r2", "t2"));
    expect(useScheduledStore.getState().runs.map((r) => r.id).sort()).toEqual(["r1", "r2"]);

    // Deleting a task removes its runs and unpins the filter.
    await useScheduledStore.getState().remove("t1");
    expect(useScheduledStore.getState().tasks.map((t) => t.id)).toEqual(["t2"]);
    expect(useScheduledStore.getState().runs.map((r) => r.id)).toEqual(["r2"]);
    expect(useScheduledStore.getState().runsTaskId).toBeNull();
  });
});
