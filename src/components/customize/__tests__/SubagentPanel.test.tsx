/**
 * SubagentPanel tests — renders the active mode's subagent cards.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { SubagentPanel } from "@/components/customize/SubagentPanel";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, opts?: Record<string, unknown>) => {
      if (key === "subagents.turn") {
        return `回合 ${opts?.turn}/${opts?.total}`;
      }
      return (
        {
          "subagents.empty": "暂无调度的子代理",
          "subagents.running": "运行中",
          "subagents.done": "已完成",
          "subagents.failed": "失败",
          "activity.untitledTask": "未命名任务",
          "chat.specialistBadge": "专家",
          "toolCall.agentTypeGeneral": "通用",
        }[key] ?? key
      );
    },
  }),
}));

const S1 = {
  subagent_id: "s1",
  task: "first agent",
  agent_type: "general",
  tool_call_id: "tc1",
  status: "running" as const,
  turn: 1,
  total_turns: 3,
  lastMessage: "working",
  result: "",
  startedAt: 1000,
};

const S2 = {
  subagent_id: "s2",
  task: "second agent",
  agent_type: "explore",
  tool_call_id: "tc2",
  status: "done" as const,
  turn: 2,
  total_turns: 2,
  lastMessage: "",
  result: "all done",
  startedAt: 2000,
};

beforeEach(() => {
  useChatStore.setState({ subagents: { s1: S1, s2: S2 } });
  useDepworkChatStore.setState({ subagents: {} });
});

describe("SubagentPanel", () => {
  it("renders one card per subagent of the active mode", () => {
    render(<SubagentPanel isDepwork={false} />);
    expect(screen.getByText("first agent")).toBeTruthy();
    expect(screen.getByText("second agent")).toBeTruthy();
  });

  it("shows the empty hint when the active mode has no subagents", () => {
    render(<SubagentPanel isDepwork />);
    expect(screen.getByText("暂无调度的子代理")).toBeTruthy();
  });
});
