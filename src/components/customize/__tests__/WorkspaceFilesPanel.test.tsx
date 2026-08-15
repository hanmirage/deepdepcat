/**
 * WorkspaceFilesPanel tests — file list, preview, and desktop-only gate.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { WorkspaceFilesPanel } from "@/components/customize/WorkspaceFilesPanel";
import { useAppStore } from "@/stores/appStore";

const readTextMock = vi.fn<() => Promise<string>>();

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    isTauri: true,
    toAssetUrl: (path: string) => `asset://localhost/${encodeURIComponent(path)}`,
    readWorkspaceTextFile: () => readTextMock(),
    readWorkspaceBinaryFile: () => Promise.resolve(new Uint8Array(0)),
  };
});

vi.mock("@/components/chat/MarkdownRenderer", () => ({
  MarkdownRenderer: ({ content }: { content: string }) => <div>{content}</div>,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "rightPanel.artifactsWorkspace": "工作区文件",
        "rightPanel.artifactsWorkspaceEmpty": "未打开工作区",
        "rightPanel.artifactsWorkspaceOpenHint": "在侧栏打开项目",
        "rightPanel.artifactsWorkspaceFilesEmpty": "工作区为空",
        "rightPanel.artifactsSelectFile": "点击文件预览内容",
        "rightPanel.artifactsPreviewUnsupported": "此文件类型暂不支持预览",
        "rightPanel.artifactsTruncated": "文件过大",
        "rightPanel.filesDesktopOnly": "文件浏览仅在桌面端可用",
        "common.refresh": "刷新",
      })[key] ?? key,
  }),
}));

beforeEach(() => {
  useAppStore.setState({
    workspacePath: "D:\\proj",
    workspaceFiles: [
      { name: "readme.md", path: "D:\\proj\\readme.md", isDir: false, size: 10 },
      { name: "src", path: "D:\\proj\\src", isDir: true, size: null },
    ],
    workspaceLoading: false,
  });
  readTextMock.mockReset().mockResolvedValue("# Hello");
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("WorkspaceFilesPanel", () => {
  it("previews a workspace file on click", async () => {
    const user = userEvent.setup();
    render(<WorkspaceFilesPanel />);

    await user.click(screen.getByText("readme.md"));

    await waitFor(() => {
      expect(screen.getByText("# Hello")).toBeInTheDocument();
    });
    expect(readTextMock).toHaveBeenCalledTimes(1);
  });

  it("shows the open-workspace hint when no project is open", () => {
    useAppStore.setState({ workspacePath: null, workspaceFiles: [] });
    render(<WorkspaceFilesPanel />);

    expect(screen.getByText("未打开工作区")).toBeInTheDocument();
    expect(screen.getByText("在侧栏打开项目")).toBeInTheDocument();
  });
});
