/**
 * DepworkFolderSelector — document-directory picker for the Depwork welcome.
 *
 * Binds to depworkStore.rootPath (the depwork document folder) — NOT the Code
 * workspace (appStore.workspacePath). The chosen directory is surfaced ONLY by
 * this button's label (and the right panel's Workspace tab): it is NOT
 * attached as an input-box context chip (that was a duplicate yellow chip —
 * the directory is already visible here). The agent still receives the
 * directory because sendMessage injects rootPath as a folder chip per turn.
 */

import { useCallback } from "react";
import { useTranslation } from "react-i18next";
import { ChevronDown, Folder, FolderOpen, X } from "lucide-react";
import type { TFunction } from "i18next";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Button } from "@/components/ui/button";
import { useDepworkStore } from "@/stores/depworkStore";
import { isTauri } from "@/lib/tauri";
import { cn } from "@/lib/utils";

/** Trigger button — the current folder name, or the empty "select" label. */
function folderTrigger(t: TFunction, rootPath: string | null) {
  const displayName = rootPath
    ? rootPath.split(/[\\/]/).pop() ?? rootPath
    : null;
  return (
    <Button
      variant="ghost"
      size="sm"
      className={cn(
        "gap-1.5 h-7 px-2.5 text-xs",
        rootPath
          ? "text-primary hover:bg-primary/10"
          : "text-muted-foreground hover:bg-muted",
      )}
      aria-label={t("depwork.selectDocumentDir")}
    >
      {displayName ? (
        <>
          <FolderOpen className="h-3.5 w-3.5" />
          <span className="max-w-[160px] truncate">{displayName}</span>
        </>
      ) : (
        <>
          <Folder className="h-3.5 w-3.5" />
          <span>{t("depwork.selectDocumentDir")}</span>
        </>
      )}
      <ChevronDown className="h-3 w-3 text-muted-foreground" />
    </Button>
  );
}

/** Dropdown body — current folder info, browse, and close actions. */
function FolderMenuContent({
  rootPath,
  onBrowse,
  onClose,
}: {
  rootPath: string | null;
  onBrowse: () => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <DropdownMenuContent align="start" className="w-72">
      {rootPath && (
        <>
          <div className="px-2 py-1.5">
            <p className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
              {t("depwork.currentDocumentDir")}
            </p>
            <p
              className="mt-0.5 truncate text-xs text-foreground"
              title={rootPath}
            >
              {rootPath}
            </p>
          </div>
          <DropdownMenuSeparator />
        </>
      )}
      <DropdownMenuItem
        onClick={onBrowse}
        className="flex items-center gap-2 text-xs"
      >
        <FolderOpen className="h-3.5 w-3.5" />
        {t("depwork.browseDocumentDir")}
      </DropdownMenuItem>
      {rootPath && (
        <>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            onClick={onClose}
            className="flex items-center gap-2 text-xs text-destructive"
          >
            <X className="h-3.5 w-3.5" />
            {t("depwork.closeDocumentDir")}
          </DropdownMenuItem>
        </>
      )}
    </DropdownMenuContent>
  );
}

export function DepworkFolderSelector() {
  const { t } = useTranslation();
  const rootPath = useDepworkStore((s) => s.rootPath);
  const openFolder = useDepworkStore((s) => s.openFolder);
  const clearFolder = useDepworkStore((s) => s.clearFolder);

  const handleBrowse = useCallback(async () => {
    if (!isTauri) return;
    await openFolder();
  }, [openFolder]);

  const handleClose = useCallback(() => {
    clearFolder();
  }, [clearFolder]);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        {folderTrigger(t, rootPath)}
      </DropdownMenuTrigger>
      <FolderMenuContent
        rootPath={rootPath}
        onBrowse={() => void handleBrowse()}
        onClose={handleClose}
      />
    </DropdownMenu>
  );
}
