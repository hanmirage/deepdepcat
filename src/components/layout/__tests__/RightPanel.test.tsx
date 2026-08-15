import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { RightPanel } from "@/components/layout/RightPanel";
import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "common.close": "关闭",
        "rightPanel.collapse": "收起面板",
        "rightPanel.paneActivity": "活动",
        "rightPanel.paneFiles": "文件",
        "rightPanel.paneBrowser": "浏览器",
        "rightPanel.paneTask": "任务",
        "rightPanel.activityBadge": "Agent 正在活动",
      })[key] ?? key,
  }),
}));

vi.mock("@/components/customize/WorkspaceFilesPanel", () => ({
  WorkspaceFilesPanel: () => <div>files-mock</div>,
}));
vi.mock("@/components/customize/AgentActivityCard", () => ({
  AgentActivityCard: () => <div>agent-mock</div>,
}));
vi.mock("@/components/customize/SubagentPanel", () => ({
  SubagentPanel: () => <div>subagents-mock</div>,
}));
vi.mock("@/components/customize/TaskPanel", () => ({
  TaskPanel: () => <div>task-mock</div>,
}));
vi.mock("@/components/depwork/DepworkTaskPanel", () => ({
  DepworkTaskPanel: () => <div>depwork-task-mock</div>,
}));
vi.mock("@/components/depwork/WorkspacePanel", () => ({
  WorkspacePanel: () => <div>workspace-mock</div>,
}));
vi.mock("@/components/depwork/DocumentContextCard", () => ({
  DocumentContextCard: () => <div>docs-mock</div>,
}));
vi.mock("@/components/preview/HtmlPreviewPane", () => ({
  HtmlPreviewPane: () => <div>preview-mock</div>,
}));
vi.mock("@/components/preview/BrowserLivePane", () => ({
  BrowserLivePane: () => <div>browser-mock</div>,
}));

class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", ResizeObserverStub);
  Element.prototype.setPointerCapture = vi.fn();
  Element.prototype.hasPointerCapture = vi.fn(() => true);
  Element.prototype.releasePointerCapture = vi.fn();
  useAppStore.setState({ mode: "code" });
  useRightPanelStore.setState({
    open: true,
    width: { code: 640, depwork: 640 },
    panes: { code: ["activity"], depwork: ["activity"] },
    pendingFile: { code: null, depwork: null },
    pendingPreview: { code: null, depwork: null },
    activitySignal: { code: false, depwork: false },
    autoOpenSuppressed: { code: false, depwork: false },
  });
  useChatStore.setState({ isStreaming: false, currentSessionId: "s1", messages: [] });
  useDepworkChatStore.setState({ isStreaming: false, currentSessionId: null, messages: [] });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("RightPanel drawer", () => {
  it("renders the active pane content and follows the header title", async () => {
    const { rerender } = render(<RightPanel />);
    expect(screen.getByRole("heading", { name: "活动" })).toBeTruthy();
    expect(screen.getByText("agent-mock")).toBeTruthy();

    useRightPanelStore.getState().openPane("code", "files");
    rerender(<RightPanel />);
    expect(screen.getByRole("heading", { name: "文件" })).toBeTruthy();
    expect(screen.getByText("files-mock")).toBeTruthy();
  });

  it("stacks two panes vertically when both are open", () => {
    useRightPanelStore.setState({
      panes: { code: ["activity", "files"], depwork: ["activity"] },
    });
    render(<RightPanel />);

    expect(screen.getByText("agent-mock")).toBeTruthy();
    expect(screen.getByText("files-mock")).toBeTruthy();
    expect(screen.getByRole("heading", { name: "文件" })).toBeTruthy();
    expect(screen.getAllByRole("button", { name: "关闭" })).toHaveLength(2);
  });

  it("renders the Depwork workspace and browser panes", () => {
    useAppStore.setState({ mode: "depwork" });
    useRightPanelStore.setState({
      panes: { code: ["activity"], depwork: ["files"] },
    });
    const { rerender } = render(<RightPanel />);
    expect(screen.getByRole("heading", { name: "文件" })).toBeTruthy();
    expect(screen.getByText("workspace-mock")).toBeTruthy();

    useRightPanelStore.getState().openPane("depwork", "browser");
    rerender(<RightPanel />);
    expect(screen.getByRole("heading", { name: "浏览器" })).toBeTruthy();
    expect(screen.getByText("browser-mock")).toBeTruthy();
  });

  it("renders the Depwork task pane with the depwork task panel", () => {
    useAppStore.setState({ mode: "depwork" });
    useRightPanelStore.setState({
      panes: { code: ["activity"], depwork: ["task"] },
    });
    render(<RightPanel />);
    expect(screen.getByRole("heading", { name: "任务" })).toBeTruthy();
    expect(screen.getByText("depwork-task-mock")).toBeTruthy();
  });

  it("dismiss closes the drawer and suppresses auto-open", async () => {
    render(<RightPanel />);
    await userEvent.click(screen.getByRole("button", { name: "收起面板" }));

    const s = useRightPanelStore.getState();
    expect(s.open).toBe(false);
    expect(s.autoOpenSuppressed.code).toBe(true);
  });

  it("viewing the activity pane consumes the activity signal", () => {
    useRightPanelStore.setState({ activitySignal: { code: true, depwork: false } });
    render(<RightPanel />);
    expect(useRightPanelStore.getState().activitySignal.code).toBe(false);
  });

  it("double-clicking the resize handle restores the default width", () => {
    const { container } = render(<RightPanel />);
    const handle = container.querySelector(".cursor-col-resize") as HTMLElement;
    fireEvent.doubleClick(handle);
    expect(useRightPanelStore.getState().width.code).toBe(300);
  });
});
