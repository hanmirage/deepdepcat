/**
 * SubagentCard tests — running cards show live progress; done cards
 * auto-collapse to a header and expand to reveal the result.
 */

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { SubagentCard } from "@/components/customize/SubagentCard";
import type { SubagentUIRecord } from "@/types";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, opts?: Record<string, unknown>) => {
      if (key === "subagents.turn") {
        return `回合 ${opts?.turn}/${opts?.total}`;
      }
      return (
        {
          "subagents.running": "运行中",
          "subagents.done": "已完成",
          "subagents.failed": "失败",
          "subagents.viewResult": "查看结果",
          "subagents.hideResult": "收起结果",
          "activity.untitledTask": "未命名任务",
          "chat.specialistBadge": "专家",
          "toolCall.agentTypeExplore": "探查",
          "toolCall.agentTypeGeneral": "通用",
        }[key] ?? key
      );
    },
  }),
}));

function record(overrides: Partial<SubagentUIRecord>): SubagentUIRecord {
  return {
    subagent_id: "s1",
    task: "search the codebase",
    agent_type: "general",
    tool_call_id: "tc1",
    status: "running",
    turn: 1,
    total_turns: 0,
    lastMessage: "",
    result: "",
    startedAt: Date.now(),
    ...overrides,
  };
}

describe("SubagentCard", () => {
  it("shows live progress for a running subagent", () => {
    render(
      <SubagentCard
        subagent={record({
          agent_type: "explore",
          status: "running",
          turn: 2,
          total_turns: 5,
          lastMessage: "checking src/auth/mod.rs",
        })}
      />,
    );
    expect(screen.getByText("search the codebase")).toBeTruthy();
    expect(screen.getByText("探查")).toBeTruthy();
    expect(screen.getByText("运行中")).toBeTruthy();
    expect(screen.getByText("回合 2/5")).toBeTruthy();
    expect(screen.getByText("checking src/auth/mod.rs")).toBeTruthy();
  });

  it("collapses a done subagent to a header line, expanding on demand", () => {
    render(
      <SubagentCard
        subagent={record({
          status: "done",
          result: "found 3 references",
        })}
      />,
    );
    expect(screen.getByText("已完成")).toBeTruthy();
    // Collapsed: no result visible yet.
    expect(screen.queryByText("found 3 references")).toBeNull();

    fireEvent.click(screen.getByLabelText("查看结果"));
    expect(screen.getByText("found 3 references")).toBeTruthy();
    expect(screen.getByLabelText("收起结果")).toBeTruthy();
  });
});
