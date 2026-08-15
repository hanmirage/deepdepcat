/**
 * SessionList tests — recency grouping + inline rename.
 */

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { SessionList } from "@/components/sidebar/SessionList";
import type { Session } from "@/types";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "sidebar.today": "今天",
        "sidebar.yesterday": "昨天",
        "sidebar.week": "本周",
        "sidebar.earlier": "更早",
        "sidebar.pinned": "置顶",
        "sidebar.pin": "置顶",
        "sidebar.unpin": "取消置顶",
        "sidebar.rename": "重命名",
        "sidebar.noMatch": "未找到匹配的会话",
        "sidebar.noSessionYet": "暂无会话",
        "sidebar.deleteFailed": "删除失败",
        "sidebar.untitledSession": "未命名会话",
        "sidebar.turns": "{{count}} 轮",
      })[key] ?? key,
  }),
}));

function session(
  id: string,
  updatedAt: string,
  title = `Session ${id}`,
  pinned = false,
  lastMessage = "",
): Session {
  return {
    id,
    title,
    model: "deepseek-v4-flash",
    provider: "deepseek",
    created_at: updatedAt,
    updated_at: updatedAt,
    status: "active",
    turn_count: 3,
    pinned,
    last_message: lastMessage,
  } as Session;
}

function daysAgo(days: number): string {
  const now = new Date();
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  // Local-midnight anchored + 10:00 — immune to the test runner's timezone.
  return new Date(todayStart.getTime() - days * 86_400_000 + 10 * 3_600_000).toISOString();
}

describe("SessionList", () => {
  it("groups sessions by recency with headers", () => {
    render(
      <SessionList
        sessions={[
          session("s-today", daysAgo(0)),
          session("s-yest", daysAgo(1)),
          session("s-week", daysAgo(4)),
          session("s-old", daysAgo(30)),
        ]}
        loading={false}
        error={null}
        activeSessionId={null}
        onSelect={vi.fn()}
        onDelete={vi.fn()}
        onRename={vi.fn()}
      />,
    );
    expect(screen.getByText("今天")).toBeInTheDocument();
    expect(screen.getByText("昨天")).toBeInTheDocument();
    expect(screen.getByText("本周")).toBeInTheDocument();
    expect(screen.getByText("更早")).toBeInTheDocument();
    expect(screen.getAllByText("Session s-today").length).toBe(1);
  });

  it("rename: pencil reveals inline input, Enter saves trimmed title", async () => {
    const onRename = vi.fn().mockResolvedValue(undefined);
    render(
      <SessionList
        sessions={[session("s1", daysAgo(0), "Old title")]}
        loading={false}
        error={null}
        activeSessionId={null}
        onSelect={vi.fn()}
        onDelete={vi.fn()}
        onRename={onRename}
      />,
    );
    fireEvent.click(screen.getByLabelText("重命名"));
    const input = screen.getByDisplayValue("Old title");
    fireEvent.change(input, { target: { value: "  New title  " } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onRename).toHaveBeenCalledWith("s1", "New title");
  });

  it("rename: Escape cancels without saving", () => {
    const onRename = vi.fn();
    render(
      <SessionList
        sessions={[session("s1", daysAgo(0), "Old title")]}
        loading={false}
        error={null}
        activeSessionId={null}
        onSelect={vi.fn()}
        onDelete={vi.fn()}
        onRename={onRename}
      />,
    );
    fireEvent.click(screen.getByLabelText("重命名"));
    fireEvent.keyDown(screen.getByDisplayValue("Old title"), { key: "Escape" });
    expect(onRename).not.toHaveBeenCalled();
  });

  it("empty + searching shows the no-match state", () => {
    render(
      <SessionList
        sessions={[]}
        loading={false}
        error={null}
        activeSessionId={null}
        onSelect={vi.fn()}
        onDelete={vi.fn()}
        onRename={vi.fn()}
        isSearching
      />,
    );
    expect(screen.getByText("未找到匹配的会话")).toBeInTheDocument();
  });

  it("shows the project badge for workspace-bound sessions", () => {
    const s = session("s1", daysAgo(0), "Title");
    s.workspace_path = "D:\\proj\\my-app";
    render(
      <SessionList
        sessions={[s]}
        loading={false}
        error={null}
        activeSessionId={null}
        onSelect={vi.fn()}
        onDelete={vi.fn()}
        onRename={vi.fn()}
      />,
    );
    expect(screen.getByText("my-app")).toBeInTheDocument();
  });

  it("pinned sessions get their own top group, not a recency group", () => {
    render(
      <SessionList
        sessions={[
          session("s-pinned", daysAgo(0), "Pinned one", true),
          session("s-today", daysAgo(0)),
        ]}
        loading={false}
        error={null}
        activeSessionId={null}
        onSelect={vi.fn()}
        onDelete={vi.fn()}
        onRename={vi.fn()}
        onTogglePin={vi.fn()}
      />,
    );
    // Pinned header + the non-pinned session still in its recency group.
    expect(screen.getByText("置顶")).toBeInTheDocument();
    expect(screen.getByText("今天")).toBeInTheDocument();
    expect(screen.getByText("Pinned one")).toBeInTheDocument();
    // The pinned session must not also appear under "今天".
    expect(screen.getAllByText("Session s-today").length).toBe(1);
  });

  it("pin button toggles the session's pinned state", () => {
    const onTogglePin = vi.fn();
    render(
      <SessionList
        sessions={[session("s1", daysAgo(0), "A", true)]}
        loading={false}
        error={null}
        activeSessionId={null}
        onSelect={vi.fn()}
        onDelete={vi.fn()}
        onRename={vi.fn()}
        onTogglePin={onTogglePin}
      />,
    );
    fireEvent.click(screen.getByLabelText("取消置顶"));
    expect(onTogglePin).toHaveBeenCalledWith("s1");
  });

  it("shows the last-message preview under the title", () => {
    render(
      <SessionList
        sessions={[
          session("s1", daysAgo(0), "With preview", false, "这是最后一条消息"),
          session("s2", daysAgo(0), "No preview"),
        ]}
        loading={false}
        error={null}
        activeSessionId={null}
        onSelect={vi.fn()}
        onDelete={vi.fn()}
        onRename={vi.fn()}
      />,
    );
    expect(screen.getByText("这是最后一条消息")).toBeInTheDocument();
  });
});
