/**
 * WorkspaceSelector tests — current-project dropdown in the Code sidebar.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { WorkspaceSelector } from "@/components/sidebar/WorkspaceSelector";
import { useAppStore } from "@/stores/appStore";

const { pickFolderMock, listFilesMock, setWorkspaceMock } = vi.hoisted(() => ({
  pickFolderMock: vi.fn(),
  listFilesMock: vi.fn(),
  setWorkspaceMock: vi.fn(),
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    pickFolder: pickFolderMock,
    listWorkspaceFiles: listFilesMock,
    systemApi: {
      ...actual.systemApi,
      setWorkspace: setWorkspaceMock,
    },
  };
});

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "sidebar.openProject": "浏览并打开项目…",
        "sidebar.closeProject": "关闭项目",
        "sidebar.noProject": "尚未打开项目",
        "sidebar.sectionProjects": "项目",
        "sidebar.removeWorkspace": "移除项目",
        "sidebar.confirmRemoveWorkspace": "再次点击确认移除",
      })[key] ?? key,
  }),
}));

beforeEach(() => {
  useAppStore.setState({
    mode: "code",
    workspacePath: null,
    workspaces: [],
  });
  pickFolderMock.mockReset().mockResolvedValue(null);
  listFilesMock.mockReset().mockResolvedValue([]);
  setWorkspaceMock.mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("WorkspaceSelector", () => {
  it("switches the current project from the dropdown", async () => {
    useAppStore.setState({
      workspacePath: "D:\\proj-a",
      workspaces: ["D:\\proj-a", "D:\\proj-b"],
    });
    const user = userEvent.setup();
    render(<WorkspaceSelector />);

    expect(screen.getByText("proj-a")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "浏览并打开项目…" }));
    await user.click(await screen.findByText("proj-b"));

    await waitFor(() => {
      expect(useAppStore.getState().workspacePath).toBe("D:\\proj-b");
    });
    expect(setWorkspaceMock).toHaveBeenCalledWith("D:\\proj-b");
  });

  it("removes a workspace from the list (two-step confirm)", async () => {
    useAppStore.setState({
      workspacePath: "D:\\proj-a",
      workspaces: ["D:\\proj-a", "D:\\proj-b"],
    });
    const user = userEvent.setup();
    render(<WorkspaceSelector />);

    await user.click(screen.getByRole("button", { name: "浏览并打开项目…" }));
    const removeButtons = await screen.findAllByLabelText("移除项目");
    // First click arms the confirm; a single click must NOT remove.
    await user.click(removeButtons[1]);
    expect(useAppStore.getState().workspaces).toEqual(["D:\\proj-a", "D:\\proj-b"]);
    // Second click (on the re-labeled confirm button) removes.
    const confirmButtons = await screen.findAllByLabelText("再次点击确认移除");
    await user.click(confirmButtons[0]);

    expect(useAppStore.getState().workspaces).toEqual(["D:\\proj-a"]);
  });

  it("opens a new project via the browse action", async () => {
    pickFolderMock.mockResolvedValue("D:\\proj-new");
    const user = userEvent.setup();
    render(<WorkspaceSelector />);

    await user.click(screen.getByRole("button", { name: "浏览并打开项目…" }));
    await user.click(await screen.findByText("浏览并打开项目…"));

    await waitFor(() => {
      expect(useAppStore.getState().workspacePath).toBe("D:\\proj-new");
    });
    expect(useAppStore.getState().workspaces).toContain("D:\\proj-new");
  });

  it("closes the current project", async () => {
    useAppStore.setState({
      workspacePath: "D:\\proj-a",
      workspaces: ["D:\\proj-a"],
    });
    const user = userEvent.setup();
    render(<WorkspaceSelector />);

    await user.click(screen.getByRole("button", { name: "浏览并打开项目…" }));
    await user.click(await screen.findByText("关闭项目"));

    expect(useAppStore.getState().workspacePath).toBeNull();
  });
});
