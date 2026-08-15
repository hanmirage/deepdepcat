/**
 * TaskPanel tests — code-mode task pane: goal + todo tree + progress.
 *
 * Covers the render contract:
 *  - empty hint when the session has no plan
 *  - three status styles (completed struck, in_progress pulsed, pending)
 *  - progress counter and overflow collapse
 *  - live store updates re-render
 *  - session goal on top + high-priority dot
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { TaskPanel } from "@/components/customize/TaskPanel";
import { useTodoStore } from "@/stores/todoStore";
import { sessionApi, type TodoItem } from "@/lib/tauri";

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    isTauri: false,
    onEvent: vi.fn(async () => () => {}),
    sessionApi: {
      ...actual.sessionApi,
      getSessionTodos: vi.fn(async () => []),
      getGoal: vi.fn(async () => null),
    },
  };
});

vi.mock("react-i18next", () => ({
  initReactI18next: { type: "3rdParty", init: () => {} },
  useTranslation: () => ({
    t: (key: string, opts?: { defaultValue?: string; count?: number }) =>
      opts?.defaultValue
        ? opts.defaultValue.replace("{{count}}", String(opts.count ?? ""))
        : key,
  }),
}));

function items(): TodoItem[] {
  return [
    { id: "a", content: "完成的任务", status: "completed" },
    { id: "b", content: "进行中的任务", status: "in_progress" },
    { id: "c", content: "待办任务", status: "pending" },
  ];
}

describe("TaskPanel", () => {
  beforeEach(() => {
    useTodoStore.setState({ bySession: {} });
    (sessionApi.getGoal as ReturnType<typeof vi.fn>).mockResolvedValue(null);
  });

  it("shows an empty hint when the session has no plan or goal", () => {
    render(<TaskPanel sessionId="s1" />);
    expect(screen.getByText("task.empty")).toBeInTheDocument();
  });

  it("renders the tree with the three status styles", () => {
    useTodoStore.getState().setSessionTodos("s1", items());
    render(<TaskPanel sessionId="s1" />);

    const done = screen.getByText("完成的任务");
    expect(done.className).toContain("line-through");

    const active = screen.getByText("进行中的任务");
    expect(active.closest("li")?.querySelector(".animate-pulse")).not.toBeNull();

    const pending = screen.getByText("待办任务");
    expect(pending.className).not.toContain("line-through");

    // Progress counter 1/3.
    expect(screen.getByText("1/3")).toBeInTheDocument();
  });

  it("collapses overflow beyond maxRows into a count", () => {
    const many: TodoItem[] = Array.from({ length: 10 }, (_, i) => ({
      id: `t${i}`,
      content: `任务 ${i}`,
      status: "pending",
    }));
    useTodoStore.getState().setSessionTodos("s1", many);
    render(<TaskPanel sessionId="s1" maxRows={4} />);

    expect(screen.getByText("任务 0")).toBeInTheDocument();
    expect(screen.queryByText("任务 4")).toBeNull();
    expect(screen.getByText(/还有 6 项/)).toBeInTheDocument();
  });

  it("re-renders on live store updates", async () => {
    useTodoStore.getState().setSessionTodos("s1", items());
    render(<TaskPanel sessionId="s1" />);
    expect(screen.getByText("1/3")).toBeInTheDocument();

    useTodoStore.getState().setSessionTodos("s1", [
      { id: "a", content: "完成的任务", status: "completed" },
      { id: "b", content: "进行中的任务", status: "completed" },
      { id: "c", content: "待办任务", status: "completed" },
    ]);
    await waitFor(() => expect(screen.getByText("3/3")).toBeInTheDocument());
  });

  it("ignores other sessions' todos", () => {
    useTodoStore.getState().setSessionTodos("other", items());
    render(<TaskPanel sessionId="s1" />);
    expect(screen.getByText("task.empty")).toBeInTheDocument();
  });

  it("shows the session goal on top", async () => {
    (sessionApi.getGoal as ReturnType<typeof vi.fn>).mockResolvedValue("修复登录模块 token 校验");
    useTodoStore.getState().setSessionTodos("s1", items());
    render(<TaskPanel sessionId="s1" />);

    await waitFor(() =>
      expect(screen.getByText("修复登录模块 token 校验")).toBeInTheDocument(),
    );
  });

  it("marks high-priority items with a dot", () => {
    useTodoStore.getState().setSessionTodos("s1", [
      { id: "p", content: "高优先级", status: "pending", priority: "high" },
    ]);
    render(<TaskPanel sessionId="s1" />);
    const dot = screen.getByText("高优先级").closest("li")?.querySelector(".bg-red-500");
    expect(dot).not.toBeNull();
  });

  it("renders verify and unmet-dependency markers", () => {
    useTodoStore.getState().setSessionTodos("s1", [
      { id: "a", content: "地基", status: "completed" },
      { id: "b", content: "碰撞", status: "in_progress", depends_on: ["a"], verify: "cargo test" },
      { id: "c", content: "计分", status: "pending", depends_on: ["b"] },
    ]);
    render(<TaskPanel sessionId="s1" />);

    // A step with a verify field surfaces the command under its title.
    expect(screen.getByText("verify: cargo test")).toBeInTheDocument();

    // "计分" depends on "碰撞" (in_progress, not completed) → waiting marker.
    const c = screen.getByText("计分");
    expect(c.closest("li")?.textContent).toContain("等待 b");

    // "碰撞"'s dependency "地基" is completed → no waiting marker on it.
    const b = screen.getByText("碰撞");
    expect(b.closest("li")?.textContent).not.toContain("等待");
  });
});
