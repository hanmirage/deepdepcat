/**
 * DocxViewer — renders a .docx document in the Depwork preview panel.
 *
 * Uses docx-preview (pure frontend, offline, no server) to render the OOXML
 * document into styled HTML. The file bytes are read via the Tauri fs
 * plugin; browser dev mode shows a hint instead.
 */

import { useEffect, useRef, useState } from "react";
import { logError } from "@/lib/logger";
import { renderAsync } from "docx-preview";
import { FileWarning, Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { readWorkspaceBinaryFile } from "@/lib/tauri";

interface DocxViewerProps {
  filePath: string;
}

export function DocxViewer({ filePath }: DocxViewerProps) {
  const { t } = useTranslation();
  const containerRef = useRef<HTMLDivElement>(null);
  const [state, setState] = useState<"loading" | "ready" | "error" | "unavailable">("loading");

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    let cancelled = false;
    setState("loading");

    void (async () => {
      try {
        const bytes = await readWorkspaceBinaryFile(filePath);
        if (cancelled) return;
        if (!bytes || bytes.length === 0) {
          setState("unavailable");
          return;
        }
        // Container must be empty before render — docx-preview appends.
        container.innerHTML = "";
        await renderAsync(bytes, container, undefined, {
          className: "docx-preview",
          inWrapper: false,
          ignoreWidth: false,
          ignoreHeight: false,
          breakPages: true,
          experimental: true,
        });
        if (!cancelled) setState("ready");
      } catch (e) {
        logError("DocxViewer", "render failed:", e);
        if (!cancelled) setState("error");
      }
    })();

    return () => {
      cancelled = true;
      container.innerHTML = "";
    };
  }, [filePath]);

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
      {/* docx-preview renders scaled-down pages; keep them readable in the
          narrow panel with a horizontal scrollbar. */}
      <div ref={containerRef} className="docx-host flex-1 overflow-auto p-3" />
    </div>
  );
}
