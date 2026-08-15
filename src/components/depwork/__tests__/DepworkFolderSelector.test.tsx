/**
 * DepworkFolderSelector tests — binds to the depwork document directory
 * (depworkStore), not the Code workspace. The directory is surfaced by the
 * picker label + store only — it is NOT attached as an input context chip
 * (sendMessage injects rootPath per turn instead).
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DepworkFolderSelector } from "@/components/depwork/DepworkFolderSelector";
import { useDepworkStore } from "@/stores/depworkStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";

const { pickFolderMock, listFilesMock } = vi.hoisted(() => ({
  pickFolderMock: vi.fn(),
  listFilesMock: vi.fn(),
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    isTauri: true,
    pickFolder: pickFolderMock,
    listWorkspaceFiles: listFilesMock,
  };
});

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "depwork.selectDocumentDir": "选择文档目录",
        "depwork.browseDocumentDir": "浏览并打开文档目录…",
        "depwork.currentDocumentDir": "当前文档目录",
        "depwork.closeDocumentDir": "关闭文档目录",
      })[key] ?? key,
  }),
}));

beforeEach(() => {
  useDepworkStore.setState({
    rootPath: null,
    tree: [],
    treeLoading: false,
    selectedFile: null,
  });
  useDepworkChatStore.setState({ contextChips: [] });
  pickFolderMock.mockReset();
  listFilesMock.mockReset().mockResolvedValue([]);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("DepworkFolderSelector", () => {
  it("opens a document directory into the depwork store", async () => {
    pickFolderMock.mockResolvedValue("D:\\docs");
    const user = userEvent.setup();
    render(<DepworkFolderSelector />);

    expect(screen.getByText("选择文档目录")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "选择文档目录" }));
    await user.click(await screen.findByText("浏览并打开文档目录…"));

    await waitFor(() => {
      expect(useDepworkStore.getState().rootPath).toBe("D:\\docs");
    });
    expect(screen.getByText("docs")).toBeInTheDocument();
    expect(listFilesMock).toHaveBeenCalledWith("D:\\docs");
    // No input-chip is attached — the picker button itself shows the folder.
    expect(useDepworkChatStore.getState().contextChips).toEqual([]);
  });

  it("closing the folder clears the store", async () => {
    useDepworkStore.setState({
      rootPath: "D:\\docs",
      tree: [],
      treeLoading: false,
      selectedFile: null,
    });
    useDepworkChatStore.setState({ contextChips: [] });
    const user = userEvent.setup();
    render(<DepworkFolderSelector />);

    await user.click(screen.getByRole("button", { name: "选择文档目录" }));
    await user.click(await screen.findByText("关闭文档目录"));

    expect(useDepworkStore.getState().rootPath).toBeNull();
    expect(useDepworkChatStore.getState().contextChips).toEqual([]);
  });

  it("opening another directory replaces the previous one", async () => {
    useDepworkStore.setState({
      rootPath: "D:\\old",
      tree: [],
      treeLoading: false,
      selectedFile: null,
    });
    useDepworkChatStore.setState({ contextChips: [] });
    pickFolderMock.mockResolvedValue("D:\\new");
    const user = userEvent.setup();
    render(<DepworkFolderSelector />);

    await user.click(screen.getByRole("button", { name: "选择文档目录" }));
    await user.click(await screen.findByText("浏览并打开文档目录…"));

    await waitFor(() => {
      expect(useDepworkStore.getState().rootPath).toBe("D:\\new");
    });
    expect(useDepworkChatStore.getState().contextChips).toEqual([]);
  });
});
