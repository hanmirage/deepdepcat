/**
 * FileTree — left-panel file browser for DepworkView.
 *
 * Features:
 * - "Open Folder" button (Tauri native dialog)
 * - Recursive tree with lazy-loaded directories (expand on click)
 * - File-type icons (.pdf, .docx, .xlsx, .md, .txt, images)
 * - Click file → preview + attach to chat context
 * - Highlights files currently attached to chat context
 *
 * All state lives in depworkStore; chat context via depworkChatStore.
 */

import {
  FolderOpen,
  Folder,
  ChevronRight,
  ChevronDown,
  FileText,
  Table,
  FileCode,
  Image as ImageIcon,
  File,
  Loader2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useDepworkStore, type FileTreeNode } from "@/stores/depworkStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import { useTranslation } from "react-i18next";

/** Map file extension to icon. */
function getFileIcon(name: string): LucideIcon {
  const ext = name.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "pdf":
    case "txt":
      return FileText;
    case "doc":
    case "docx":
      return FileText;
    case "xls":
    case "xlsx":
      return Table;
    case "md":
    case "markdown":
      return FileCode;
    case "png":
    case "jpg":
    case "jpeg":
    case "gif":
    case "svg":
    case "webp":
      return ImageIcon;
    default:
      return File;
  }
}

/** Check if a file path is currently attached as a context chip. Matched by
 *  PATH, never by name — two files with the same name in different folders
 *  must not be confused with each other. Separators are normalized so a
 *  native-picker chip path (`C:\a\b`) matches the tree's `/`-joined path. */
function normalizePath(p: string): string {
  return p.replace(/\\/g, "/").toLowerCase();
}
function isAttached(chips: { name: string; path?: string }[], node: FileTreeNode): boolean {
  return chips.some((c) => c.path && normalizePath(c.path) === normalizePath(node.path));
}

/** Fold the indentation after 12 levels so deep trees keep their text on
 *  screen instead of overflowing the panel's horizontal bounds. */
const MAX_TREE_DEPTH = 12;

export interface FileTreeProps {
  className?: string;
}

export function FileTree({ className }: FileTreeProps) {
  const { t } = useTranslation();
  const rootPath = useDepworkStore((s) => s.rootPath);
  const tree = useDepworkStore((s) => s.tree);
  const treeLoading = useDepworkStore((s) => s.treeLoading);
  const openFolder = useDepworkStore((s) => s.openFolder);
  const toggleDirectory = useDepworkStore((s) => s.toggleDirectory);
  const selectFile = useDepworkStore((s) => s.selectFile);
  const selectedFile = useDepworkStore((s) => s.selectedFile);
  const chips = useDepworkChatStore((s) => s.contextChips);
  const addContextChip = useDepworkChatStore((s) => s.addContextChip);
  const removeContextChip = useDepworkChatStore((s) => s.removeContextChip);

  const handleFileClick = (node: FileTreeNode) => {
    selectFile(node);
    const attached = isAttached(chips, node);
    if (attached) {
      // Already attached — clicking again detaches (symmetric, so the tree
      // is never a one-way trap into the chips row).
      const chip = chips.find((c) => c.path && normalizePath(c.path) === normalizePath(node.path));
      if (chip) removeContextChip(chip.id);
    } else {
      addContextChip({
        id: `file-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
        type: "file",
        name: node.name,
        path: node.path,
      });
    }
  };

  const renderNode = (node: FileTreeNode, depth: number): React.ReactNode => {
    const isSelected = selectedFile?.path === node.path;
    const attached = isAttached(chips, node);
    const indentStyle = { '--tree-depth': Math.min(depth, MAX_TREE_DEPTH) } as React.CSSProperties;

    if (node.isDir) {
      return (
        <div key={node.path}>
          <button
            onClick={() => toggleDirectory(node)}
            style={indentStyle}
            aria-expanded={node.expanded}
            className={cn(
              "flex w-full items-center gap-1 py-1 pr-2 pl-[calc(var(--tree-depth)*12px+8px)] text-left text-xs transition-colors",
              "hover:bg-secondary/60",
            )}
          >
            {node.expanded ? (
              <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" />
            ) : (
              <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground" />
            )}
            {node.expanded ? (
              <FolderOpen className="h-3.5 w-3.5 shrink-0 text-primary/70" />
            ) : (
              <Folder className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
            )}
            <span className="truncate text-foreground">{node.name}</span>
          </button>
          {node.expanded && node.children && (
            <div>
              {node.children.map((child) => renderNode(child, depth + 1))}
            </div>
          )}
        </div>
      );
    }

    const Icon = getFileIcon(node.name);

    return (
      <button
        key={node.path}
        onClick={() => handleFileClick(node)}
        style={indentStyle}
        className={cn(
          "flex w-full items-center gap-1 py-1 pr-2 pl-[calc(var(--tree-depth)*12px+8px)] text-left text-xs transition-colors",
          isSelected && "bg-primary/10",
          !isSelected && "hover:bg-secondary/60",
        )}
      >
        <span className="w-3 shrink-0" />
        <Icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        <span className={cn("truncate", attached && "text-primary font-medium")}>
          {node.name}
        </span>
        {attached && (
          <span className="ml-auto shrink-0 text-primary" title={t("depwork.fileAttached", { defaultValue: "已附加到对话，点击取消" })}>
            ✓
          </span>
        )}
      </button>
    );
  };

  return (
    <div className={cn("flex h-full flex-col", className)}>
      {/* ── Header ───────────────────────────────────────────── */}
      <div className="border-b border-border p-2">
        <Button
          variant="outline"
          size="sm"
          className="w-full gap-1.5 text-xs"
          onClick={openFolder}
        >
          <FolderOpen className="h-3.5 w-3.5" />
          {t("depwork.openFolder")}
        </Button>
      </div>

      {/* ── Tree ─────────────────────────────────────────────── */}
      <ScrollArea className="flex-1">
        <div className="py-1">
          {treeLoading ? (
            <div className="flex items-center justify-center py-8">
              <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
            </div>
          ) : !rootPath ? (
            <div className="px-3 py-8 text-center">
              <Folder className="mx-auto mb-2 h-8 w-8 text-muted-foreground/30" />
              <p className="text-[11px] text-muted-foreground">
                {t("depwork.clickToOpen")}
              </p>
            </div>
          ) : tree.length === 0 ? (
            <div className="px-3 py-8 text-center">
              <p className="text-[11px] text-muted-foreground">{t("depwork.folderEmpty")}</p>
            </div>
          ) : (
            tree.map((node) => renderNode(node, 0))
          )}
        </div>
      </ScrollArea>

      {/* ── Root path display ────────────────────────────────── */}
      {rootPath && (
        <div className="border-t border-border px-2 py-1.5">
          <p className="truncate text-[10px] text-muted-foreground" title={rootPath}>
            {rootPath}
          </p>
        </div>
      )}
    </div>
  );
}
