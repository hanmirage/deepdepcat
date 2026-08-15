/**
 * PlanSummaryPanel tests — plan-mode status, retained plan MD, pending
 * interactions, empty hint.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { PlanSummaryPanel } from "@/components/customize/PlanSummaryPanel";
import { usePlanStore } from "@/stores/planStore";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "rightPanel.planModeActive": "计划模式",
        "rightPanel.planModeActiveDesc": "Agent 正在起草方案，执行前需要你批准",
        "rightPanel.planModeIdle": "未处于计划模式",
        "rightPanel.planWaitingApproval": "聊天流中有方案等待你决策",
        "rightPanel.planInteractions": "等待你处理",
        "rightPanel.planView": "计划内容",
        "rightPanel.planEmpty": "暂无待处理的计划或审批",
      })[key] ?? key,
  }),
}));

vi.mock("@/components/chat/MarkdownRenderer", () => ({
  MarkdownRenderer: ({ content }: { content: string }) => <div>md:{content}</div>,
}));

beforeEach(() => {
  usePlanStore.setState({
    planModeSessions: {},
    interactions: {},
    approval: null,
    currentPlan: null,
  });
});

describe("PlanSummaryPanel", () => {
  it("shows the plan-mode status when the session is in plan mode", () => {
    usePlanStore.setState({ planModeSessions: { s1: true } });
    render(<PlanSummaryPanel sessionId="s1" />);
    expect(screen.getByText("计划模式")).toBeTruthy();
  });

  it("renders the retained plan MD for the active session", () => {
    usePlanStore.setState({
      currentPlan: { sessionId: "s1", plan: "# 修复 token 校验" },
    });
    render(<PlanSummaryPanel sessionId="s1" />);
    expect(screen.getByText("md:# 修复 token 校验")).toBeTruthy();
    expect(screen.getByText("计划内容")).toBeTruthy();
  });

  it("does not show another session's retained plan", () => {
    usePlanStore.setState({
      currentPlan: { sessionId: "other", plan: "# 别的计划" },
    });
    render(<PlanSummaryPanel sessionId="s1" />);
    expect(screen.queryByText("md:# 别的计划")).toBeNull();
  });

  it("lists the session's pending interactions", () => {
    usePlanStore.setState({
      interactions: {
        s1: [{ kind: "permission", request_id: "r1", summary: "运行 npm test", since: 0 }],
      },
    });
    render(<PlanSummaryPanel sessionId="s1" />);
    expect(screen.getByText("运行 npm test")).toBeTruthy();
  });

  it("does not show another session's interactions", () => {
    usePlanStore.setState({
      interactions: {
        other: [{ kind: "permission", request_id: "r2", summary: "别的请求", since: 0 }],
      },
    });
    render(<PlanSummaryPanel sessionId="s1" />);
    expect(screen.queryByText("别的请求")).toBeNull();
  });

  it("shows an empty hint when nothing is pending", () => {
    render(<PlanSummaryPanel sessionId="s1" />);
    expect(screen.getByText("暂无待处理的计划或审批")).toBeTruthy();
  });
});
