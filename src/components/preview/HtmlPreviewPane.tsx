/**
 * HtmlPreviewPane — the Claude-Preview-style dev preview (replaces the old
 * real-browser CDP screencast pane).
 *
 * When the agent opens a LOCAL HTML report (`dev_browser_open` → path), the
 * file's content is read via `read_preview_target`, given a restrictive CSP
 * (reused from the MCP-app sandbox) and rendered in a `srcdoc` iframe that
 * runs the report's own scripts — so interactive dashboards "work" the way
 * Claude's Preview works. External URLs are not rendered here: they open the
 * system browser (`open_preview_external`).
 *
 * The pane is driven purely by `pendingPreview[mode]`: the one-shot agent
 * target is stashed per mode, consumed on mount (and on every subsequent
 * handoff), then cleared.
 */

import { useCallback, useEffect, useState } from "react";
import { ExternalLink, FileCode2, Loader2, Maximize2, RefreshCw } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AppMode } from "@/config/constants";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { previewApi, type PreviewTarget } from "@/lib/tauri";
import { injectCspIntoHtml } from "@/components/chat/McpAppView";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

/** iframe sandbox flags — scripts + forms + modals + popups, NO
 *  allow-same-origin (opaque origin, no host DOM/storage access). Mirrors the
 *  MCP-app sandbox contract. */
const IFRAME_SANDBOX = "allow-scripts allow-forms allow-modals allow-popups";
/** Sandbox for an EMBEDDED EXTERNAL URL — keeps the site's own origin (so
 *  its scripts/storage work) while still forbidding top navigation and host
 *  access. A site that forbids framing shows blank; the header offers a
 *  manual system-browser open. */
const URL_SANDBOX = "allow-scripts allow-same-origin allow-forms allow-popups";

/** The sandboxed preview canvas — runs the report's own scripts. */
function PreviewFrame({
  srcdoc,
  filename,
  nonce,
}: {
  srcdoc: string;
  filename: string;
  nonce: number;
}) {
  return (
    <iframe
      key={nonce}
      sandbox={IFRAME_SANDBOX}
      srcDoc={srcdoc}
      title={filename}
      className="h-full w-full border-0 bg-white"
    />
  );
}

/** The slim header: filename + reload / open-in-system / fullscreen. */
function PreviewHeader({
  title,
  isUrl,
  onReload,
  onOpenExternal,
  onFullscreen,
}: {
  title: string;
  isUrl: boolean;
  onReload: () => void;
  onOpenExternal: () => void;
  onFullscreen: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex shrink-0 items-center gap-2 border-b border-border/60 bg-muted/30 px-3 py-1.5">
      <FileCode2 className="h-3.5 w-3.5 shrink-0 text-muted-foreground/60" />
      <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-foreground/80">
        {title}
      </span>
      {isUrl ? (
        <button
          onClick={onOpenExternal}
          className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground"
          title={t("preview.openInBrowser", { defaultValue: "在系统浏览器打开" })}
        >
          <ExternalLink className="h-3 w-3" />
        </button>
      ) : (
        <>
          <button
            onClick={onReload}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground"
            title={t("preview.reload", { defaultValue: "重新加载" })}
          >
            <RefreshCw className="h-3 w-3" />
          </button>
          <button
            onClick={onFullscreen}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground"
            title={t("preview.fullscreen", { defaultValue: "全屏" })}
          >
            <Maximize2 className="h-3 w-3" />
          </button>
        </>
      )}
    </div>
  );
}

/** Fullscreen overlay — the same sandboxed frame, near-full-screen (mirrors
 *  FilePreviewDialog). */
function PreviewFullscreen({
  open,
  onOpenChange,
  filename,
  frame,
  onReload,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  filename: string;
  frame: React.ReactNode;
  onReload: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[90vh] w-[92vw] max-w-[92vw] flex-col p-0">
        <DialogHeader className="flex shrink-0 flex-row items-center gap-2 border-b border-border px-4 py-3">
          <Maximize2 className="h-4 w-4 shrink-0 text-muted-foreground" />
          <DialogTitle className="truncate text-sm font-semibold">{filename}</DialogTitle>
          <button
            onClick={onReload}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground"
            title={t("preview.reload", { defaultValue: "重新加载" })}
          >
            <RefreshCw className="h-3 w-3" />
          </button>
        </DialogHeader>
        <div className="min-h-0 flex-1">{frame}</div>
      </DialogContent>
    </Dialog>
  );
}

/** The pane body — the preview canvas, or the right empty/error/loading state. */
function PreviewBody({
  loaded,
  error,
  preview,
  frame,
}: {
  loaded: { path: string; url: string | null } | null;
  error: string | null;
  preview: PreviewTarget | null;
  frame: React.ReactNode;
}) {
  const { t } = useTranslation();
  if (loaded?.url) {
    // External URL — embedded in-app. Sites that forbid embedding render
    // blank; the header's system-browser button stays as a manual fallback.
    return (
      <iframe
        src={loaded.url}
        sandbox={URL_SANDBOX}
        title={loaded.url}
        className="h-full w-full border-0 bg-white"
      />
    );
  }
  if (error) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
        <p className="text-[11px] text-destructive">{error}</p>
      </div>
    );
  }
  if (!loaded) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
        <FileCode2 className="h-8 w-8 text-muted-foreground/40" />
        <p className="text-[11px] text-muted-foreground">
          {t("preview.empty", { defaultValue: "agent 生成 HTML 报告后，这里会显示预览" })}
        </p>
      </div>
    );
  }
  if (!preview) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-6">
        <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
      </div>
    );
  }
  return <>{frame}</>;
}

interface HtmlPreviewPaneProps {
  mode: AppMode;
}

export function HtmlPreviewPane({ mode }: HtmlPreviewPaneProps) {
  const pending = useRightPanelStore((s) => s.pendingPreview[mode]);
  const clearPendingPreview = useRightPanelStore((s) => s.clearPendingPreview);
  const [loaded, setLoaded] = useState<{ path: string; url: string | null } | null>(null);
  const [preview, setPreview] = useState<PreviewTarget | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const [fullscreen, setFullscreen] = useState(false);

  // Consume the one-shot agent target whenever one lands (mount or handoff).
  useEffect(() => {
    if (!pending) return;
    const target = pending;
    clearPendingPreview(mode);
    setError(null);
    setPreview(null);
    if (target.path) {
      setLoaded({ path: target.path, url: null });
      void previewApi
        .readPreviewTarget(target.path)
        .then((t) => setPreview(t))
        .catch((e) => setError(e instanceof Error ? e.message : String(e)));
    } else if (target.url) {
      // External URL — not rendered; the pane offers the system browser.
      setLoaded({ path: "", url: target.url });
    }
  }, [pending, mode, clearPendingPreview]);

  const reload = useCallback(() => {
    if (!loaded || loaded.url) return;
    setPreview(null);
    setError(null);
    void previewApi
      .readPreviewTarget(loaded.path)
      .then((t) => setPreview(t))
      .catch((e) => setError(e instanceof Error ? e.message : String(e)));
    setNonce((n) => n + 1);
  }, [loaded]);

  const openExternal = useCallback(() => {
    if (!loaded) return;
    void previewApi.openExternal(loaded.url ?? loaded.path).catch(() => {});
  }, [loaded]);

  const srcdoc = preview ? injectCspIntoHtml(preview.html) : "";
  const frame = preview ? (
    <PreviewFrame srcdoc={srcdoc} filename={preview.filename} nonce={nonce} />
  ) : null;

  const header =
    loaded || preview ? (
      <PreviewHeader
        title={preview?.filename ?? loaded?.url ?? loaded?.path ?? ""}
        isUrl={!!loaded?.url}
        onReload={reload}
        onOpenExternal={openExternal}
        onFullscreen={() => setFullscreen(true)}
      />
    ) : null;

  return (
    <div className="flex h-full min-h-0 flex-col">
      {header}
      <div className="min-h-0 flex-1">
        <PreviewBody
          loaded={loaded}
          error={error}
          preview={preview}
          frame={frame}
        />
      </div>
      {preview && (
        <PreviewFullscreen
          open={fullscreen}
          onOpenChange={setFullscreen}
          filename={preview.filename}
          frame={frame}
          onReload={reload}
        />
      )}
    </div>
  );
}
