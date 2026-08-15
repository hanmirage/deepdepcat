import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { CsvViewer } from "../CsvViewer";

const readWorkspaceBinaryFileMock = vi.fn<(path: string) => Promise<Uint8Array | null>>();

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    isTauri: true,
    readWorkspaceBinaryFile: (path: string) => readWorkspaceBinaryFileMock(path),
  };
});

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string, opts?: { defaultValue?: string; count?: number }) =>
      opts?.defaultValue
        ? opts.defaultValue.replace("{{count}}", String(opts.count ?? ""))
        : key,
  }),
  initReactI18next: { type: "3rdParty", init: () => {} },
}));

beforeEach(() => {
  readWorkspaceBinaryFileMock.mockReset();
});

describe("CsvViewer", () => {
  it("renders a UTF-8 CSV as a table", async () => {
    readWorkspaceBinaryFileMock.mockResolvedValue(
      new TextEncoder().encode("name,score\nAlice,95\nBob,88\n"),
    );
    render(<CsvViewer filePath="C:\\ws\\data.csv" />);

    expect(await screen.findByText("Alice")).toBeTruthy();
    expect(screen.getByText("95")).toBeTruthy();
    expect(screen.getByText("Bob")).toBeTruthy();
    expect(screen.getByText("name")).toBeTruthy();
    expect(screen.getByText("A")).toBeTruthy();
    expect(screen.getByText("B")).toBeTruthy();
  });

  it("decodes GBK Chinese CSVs", async () => {
    // GBK bytes for 名称,数值\n苹果,3\n
    const gbk = new Uint8Array([
      0xc3, 0xfb, 0xb3, 0xc6, 0x2c, 0xca, 0xfd, 0xd6, 0xb5, 0x0a, 0xc6, 0xbb,
      0xb9, 0xfb, 0x2c, 0x33, 0x0a,
    ]);
    readWorkspaceBinaryFileMock.mockResolvedValue(gbk);
    render(<CsvViewer filePath="C:\\ws\\sales.csv" />);

    expect(await screen.findByText("苹果")).toBeTruthy();
    expect(screen.getByText("名称")).toBeTruthy();
  });

  it("shows the unavailable message for empty files", async () => {
    readWorkspaceBinaryFileMock.mockResolvedValue(new Uint8Array(0));
    render(<CsvViewer filePath="C:\\ws\\empty.csv" />);

    await waitFor(() => {
      expect(screen.getByText(/桌面端|desktop app/i)).toBeTruthy();
    });
  });
});
