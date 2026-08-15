/**
 * Document viewers + dispatch tests (main line #70).
 *
 * Covers:
 *  - extractDocumentPath / basenameOf (dispatch pure functions)
 *  - PptxViewer: real zip fixture rendered through the component
 *  - XlsxViewer: real SheetJS-written workbook rendered through the component
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import JSZip from "jszip";
import * as XLSX from "xlsx";
import { extractDocumentPath, basenameOf } from "@/stores/depworkChatStore";
import { PptxViewer } from "@/components/depwork/viewers/PptxViewer";
import { XlsxViewer } from "@/components/depwork/viewers/XlsxViewer";
import { PdfViewer } from "@/components/depwork/viewers/PdfViewer";

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
  // The i18n module chains `.use(initReactI18next)` — keep it importable.
  initReactI18next: { type: "3rdParty", init: () => {} },
}));

// pdfjs-dist v6 touches DOMMatrix at module scope (absent in jsdom). The
// viewer tests only exercise the bytes-unavailable path, where getDocument is
// never called — a stub keeps the import safe.
vi.mock("pdfjs-dist", () => ({
  GlobalWorkerOptions: { workerSrc: "" },
  getDocument: vi.fn(),
}));

beforeEach(() => {
  readWorkspaceBinaryFileMock.mockReset();
});

describe("extractDocumentPath (dispatch)", () => {
  it("extracts a Windows docx path from a generation result", () => {
    const result = "Created Word document: D:\\out\\report.docx\n(12 KB, Word-compatible)";
    expect(extractDocumentPath(result)).toBe("D:\\out\\report.docx");
  });

  it("extracts a POSIX pptx path", () => {
    const result = "Presentation saved: /home/user/deck.pptx\n(3 slides)";
    expect(extractDocumentPath(result)).toBe("/home/user/deck.pptx");
  });

  it("returns null when no document path is present", () => {
    expect(extractDocumentPath("No documents were created")).toBeNull();
    expect(extractDocumentPath("")).toBeNull();
  });

  it("extracts a pdf path", () => {
    expect(extractDocumentPath("Exported PDF: D:\\out\\report.pdf\n(3 pages)")).toBe(
      "D:\\out\\report.pdf",
    );
  });

  it("takes the last path when multiple appear", () => {
    const result = "Created: a.docx then also b.xlsx";
    expect(extractDocumentPath(result)).toBe("b.xlsx");
  });

  it("strips trailing punctuation", () => {
    expect(extractDocumentPath("done: report.pptx.")).toBe("report.pptx");
  });
});

describe("basenameOf", () => {
  it("handles windows and posix separators", () => {
    expect(basenameOf("D:\\a\\b\\f.docx")).toBe("f.docx");
    expect(basenameOf("/a/b/f.pptx")).toBe("f.pptx");
    expect(basenameOf("plain.xlsx")).toBe("plain.xlsx");
  });
});

/** Build a minimal pptx zip: one slide with a text box. */
async function minimalPptx(text: string): Promise<Uint8Array> {
  const zip = new JSZip();
  zip.file(
    "[Content_Types].xml",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
    <Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
      <Default Extension="xml" ContentType="application/xml"/>
      <Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
    </Types>`,
  );
  zip.file(
    "ppt/slides/slide1.xml",
    `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
    <p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
           xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
      <p:cSld>
        <p:spTree>
          <p:sp>
            <p:spPr><a:xfrm><a:off x="457200" y="457200"/><a:ext cx="3657600" cy="914400"/></a:xfrm></p:spPr>
            <p:txBody>
              <a:bodyPr/>
              <a:p>
                <a:r><a:t>${text}</a:t></a:r>
              </a:p>
            </p:txBody>
          </p:sp>
        </p:spTree>
      </p:cSld>
    </p:sld>`,
  );
  return zip.generateAsync({ type: "uint8array" });
}

describe("PptxViewer", () => {
  it("renders slide text from a real pptx zip", async () => {
    readWorkspaceBinaryFileMock.mockResolvedValue(await minimalPptx("季度业绩汇报"));
    render(<PptxViewer filePath="test.pptx" />);

    await waitFor(() => expect(screen.getByText("季度业绩汇报")).toBeInTheDocument());
    // Pager shows 1/1.
    expect(screen.getByText("1 / 1")).toBeInTheDocument();
  });

  it("shows the browser-only hint when bytes are unavailable", async () => {
    readWorkspaceBinaryFileMock.mockResolvedValue(null);
    render(<PptxViewer filePath="test.pptx" />);
    await waitFor(() => expect(screen.getByText(/桌面端可用/)).toBeInTheDocument());
  });
});

describe("XlsxViewer", () => {
  it("renders cells from a real workbook", async () => {
    const wb = XLSX.utils.book_new();
    const ws = XLSX.utils.aoa_to_sheet([
      ["项目", "金额"],
      ["甲项目", 100],
      ["乙项目", 200],
    ]);
    XLSX.utils.book_append_sheet(wb, ws, "数据");
    const bytes = XLSX.write(wb, { type: "array", bookType: "xlsx" });
    readWorkspaceBinaryFileMock.mockResolvedValue(new Uint8Array(bytes));

    render(<XlsxViewer filePath="test.xlsx" />);

    await waitFor(() => expect(screen.getByText("项目")).toBeInTheDocument());
    expect(screen.getByText("金额")).toBeInTheDocument();
    expect(screen.getByText("甲项目")).toBeInTheDocument();
    expect(screen.getByText("200")).toBeInTheDocument();
  });

  it("shows the browser-only hint when bytes are unavailable", async () => {
    readWorkspaceBinaryFileMock.mockResolvedValue(null);
    render(<XlsxViewer filePath="test.xlsx" />);
    await waitFor(() => expect(screen.getByText(/桌面端可用/)).toBeInTheDocument());
  });
});

describe("PdfViewer", () => {
  it("shows the browser-only hint when bytes are unavailable", async () => {
    readWorkspaceBinaryFileMock.mockResolvedValue(null);
    render(<PdfViewer filePath="test.pdf" />);
    await waitFor(() => expect(screen.getByText(/桌面端可用/)).toBeInTheDocument());
  });
});
