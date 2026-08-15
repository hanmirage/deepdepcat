import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ArtifactCard } from "@/components/chat/ArtifactCard";
import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useDepworkStore } from "@/stores/depworkStore";
import type { MessageBlock } from "@/types";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "chat.artifactLabel": "文档产物",
        "chat.openPreview": "打开预览",
      })[key] ?? key,
  }),
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return { ...actual, isTauri: true };
});

beforeEach(() => {
  useAppStore.setState({ mode: "code" });
  useRightPanelStore.setState({
    open: false,
    pendingFile: { code: null, depwork: null },
  });
  useDepworkStore.setState({
    rootPath: null,
    tree: [],
    treeLoading: false,
    selectedFile: null,
  });
});

const artifact: Extract<MessageBlock, { type: "artifact" }> = {
  type: "artifact",
  id: "c1",
  path: "D:\\out\\report.docx",
  name: "report.docx",
};

describe("ArtifactCard", () => {
  it("renders the product name, extension badge and open button", () => {
    render(<ArtifactCard artifact={artifact} />);
    expect(screen.getByText("report.docx")).toBeInTheDocument();
    expect(screen.getByText("docx")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "打开预览" })).toBeInTheDocument();
  });

  it("open-preview reveals the file in the right panel (code mode)", async () => {
    const user = userEvent.setup();
    render(<ArtifactCard artifact={artifact} />);
    await user.click(screen.getByRole("button", { name: "打开预览" }));
    await waitFor(() => {
      expect(useRightPanelStore.getState().open).toBe(true);
      expect(useRightPanelStore.getState().pendingFile.code).toBe("D:\\out\\report.docx");
    });
  });

  it("in depwork mode also loads the file into the preview panel", async () => {
    useAppStore.setState({ mode: "depwork" });
    const user = userEvent.setup();
    render(<ArtifactCard artifact={artifact} />);
    await user.click(screen.getByRole("button", { name: "打开预览" }));
    await waitFor(() => {
      expect(useDepworkStore.getState().selectedFile?.path).toBe("D:\\out\\report.docx");
    });
    expect(useRightPanelStore.getState().pendingFile.depwork).toBe("D:\\out\\report.docx");
  });
});
