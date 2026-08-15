/**
 * PreviewPanel — right-panel file preview for DepworkView.
 *
 * Shows a header (file name, path, badges, external actions) and the file's
 * content via FilePreviewBody. A full-screen expand button opens the same
 * content in a Claude-Preview-style overlay dialog (FilePreviewDialog) — the
 * conversation pane is never reflowed, the overlay just covers it.
 *
 * Reads selectedFile from depworkStore.
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  FileText,
  ExternalLink,
  FolderOpen,
  Maximize2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { useDepworkStore } from "@/stores/depworkStore";
import { workspaceFileApi, isTauri } from "@/lib/tauri";
import { logError } from "@/lib/logger";
import { cn } from "@/lib/utils";
import {
  categorize,
  getCategoryIcon,
  getExt,
  formatSize,
  FilePreviewBody,
} from "@/components/depwork/viewers/FilePreviewBody";
import { FilePreviewDialog } from "@/components/depwork/viewers/FilePreviewDialog";

export interface PreviewPanelProps {
  className?: string;
}

export function PreviewPanel({ className }: PreviewPanelProps) {
  const { t } = useTranslation();
  const selectedFile = useDepworkStore((s) => s.selectedFile);
  const [actionError, setActionError] = useState<string | null>(null);
  const [previewOpen, setPreviewOpen] = useState(false);

  const category = selectedFile ? categorize(selectedFile.name) : "unknown";
  const Icon = getCategoryIcon(category);

  const openExternal = async (reveal: boolean) => {
    if (!selectedFile) return;
    setActionError(null);
    try {
      if (reveal) {
        await workspaceFileApi.reveal(selectedFile.path);
      } else {
        await workspaceFileApi.open(selectedFile.path);
      }
    } catch (e) {
      logError("PreviewPanel", "Failed to open file externally:", e);
      setActionError(t("depwork.previewOpenFail"));
    }
  };

  // ── No file selected ───────────────────────────────────────
  if (!selectedFile) {
    return (
      <div className={cn("flex h-full flex-col", className)}>
        <div className="flex flex-1 flex-col items-center justify-center px-6 text-center">
          <FileText className="mb-3 h-10 w-10 text-muted-foreground/30" />
          <p className="text-xs text-muted-foreground">{t("depwork.previewSelectFile")}</p>
        </div>
      </div>
    );
  }

  return (
    <div className={cn("flex h-full flex-col", className)}>
      {/* ── File header ─────────────────────────────────────── */}
      <div className="border-b border-border p-3">
        <div className="flex items-center gap-2">
          <div className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-secondary">
            <Icon className="h-4 w-4 text-muted-foreground" />
          </div>
          <div className="min-w-0 flex-1">
            <p className="truncate text-xs font-medium">{selectedFile.name}</p>
            <p className="truncate text-[10px] text-muted-foreground">
              {selectedFile.path}
            </p>
          </div>
        </div>

        {/* File metadata badges */}
        <div className="mt-2 flex flex-wrap gap-1.5">
          <Badge variant="secondary" className="text-[9px]">
            {getExt(selectedFile.name).toUpperCase() || "FILE"}
          </Badge>
          {selectedFile.size !== null && (
            <Badge variant="secondary" className="text-[9px]">
              {formatSize(selectedFile.size)}
            </Badge>
          )}
        </div>

        {/* External actions — open with the system app or locate the file */}
        {isTauri && (
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            <Button
              variant="outline"
              size="sm"
              className="h-7 gap-1 text-[10px]"
              onClick={() => void openExternal(false)}
            >
              <ExternalLink className="h-3 w-3" />
              {t("depwork.previewOpenExternal")}
            </Button>
            <Button
              variant="outline"
              size="sm"
              className="h-7 gap-1 text-[10px]"
              onClick={() => void openExternal(true)}
            >
              <FolderOpen className="h-3 w-3" />
              {t("depwork.previewRevealInFolder")}
            </Button>
            {/* Full-screen expand — Claude-Preview-style overlay, never reflows
                the conversation pane. */}
            <Button
              variant="outline"
              size="sm"
              className="h-7 gap-1 text-[10px]"
              onClick={() => setPreviewOpen(true)}
              title={t("depwork.previewExpand")}
            >
              <Maximize2 className="h-3 w-3" />
              {t("depwork.previewExpand")}
            </Button>
            {actionError && (
              <span className="text-[10px] text-destructive">{actionError}</span>
            )}
          </div>
        )}
      </div>

      {/* ── Preview content ─────────────────────────────────── */}
      <FilePreviewBody selectedFile={selectedFile} />

      {/* ── Full-screen overlay ─────────────────────────────── */}
      <FilePreviewDialog
        open={previewOpen}
        onOpenChange={setPreviewOpen}
        file={selectedFile}
      />
    </div>
  );
}
