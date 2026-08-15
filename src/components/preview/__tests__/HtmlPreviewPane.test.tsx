/**
 * HtmlPreviewPane tests — the Claude-Preview-style dev preview:
 * - consumes the one-shot per-mode pendingPreview target on mount
 * - renders a local HTML report in a sandboxed srcdoc iframe (CSP injected)
 * - routes external URLs to the system browser, never rendering them
 * - a target stashed for the OTHER mode is not consumed
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { HtmlPreviewPane } from "@/components/preview/HtmlPreviewPane";
import { useRightPanelStore } from "@/stores/rightPanelStore";

vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (key: string) =>
      ({
        "preview.empty": "agent 生成 HTML 报告后，这里会显示预览",
        "preview.openInBrowser": "在系统浏览器打开",
      })[key] ?? key,
  }),
}));

const previewApiMock = vi.hoisted(() => ({
  readPreviewTarget: vi.fn(async () => ({ html: "", filename: "" })),
  openExternal: vi.fn(async () => {}),
}));

vi.mock("@/lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/tauri")>();
  return {
    ...actual,
    previewApi: previewApiMock,
  };
});

vi.mock("@/components/chat/McpAppView", () => ({
  injectCspIntoHtml: (html: string) => `CSP[${html}]`,
}));

vi.mock("@/components/ui/dialog", () => ({
  Dialog: ({ open, children }: { open?: boolean; children: React.ReactNode }) =>
    open ? <div>{children}</div> : null,
  DialogContent: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogHeader: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
  DialogTitle: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

function resetStore() {
  useRightPanelStore.setState({
    pendingPreview: { code: null, depwork: null },
  });
}

beforeEach(() => {
  resetStore();
  previewApiMock.readPreviewTarget.mockReset();
  previewApiMock.openExternal.mockReset();
});

describe("HtmlPreviewPane", () => {
  it("shows an empty state when no target has been handed over", () => {
    render(<HtmlPreviewPane mode="code" />);
    expect(screen.getByText("agent 生成 HTML 报告后，这里会显示预览")).toBeTruthy();
  });

  it("consumes a local report target and renders it in a sandboxed srcdoc iframe", async () => {
    previewApiMock.readPreviewTarget.mockResolvedValue({
      html: "<h1>报告</h1>",
      filename: "report.html",
    });
    useRightPanelStore
      .getState()
      .setPendingPreview("code", { url: null, path: "D:\\proj\\report.html" });

    render(<HtmlPreviewPane mode="code" />);

    // The one-shot target is consumed on mount.
    expect(useRightPanelStore.getState().pendingPreview.code).toBeNull();

    await waitFor(() => {
      expect(previewApiMock.readPreviewTarget).toHaveBeenCalledWith("D:\\proj\\report.html");
      expect(screen.getByText("report.html")).toBeTruthy();
    });

    const frame = document.querySelector("iframe") as HTMLIFrameElement;
    expect(frame).not.toBeNull();
    // The CSP was injected into the srcdoc and the report's own content rides.
    expect(frame.getAttribute("srcdoc")).toContain("CSP[<h1>报告</h1>]");
    // Sandboxed: scripts allowed, no same-origin (opaque origin).
    expect(frame.getAttribute("sandbox")).toContain("allow-scripts");
    expect(frame.getAttribute("sandbox")).not.toContain("allow-same-origin");
  });

  it("never consumes a target stashed for the other mode", async () => {
    previewApiMock.readPreviewTarget.mockResolvedValue({
      html: "<h1>x</h1>",
      filename: "dep.html",
    });
    useRightPanelStore
      .getState()
      .setPendingPreview("depwork", { url: null, path: "D:\\dep.html" });

    render(<HtmlPreviewPane mode="code" />);

    expect(useRightPanelStore.getState().pendingPreview.depwork).not.toBeNull();
    expect(previewApiMock.readPreviewTarget).not.toHaveBeenCalled();
  });

  it("embeds an external URL in an in-app iframe with a manual system-browser fallback", async () => {
    useRightPanelStore
      .getState()
      .setPendingPreview("code", { url: "https://example.com", path: null });

    render(<HtmlPreviewPane mode="code" />);

    // The URL is rendered IN-APP (iframe), never auto-opened in the system
    // browser.
    const frame = document.querySelector("iframe") as HTMLIFrameElement;
    expect(frame).not.toBeNull();
    expect(frame.getAttribute("src")).toBe("https://example.com");
    // Keeps the site's own origin so its scripts/storage work.
    expect(frame.getAttribute("sandbox")).toContain("allow-same-origin");

    // The header keeps a manual fallback for sites that forbid embedding.
    fireEvent.click(screen.getByTitle("在系统浏览器打开"));
    await waitFor(() => {
      expect(previewApiMock.openExternal).toHaveBeenCalledWith("https://example.com");
    });
  });
});
