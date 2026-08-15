/**
 * depworkStore tests — document-directory open/close semantics.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { useDepworkStore } from "@/stores/depworkStore";

const { pickFolderMock, listFilesMock } = vi.hoisted(() => ({
  pickFolderMock: vi.fn(),
  listFilesMock: vi.fn(),
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    pickFolder: pickFolderMock,
    listWorkspaceFiles: listFilesMock,
  };
});

const FILE_NODE = {
  name: "a.pdf",
  path: "D:\\docs\\a.pdf",
  isDir: false,
  size: 10,
};

describe("depworkStore folder", () => {
  beforeEach(() => {
    useDepworkStore.setState({
      rootPath: null,
      tree: [],
      treeLoading: false,
      selectedFile: null,
    });
    pickFolderMock.mockReset();
    listFilesMock.mockReset();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("openFolder returns the picked path and loads the tree", async () => {
    pickFolderMock.mockResolvedValue("D:\\docs");
    listFilesMock.mockResolvedValue([
      FILE_NODE,
      { name: "sub", path: "D:\\docs\\sub", isDir: true, size: null },
    ]);

    const path = await useDepworkStore.getState().openFolder();

    expect(path).toBe("D:\\docs");
    expect(useDepworkStore.getState().rootPath).toBe("D:\\docs");
    expect(useDepworkStore.getState().tree).toEqual([
      { ...FILE_NODE, loaded: true },
      { name: "sub", path: "D:\\docs\\sub", isDir: true, size: null, loaded: false },
    ]);
    expect(listFilesMock).toHaveBeenCalledWith("D:\\docs");
  });

  it("openFolder returns null when the picker is cancelled", async () => {
    pickFolderMock.mockResolvedValue(null);

    const path = await useDepworkStore.getState().openFolder();

    expect(path).toBeNull();
    expect(useDepworkStore.getState().rootPath).toBeNull();
    expect(listFilesMock).not.toHaveBeenCalled();
  });

  it("clearFolder resets the root, tree and preview selection", () => {
    useDepworkStore.setState({
      rootPath: "D:\\docs",
      tree: [FILE_NODE],
      selectedFile: FILE_NODE,
    });

    useDepworkStore.getState().clearFolder();

    expect(useDepworkStore.getState().rootPath).toBeNull();
    expect(useDepworkStore.getState().tree).toEqual([]);
    expect(useDepworkStore.getState().selectedFile).toBeNull();
  });
});
