/**
 * FilePreviewBody — renders the content of a selected file by category.
 *
 * Shared by the right-panel PreviewPanel (small, embedded) and the full-screen
 * FilePreviewDialog (Claude Preview style): one component, two sizes. Text
 * content loads lazily here (capped), so each mount manages its own state and
 * the dialog can mount independently of the panel.
 */

import { useState, useEffect, Suspense, lazy } from "react";
import { logError } from "@/lib/logger";
import { useTranslation } from "react-i18next";
import {
  FileText,
  Image as ImageIcon,
  ImageOff,
  RefreshCw,
  FileCode,
  File as FileIcon,
  Sparkles,
  Loader2,
  Copy,
  Check,
  ArrowLeft,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import {
  readWorkspaceTextFile,
  isTauri,
  toAssetUrl,
  pdfApi,
} from "@/lib/tauri";
import { MarkdownRenderer } from "@/components/chat/MarkdownRenderer";
import type { FileTreeNode } from "@/stores/depworkStore";
import type { LucideIcon } from "lucide-react";

// ── Lazy viewers ───────────────────────────────────────────
// The four office/PDF viewers pull in heavy libraries (pdfjs-dist ~1.3 MB,
// docx-preview, xlsx, jszip) that were statically imported into the MAIN
// bundle (1.5 MB). Loading them on demand keeps the app shell (and the Code
// surface, which never opens these) free of that weight.
const DocxViewer = lazy(() =>
  import("@/components/depwork/viewers/DocxViewer").then((m) => ({
    default: m.DocxViewer,
  })),
);
const XlsxViewer = lazy(() =>
  import("@/components/depwork/viewers/XlsxViewer").then((m) => ({
    default: m.XlsxViewer,
  })),
);
const CsvViewer = lazy(() =>
  import("@/components/depwork/viewers/CsvViewer").then((m) => ({
    default: m.CsvViewer,
  })),
);
const PptxViewer = lazy(() =>
  import("@/components/depwork/viewers/PptxViewer").then((m) => ({
    default: m.PptxViewer,
  })),
);
const PdfViewer = lazy(() =>
  import("@/components/depwork/viewers/PdfViewer").then((m) => ({
    default: m.PdfViewer,
  })),
);

/** Suspense fallback for a lazily loading viewer chunk. */
function ViewerFallback() {
  return (
    <div className="flex h-full items-center justify-center">
      <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
    </div>
  );
}

/** Get the file extension (lowercase, without dot). */
export function getExt(name: string): string {
  return name.split(".").pop()?.toLowerCase() ?? "";
}

/** Categorize file by extension. */
export type FileCategory =
  | "pdf"
  | "doc"
  | "spreadsheet"
  | "csv"
  | "slides"
  | "markdown"
  | "text"
  | "image"
  | "unknown";

export function categorize(name: string): FileCategory {
  const ext = getExt(name);
  if (ext === "pdf") return "pdf";
  if (ext === "doc" || ext === "docx") return "doc";
  if (ext === "xls" || ext === "xlsx") return "spreadsheet";
  if (ext === "csv") return "csv";
  if (ext === "ppt" || ext === "pptx") return "slides";
  if (ext === "md" || ext === "markdown") return "markdown";
  if (ext === "txt") return "text";
  if (["png", "jpg", "jpeg", "gif", "svg", "webp", "bmp"].includes(ext)) return "image";
  return "unknown";
}

/** Map category to icon. */
export function getCategoryIcon(category: FileCategory): LucideIcon {
  switch (category) {
    case "pdf":
    case "doc":
    case "text":
      return FileText;
    case "spreadsheet":
    case "csv":
      return FileText;
    case "markdown":
      return FileCode;
    case "image":
      return ImageIcon;
    default:
      return FileIcon;
  }
}

/** Format byte size to human-readable string. */
export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/** Image preview using Tauri's asset protocol. */
function ImagePreview({ path }: { path: string }) {
  const { t } = useTranslation();
  // convertFileSrc builds the correct asset:// URL (escapes the path per the
  // platform's protocol rules) — hand-assembling asset://localhost/... with
  // encodeURIComponent does not work for Windows drive-letter paths.
  const src = toAssetUrl(path);
  const [failed, setFailed] = useState(false);
  const fileName = path.split(/[\\/]/).pop() ?? path;

  if (failed) {
    return (
      <div className="flex flex-col items-center gap-2 py-8 text-center">
        <ImageOff className="h-8 w-8 text-muted-foreground/30" />
        <p className="text-[11px] text-muted-foreground">
          {t("depwork.previewImageFailed", { defaultValue: "无法加载图片" })}
        </p>
        <Button
          variant="outline"
          size="sm"
          className="h-7 gap-1 text-[10px]"
          onClick={() => setFailed(false)}
        >
          <RefreshCw className="h-3 w-3" />
          {t("common.retry")}
        </Button>
      </div>
    );
  }

  return (
    <div className="flex items-center justify-center">
      <img
        src={src}
        alt={fileName}
        className="max-h-[400px] max-w-full rounded-lg border border-border object-contain"
        onError={() => setFailed(true)}
      />
    </div>
  );
}

/**
 * PDF section — real page rendering plus working quick actions.
 *
 * - Extract text: pulls the PDF text layer via the backend command and shows
 *   it in a scrollable text view.
 * - To Word / To Excel / Extract tables: dispatched to the Agent, which picks
 *   the right tool (doc_read, table_process) — surfaced live in the task panel.
 */
function PdfSection({ filePath }: { filePath: string }) {
  const { t } = useTranslation();
  const [view, setView] = useState<"pages" | "text">("pages");
  const [text, setText] = useState<string | null>(null);
  const [extracting, setExtracting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const extractText = async () => {
    setExtracting(true);
    setError(null);
    try {
      const result = await pdfApi.extractText(filePath);
      setText(result);
      setView("text");
    } catch {
      setError(t("depwork.previewExtractFail"));
    } finally {
      setExtracting(false);
    }
  };

  const sendToAgent = (prompt: string) => {
    // Fill the input only — the user reviews the prompt (and the file path
    // inside it) before sending.
    const store = useDepworkChatStore.getState();
    store.setInputText(prompt);
  };

  const copyText = async () => {
    if (!text) return;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard unavailable — text stays visible for manual selection.
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* Quick actions */}
      <div className="flex flex-wrap gap-1.5 border-b border-border p-2">
        <Button
          variant="outline"
          size="sm"
          className="h-7 gap-1.5 text-[11px]"
          onClick={extractText}
          disabled={extracting || view === "text"}
        >
          {extracting ? (
            <Loader2 className="h-3 w-3 animate-spin" />
          ) : (
            <Sparkles className="h-3 w-3" />
          )}
          {view === "text" ? t("depwork.previewShowText") : t("depwork.previewExtractText")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="h-7 gap-1.5 text-[11px]"
          onClick={() =>
            sendToAgent(t("depwork.convertToWordPrompt", {
              path: filePath,
              defaultValue: `请把 ${filePath} 转换为 Word 文档，保存到原文件同目录`,
            }))
          }
        >
          <Sparkles className="h-3 w-3" />
          {t("depwork.previewToWord")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="h-7 gap-1.5 text-[11px]"
          onClick={() =>
            sendToAgent(t("depwork.convertToExcelPrompt", {
              path: filePath,
              defaultValue: `请把 ${filePath} 转换为 Excel 文件，保存到原文件同目录`,
            }))
          }
        >
          <Sparkles className="h-3 w-3" />
          {t("depwork.previewToExcel")}
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="h-7 gap-1.5 text-[11px]"
          onClick={() =>
            sendToAgent(t("depwork.extractTablePrompt", {
              path: filePath,
              defaultValue: `请提取 ${filePath} 中的表格数据并整理为 Excel 表格保存到原文件同目录`,
            }))
          }
        >
          <Sparkles className="h-3 w-3" />
          {t("depwork.previewExtractTable")}
        </Button>
      </div>

      {error && (
        <p className="border-b border-border px-3 py-1.5 text-[10px] text-destructive">{error}</p>
      )}

      {view === "text" && text !== null ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex items-center justify-between border-b border-border px-2 py-1.5">
            <Button
              variant="ghost"
              size="sm"
              className="h-6 gap-1 px-2 text-[10px]"
              onClick={() => setView("pages")}
            >
              <ArrowLeft className="h-3 w-3" />
              {t("depwork.previewBackToPages")}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 gap-1 px-2 text-[10px]"
              onClick={copyText}
            >
              {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
              {copied ? "✓" : t("depwork.previewCopy")}
            </Button>
          </div>
          <ScrollArea className="flex-1">
            <pre className="whitespace-pre-wrap break-words px-3 py-2 font-mono text-[11px] leading-relaxed text-muted-foreground">
              {text}
            </pre>
          </ScrollArea>
        </div>
      ) : (
        <div className="min-h-0 flex-1">
          <Suspense fallback={<ViewerFallback />}>
            <PdfViewer filePath={filePath} />
          </Suspense>
        </div>
      )}
    </div>
  );
}

/** Render the selected file's content by category. */
export function FilePreviewBody({ selectedFile }: { selectedFile: FileTreeNode }) {
  const { t } = useTranslation();
  const [textContent, setTextContent] = useState<string | null>(null);
  const [textTruncated, setTextTruncated] = useState(false);
  const [loading, setLoading] = useState(false);

  const category = categorize(selectedFile.name);
  const isTextLike = category === "markdown" || category === "text";

  // Load text content when a text-like file is selected.
  // Capped: a 500 MB log read into the DOM would freeze the panel — show
  // the first slice and say it was truncated.
  const MAX_TEXT_CHARS = 200_000;
  useEffect(() => {
    setTextContent(null);
    setTextTruncated(false);
    if (!isTextLike) return;

    // Guard against the race where a faster previous file's read resolves
    // after the current one — the stale content must never overwrite the
    // selected file's preview.
    let alive = true;
    setLoading(true);
    readWorkspaceTextFile(selectedFile.path)
      .then((content) => {
        if (!alive) return;
        if (content.length > MAX_TEXT_CHARS) {
          setTextContent(content.slice(0, MAX_TEXT_CHARS));
          setTextTruncated(true);
        } else {
          setTextContent(content);
        }
        setLoading(false);
      })
      .catch((e) => {
        logError("FilePreviewBody", "Failed to read file:", e);
        if (!alive) return;
        setTextContent(t("depwork.previewCantRead"));
        setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [selectedFile.path, isTextLike, t]);

  if (category === "doc") {
    return (
      <div className="min-h-0 flex-1">
        <Suspense fallback={<ViewerFallback />}>
          <DocxViewer filePath={selectedFile.path} />
        </Suspense>
      </div>
    );
  }
  if (category === "spreadsheet") {
    return (
      <div className="min-h-0 flex-1">
        <Suspense fallback={<ViewerFallback />}>
          <XlsxViewer filePath={selectedFile.path} />
        </Suspense>
      </div>
    );
  }
  if (category === "csv") {
    return (
      <div className="min-h-0 flex-1">
        <Suspense fallback={<ViewerFallback />}>
          <CsvViewer filePath={selectedFile.path} />
        </Suspense>
      </div>
    );
  }
  if (category === "slides") {
    return (
      <div className="min-h-0 flex-1">
        <Suspense fallback={<ViewerFallback />}>
          <PptxViewer filePath={selectedFile.path} />
        </Suspense>
      </div>
    );
  }

  return (
    <ScrollArea className="flex-1">
      <div className="p-3">
        {loading ? (
          <div className="flex items-center justify-center py-12">
            <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
          </div>
        ) : category === "image" && isTauri ? (
          <ImagePreview path={selectedFile.path} />
        ) : isTextLike && textContent !== null ? (
          <>
            {textTruncated && (
              <p className="mb-2 rounded-md border border-amber-400/30 bg-amber-400/5 px-2 py-1 text-[10px] text-amber-600 dark:text-amber-400">
                {t("depwork.previewTruncatedText", { defaultValue: "文件过大，仅显示前 200,000 字符" })}
              </p>
            )}
            {category === "markdown" ? (
              <MarkdownRenderer content={textContent} />
            ) : (
              <pre className="whitespace-pre-wrap break-words font-mono text-[11px] leading-relaxed text-muted-foreground">
                {textContent}
              </pre>
            )}
          </>
        ) : category === "pdf" ? (
          <PdfSection filePath={selectedFile.path} />
        ) : (
          <div className="flex flex-col items-center justify-center py-12 text-center">
            <FileIcon className="mb-2 h-8 w-8 text-muted-foreground/30" />
            <p className="text-[11px] text-muted-foreground">
              {t("depwork.previewUnsupported")}
            </p>
            <p className="mt-1 text-[10px] text-muted-foreground/60">
              {t("depwork.previewUseAgent")}
            </p>
          </div>
        )}
      </div>
    </ScrollArea>
  );
}
