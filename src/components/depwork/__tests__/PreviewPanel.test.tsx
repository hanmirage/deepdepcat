/**
 * PreviewPanel tests — CSV preview + external open/reveal actions.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { PreviewPanel } from "@/components/depwork/PreviewPanel";
import { useDepworkStore } from "@/stores/depworkStore";
import { workspaceFileApi } from "@/lib/tauri";

const readWorkspaceTextFileMock = vi.fn<(path: string) => Promise<string>>();
const readWorkspaceBinaryFileMock = vi.fn<(path: string) => Promise<Uint8Array | null>>();

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    isTauri: true,
    toAssetUrl: (path: string) => `asset://localhost/${encodeURIComponent(path)}`,
    readWorkspaceTextFile: (path: string) => readWorkspaceTextFileMock(path),
    readWorkspaceBinaryFile: (path: string) => readWorkspaceBinaryFileMock(path),
    pdfApi: {
      extractText: vi.fn(),
    },
    workspaceFileApi: {
      open: vi.fn(),
      reveal: vi.fn(),
    },
  };
});

vi.mock("@/components/chat/MarkdownRenderer", () => ({
  MarkdownRenderer: ({ content }: { content: string }) => <div>{content}</div>,
}));

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, opts?: { defaultValue?: string }) => opts?.defaultValue ?? key,
  }),
  initReactI18next: { type: "3rdParty", init: () => {} },
}));

const CSV_FILE = {
  name: "data.csv",
  path: "C:\\ws\\data.csv",
  isDir: false,
  size: 42,
};

beforeEach(() => {
  useDepworkStore.setState({ selectedFile: CSV_FILE });
  readWorkspaceTextFileMock.mockReset();
  readWorkspaceBinaryFileMock.mockReset();
  vi.mocked(workspaceFileApi.open).mockResolvedValue(undefined);
  vi.mocked(workspaceFileApi.reveal).mockResolvedValue(undefined);
});

afterEach(() => {
  useDepworkStore.setState({ selectedFile: null });
  vi.restoreAllMocks();
});

describe("PreviewPanel", () => {
  it("renders a CSV file as a table", async () => {
    readWorkspaceBinaryFileMock.mockResolvedValue(
      new TextEncoder().encode("name,score\nAlice,95\n"),
    );
    render(<PreviewPanel />);

    expect(await screen.findByText("Alice")).toBeTruthy();
    expect(screen.getByText("95")).toBeTruthy();
  });

  it("opens the file externally", async () => {
    render(<PreviewPanel />);

    const openButton = screen.getByRole("button", { name: "depwork.previewOpenExternal" });
    fireEvent.click(openButton);
    await waitFor(() => {
      expect(workspaceFileApi.open).toHaveBeenCalledWith("C:\\ws\\data.csv");
    });
  });

  it("reveals the file in the folder", async () => {
    render(<PreviewPanel />);

    const revealButton = screen.getByRole("button", { name: "depwork.previewRevealInFolder" });
    fireEvent.click(revealButton);
    await waitFor(() => {
      expect(workspaceFileApi.reveal).toHaveBeenCalledWith("C:\\ws\\data.csv");
    });
  });

  it("opens the full-screen preview overlay on expand", async () => {
    render(<PreviewPanel />);

    const expandButton = screen.getByRole("button", { name: "depwork.previewExpand" });
    fireEvent.click(expandButton);

    // The overlay dialog is open and shows the file name in its title.
    await waitFor(() => {
      expect(screen.getByRole("dialog")).toBeInTheDocument();
    });
    expect(screen.getByRole("dialog").textContent).toContain("data.csv");
  });
});
