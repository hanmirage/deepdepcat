/**
 * ArtifactCard — a document artifact produced this turn.
 *
 * Rendered in flow position right after the tool that generated it: a compact
 * card with the product's extension badge, file name, and an open-in-preview
 * action. `revealFile` opens the right panel and selects the file; in depwork
 * the file is also loaded into the preview panel directly, so "打开预览" lands
 * on the document. Paper-settle entrance (stage 3) — the deliverable "lands".
 */

import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import {
  FileText,
  FileSpreadsheet,
  Presentation,
  Globe,
  Eye,
  File as FileIcon,
  type LucideIcon,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useDepworkStore } from "@/stores/depworkStore";
import { isTauri } from "@/lib/tauri";
import type { MessageBlock } from "@/types";

type ArtifactBlock = Extract<MessageBlock, { type: "artifact" }>;

/** Per-extension visual identity — icon + accent class. */
const EXT_META: Record<string, { icon: LucideIcon; accent: string }> = {
  docx: { icon: FileText, accent: "text-sky-600 dark:text-sky-400" },
  pdf: { icon: FileText, accent: "text-red-500 dark:text-red-400" },
  xlsx: { icon: FileSpreadsheet, accent: "text-green-600 dark:text-green-400" },
  pptx: { icon: Presentation, accent: "text-orange-500 dark:text-orange-400" },
  html: { icon: Globe, accent: "text-primary" },
};

function artifactMeta(ext: string): { icon: LucideIcon; accent: string } {
  return EXT_META[ext] ?? { icon: FileIcon, accent: "text-muted-foreground" };
}

export function ArtifactCard({ artifact }: { artifact: ArtifactBlock }) {
  const { t } = useTranslation();
  const { name, path } = artifact;
  const ext = useMemo(
    () => path.split(".").pop()?.toLowerCase() ?? "",
    [path],
  );
  const { icon: Icon, accent } = artifactMeta(ext);

  const openPreview = () => {
    if (!isTauri) return;
    const mode = useAppStore.getState().mode;
    if (mode === "depwork") {
      useDepworkStore.getState().selectFile({ name, path, isDir: false, size: null });
    }
    useRightPanelStore.getState().revealFile(mode, path);
  };

  return (
    <div className="paper-settle my-2 flex items-center gap-3 rounded-lg border border-border/70 bg-card/60 px-3 py-2">
      <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-primary/10">
        <Icon className={cn("h-4 w-4", accent)} />
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground/70">
            {t("chat.artifactLabel", { defaultValue: "文档产物" })}
          </span>
          {ext && (
            <span className="rounded bg-primary/10 px-1 py-px font-mono text-[9px] uppercase leading-none text-primary/80">
              {ext}
            </span>
          )}
        </div>
        <p className="mt-0.5 truncate text-xs font-medium text-foreground" title={path}>
          {name}
        </p>
      </div>
      <Button
        size="sm"
        variant="secondary"
        className="h-7 shrink-0 gap-1 px-2 text-[11px]"
        onClick={openPreview}
        disabled={!isTauri}
      >
        <Eye className="h-3 w-3" />
        {t("chat.openPreview", { defaultValue: "打开预览" })}
      </Button>
    </div>
  );
}
