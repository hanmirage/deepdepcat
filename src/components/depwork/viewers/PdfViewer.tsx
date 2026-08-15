/**
 * PdfViewer — renders a .pdf file in the Depwork preview panel.
 *
 * Uses pdfjs-dist (pure frontend, offline) to rasterize pages onto a canvas.
 * The file bytes are read via the Tauri fs plugin; browser dev mode shows a
 * hint instead (same contract as the other viewers).
 */

import { useEffect, useRef, useState, useCallback } from "react";
import { logError } from "@/lib/logger";
import { ChevronLeft, ChevronRight, FileWarning, Loader2, Minus, Plus } from "lucide-react";
import { useTranslation } from "react-i18next";
import * as pdfjsLib from "pdfjs-dist";
import type { PDFDocumentProxy } from "pdfjs-dist";
import { readWorkspaceBinaryFile } from "@/lib/tauri";
import { Button } from "@/components/ui/button";

pdfjsLib.GlobalWorkerOptions.workerSrc = new URL(
  "pdfjs-dist/build/pdf.worker.min.mjs",
  import.meta.url
).toString();

interface PdfViewerProps {
  filePath: string;
}

const ZOOM_STEP = 0.25;
const MIN_ZOOM = 0.5;
const MAX_ZOOM = 3;

export function PdfViewer({ filePath }: PdfViewerProps) {
  const { t } = useTranslation();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<"loading" | "ready" | "error" | "unavailable">("loading");
  const [doc, setDoc] = useState<PDFDocumentProxy | null>(null);
  const [pageNum, setPageNum] = useState(1);
  const [pageCount, setPageCount] = useState(0);
  const [zoom, setZoom] = useState(1);
  const [fitWidth, setFitWidth] = useState(true);
  const [rendering, setRendering] = useState(false);
  const [containerW, setContainerW] = useState(0);
  const renderIdRef = useRef(0);
  const taskRef = useRef<ReturnType<typeof pdfjsLib.getDocument> | null>(null);
  const renderTaskRef = useRef<{ cancel: () => void } | null>(null);

  // fit-width mode renders at the container's current width — observe the
  // wrapper so a window/panel resize re-renders instead of going stale.
  useEffect(() => {
    const wrap = wrapRef.current;
    if (!wrap) return;
    const obs = new ResizeObserver((entries) => {
      for (const e of entries) {
        const w = (e.target as HTMLElement).clientWidth;
        if (w > 0 && Math.abs(w - containerW) > 2) setContainerW(w);
      }
    });
    obs.observe(wrap);
    return () => obs.disconnect();
  }, [containerW]);

  // Load the document once per file.
  useEffect(() => {
    let cancelled = false;
    setState("loading");
    setDoc(null);
    setPageNum(1);
    setPageCount(0);

    void (async () => {
      try {
        const bytes = await readWorkspaceBinaryFile(filePath);
        if (cancelled) return;
        if (!bytes || bytes.length === 0) {
          setState("unavailable");
          return;
        }
        const task = pdfjsLib.getDocument({ data: bytes });
        taskRef.current = task;
        const loaded = await task.promise;
        if (cancelled) {
          void task.destroy();
          return;
        }
        setDoc(loaded);
        setPageCount(loaded.numPages);
        setState("ready");
      } catch (e) {
        logError("PdfViewer", "load failed:", e);
        if (!cancelled) setState("error");
      }
    })();

    return () => {
      cancelled = true;
      renderIdRef.current += 1;
      void taskRef.current?.destroy();
      taskRef.current = null;
      setDoc(null);
    };
  }, [filePath]);

  // Render the current page. Any prior render task is cancelled so a rapid
  // page/zoom change can't leave two pdf.js renders fighting over the same
  // canvas ("Cannot use the same canvas ..."). The effect cleanup also
  // cancels on unmount.
  useEffect(() => {
    if (!doc || state !== "ready") return;
    const canvas = canvasRef.current;
    const wrap = wrapRef.current;
    if (!canvas || !wrap) return;

    const renderId = ++renderIdRef.current;
    setRendering(true);

    void (async () => {
      try {
        const page = await doc.getPage(pageNum);
        if (renderId !== renderIdRef.current) return; // superseded while loading
        const baseScale = 1.5;
        const viewport1 = page.getViewport({ scale: baseScale });
        let scale = baseScale * zoom;
        if (fitWidth) {
          const avail = Math.max(wrap.clientWidth - 8, 100);
          scale = (avail / viewport1.width) * baseScale;
        }
        const viewport = page.getViewport({ scale });
        // pdfjs v4+ sizes the canvas itself (including devicePixelRatio).
        const renderTask = page.render({ canvas, viewport });
        renderTaskRef.current = renderTask;
        await renderTask.promise;
        if (renderId !== renderIdRef.current) return;
        setRendering(false);
      } catch (e) {
        // RenderTask.cancel() rejects with RenderingCancelledException — a
        // superseded render, not a real failure; don't log it as one.
        if (renderId === renderIdRef.current) {
          logError("PdfViewer", "render failed:", e);
          setRendering(false);
        }
      }
    })();

    // Cancel the in-flight render when page/zoom/unmount supersedes it —
    // otherwise pdf.js throws on concurrent renders of the same canvas.
    return () => {
      renderTaskRef.current?.cancel();
      renderTaskRef.current = null;
    };
    // containerW: re-render when the wrapper resizes in fit-width mode.
  }, [doc, pageNum, zoom, fitWidth, state, containerW]);

  const goPage = useCallback((delta: number) => {
    setPageNum((p) => Math.max(1, Math.min(pageCount, p + delta)));
  }, [pageCount]);

  const zoomIn = useCallback(() => {
    setFitWidth(false);
    setZoom((z) => Math.min(MAX_ZOOM, +(z + ZOOM_STEP).toFixed(2)));
  }, []);

  const zoomOut = useCallback(() => {
    setFitWidth(false);
    setZoom((z) => Math.max(MIN_ZOOM, +(z - ZOOM_STEP).toFixed(2)));
  }, []);

  // Keyboard paging — ←/→ flip pages while the viewer area has focus.
  // Scoped to the viewer itself (focusable scroll container) so global
  // arrow keys elsewhere in the app are never hijacked.
  const handleViewerKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === "ArrowLeft") {
      e.preventDefault();
      goPage(-1);
    } else if (e.key === "ArrowRight") {
      e.preventDefault();
      goPage(1);
    }
  }, [goPage]);

  if (state === "unavailable") {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center">
        <FileWarning className="h-8 w-8 text-muted-foreground/30" />
        <p className="text-xs text-muted-foreground">
          {t("depwork.previewBrowserOnly", {
            defaultValue: "文档预览仅在桌面端可用（浏览器模式无法读取文件）",
          })}
        </p>
      </div>
    );
  }

  return (
    <div className="relative flex h-full flex-col overflow-hidden bg-white">
      {state === "ready" && (
        <div className="flex items-center justify-between border-b border-border px-2 py-1.5">
          <div className="flex items-center gap-1">
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6"
              disabled={pageNum <= 1}
              onClick={() => goPage(-1)}
              aria-label={t("depwork.pdfPrev", { defaultValue: "上一页" })}
            >
              <ChevronLeft className="h-3.5 w-3.5" />
            </Button>
            <span className="min-w-16 text-center text-[10px] tabular-nums text-muted-foreground">
              {pageNum} / {pageCount}
            </span>
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6"
              disabled={pageNum >= pageCount}
              onClick={() => goPage(1)}
              aria-label={t("depwork.pdfNext", { defaultValue: "下一页" })}
            >
              <ChevronRight className="h-3.5 w-3.5" />
            </Button>
          </div>
          <div className="flex items-center gap-1">
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6"
              onClick={zoomOut}
              aria-label={t("depwork.pdfZoomOut", { defaultValue: "缩小" })}
            >
              <Minus className="h-3.5 w-3.5" />
            </Button>
            <span className="min-w-8 text-center text-[10px] tabular-nums text-muted-foreground">
              {fitWidth ? t("depwork.pdfFitWidth", { defaultValue: "适配" }) : `${Math.round(zoom * 100)}%`}
            </span>
            <Button
              variant="ghost"
              size="icon"
              className="h-6 w-6"
              onClick={zoomIn}
              aria-label={t("depwork.pdfZoomIn", { defaultValue: "放大" })}
            >
              <Plus className="h-3.5 w-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 px-2 text-[10px]"
              onClick={() => setFitWidth((f) => !f)}
              aria-pressed={fitWidth}
            >
              {t("depwork.pdfToggleFit", { defaultValue: "100%" })}
            </Button>
          </div>
        </div>
      )}
      {state === "loading" && (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-white/70">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        </div>
      )}
      {state === "error" && (
        <div className="flex flex-1 items-center justify-center px-6 text-center">
          <p className="text-xs text-muted-foreground">
            {t("depwork.previewCantRead", { defaultValue: "无法预览此文档" })}
          </p>
        </div>
      )}
      {state === "ready" && (
        <div
          ref={wrapRef}
          tabIndex={0}
          onKeyDown={handleViewerKeyDown}
          className="min-h-0 flex-1 overflow-auto bg-[hsl(var(--muted))] p-1 outline-none focus-visible:ring-1 focus-visible:ring-primary/40"
        >
          <canvas ref={canvasRef} className="mx-auto block bg-white shadow-sm" />
          {rendering && (
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center bg-white/40">
              <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
            </div>
          )}
        </div>
      )}
    </div>
  );
}
