/**
 * WorkspaceFilesPanel — Code-mode right-panel "files" page.
 *
 * A dedicated page for browsing the current project's files with a
 * lightweight text / markdown / image preview. Folders are listed but not
 * navigable here — opening projects stays in the sidebar selector.
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  File as FileIcon,
  Folder,
  FolderOpen,
  Loader2,
  RefreshCw,
} from "lucide-react";
import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import {
  isTauri,
  readWorkspaceTextFile,
  toAssetUrl,
  type WorkspaceFileEntry,
} from "@/lib/tauri";
import { MarkdownRenderer } from "@/components/chat/MarkdownRenderer";
import { EmptyHint, SectionHeader } from "@/components/customize/panelParts";
import { logError } from "@/lib/logger";
import { cn } from "@/lib/utils";

const MAX_PREVIEW_CHARS = 200_000;

function extOf(name: string): string {
  return name.split(".").pop()?.toLowerCase() ?? "";
}

type PreviewKind = "markdown" | "text" | "image" | "unsupported";

function previewKind(name: string): PreviewKind {
  const ext = extOf(name);
  if (ext === "md" || ext === "markdown") return "markdown";
  if (["png", "jpg", "jpeg", "gif", "svg", "webp", "bmp"].includes(ext)) return "image";
  if (ext === "") return "unsupported";
  return "text";
}

function WorkspaceFileRow({
  file,
  active,
  onSelect,
}: {
  file: WorkspaceFileEntry;
  active: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onSelect}
      disabled={file.isDir}
      className={cn(
        "flex w-full items-center gap-1.5 rounded px-2 py-1 text-left text-[11px] transition-colors",
        active
          ? "bg-primary/10 text-foreground"
          : "text-foreground/80 hover:bg-muted/50",
        file.isDir && "cursor-default text-muted-foreground",
      )}
    >
      {file.isDir ? (
        <Folder className="h-3 w-3 shrink-0 text-amber-600/80" />
      ) : (
        <FileIcon className="h-3 w-3 shrink-0 text-muted-foreground/60" />
      )}
      <span className="min-w-0 flex-1 truncate">{file.name}</span>
    </button>
  );
}

function WorkspaceFilePreview({ path, name }: { path: string; name: string }) {
  const { t } = useTranslation();
  const [preview, setPreview] = useState<{
    kind: PreviewKind;
    content?: string;
    url?: string;
    truncated?: boolean;
  } | null>(null);

  useEffect(() => {
    const kind = previewKind(name);
    if (kind === "unsupported") {
      setPreview({ kind });
      return;
    }
    if (kind === "image") {
      setPreview({ kind, url: toAssetUrl(path) });
      return;
    }
    let alive = true;
    setPreview({ kind });
    readWorkspaceTextFile(path)
      .then((content) => {
        if (!alive) return;
        const truncated = content.length > MAX_PREVIEW_CHARS;
        setPreview({
          kind,
          content: truncated ? content.slice(0, MAX_PREVIEW_CHARS) : content,
          truncated,
        });
      })
      .catch((e) => {
        logError("WorkspaceFilesPanel", "Failed to preview file:", e);
        if (alive) setPreview({ kind: "unsupported" });
      });
    return () => {
      alive = false;
    };
  }, [path, name]);

  if (!preview) {
    return (
      <div className="flex items-center justify-center py-6">
        <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
      </div>
    );
  }
  if (preview.kind === "unsupported") {
    return (
      <p className="px-3 py-3 text-[10px] text-muted-foreground/60">
        {t("rightPanel.artifactsPreviewUnsupported")}
      </p>
    );
  }
  if (preview.kind === "image") {
    return (
      <div className="flex items-center justify-center p-2">
        <img
          src={preview.url}
          alt={name}
          className="max-h-56 max-w-full rounded border border-border object-contain"
        />
      </div>
    );
  }
  return (
    <div className="h-full overflow-y-auto p-2">
      {preview.truncated && (
        <p className="mb-2 rounded border border-amber-400/30 bg-amber-400/5 px-2 py-1 text-[10px] text-amber-600 dark:text-amber-400">
          {t("rightPanel.artifactsTruncated")}
        </p>
      )}
      {preview.kind === "markdown" ? (
        <MarkdownRenderer content={preview.content ?? ""} />
      ) : (
        <pre className="whitespace-pre-wrap break-words font-mono text-[10.5px] leading-relaxed text-muted-foreground">
          {preview.content ?? ""}
        </pre>
      )}
    </div>
  );
}

function WorkspaceFileBrowser({
  pendingPath,
  onPendingConsumed,
}: {
  pendingPath: string | null;
  onPendingConsumed: () => void;
}) {
  const { t } = useTranslation();
  const workspacePath = useAppStore((s) => s.workspacePath);
  const workspaceFiles = useAppStore((s) => s.workspaceFiles);
  const workspaceLoading = useAppStore((s) => s.workspaceLoading);
  const refreshWorkspaceFiles = useAppStore((s) => s.refreshWorkspaceFiles);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  // A revealed file that isn't in the (single-level) root list — e.g. a
  // nested path like src/main.ts from a chat jump — is previewed directly
  // by its path, so the jump never silently fails just because the browser
  // only lists one level.
  const [directPath, setDirectPath] = useState<string | null>(null);

  // A different workspace invalidates the previous selection/preview.
  useEffect(() => {
    setSelectedPath(null);
    setDirectPath(null);
  }, [workspacePath]);

  // Chat jump: a revealed file selects it in the root list (by exact path
  // or by file name), then consumes the pending request. While the list is
  // still loading, keep the pending request so the jump lands once files
  // arrive. A nested path that the single-level list can't match previews
  // directly instead of being dropped.
  useEffect(() => {
    if (!pendingPath) return;
    if (workspaceLoading) return;
    const match = workspaceFiles.find(
      (f) =>
        !f.isDir &&
        (f.path === pendingPath || f.name === pendingPath.split(/[\\/]/).pop()),
    );
    if (match) {
      setSelectedPath(match.path);
      setDirectPath(null);
    } else {
      setSelectedPath(null);
      setDirectPath(pendingPath);
    }
    onPendingConsumed();
  }, [pendingPath, workspaceFiles, workspaceLoading, onPendingConsumed]);

  const selectedFile =
    workspaceFiles.find((f) => f.path === selectedPath) ?? null;

  return (
    <section className="flex h-full min-h-0 flex-col space-y-2 p-3">
      <SectionHeader
        icon={FolderOpen}
        label={t("rightPanel.artifactsWorkspace")}
        count={workspaceFiles.length}
        action={
          <button
            type="button"
            onClick={() => void refreshWorkspaceFiles()}
            className="rounded p-1 text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground"
            title={t("common.refresh")}
            aria-label={t("common.refresh")}
          >
            <RefreshCw className="h-3 w-3" />
          </button>
        }
      />
      {!workspacePath ? (
        <EmptyHint
          text={t("rightPanel.artifactsWorkspaceEmpty")}
          sub={t("rightPanel.artifactsWorkspaceOpenHint")}
        />
      ) : workspaceLoading ? (
        <div className="flex items-center justify-center py-6">
          <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" />
        </div>
      ) : workspaceFiles.length === 0 ? (
        <EmptyHint text={t("rightPanel.artifactsWorkspaceFilesEmpty")} />
      ) : (
        <div className="min-h-0 flex-1 overflow-hidden rounded-lg border border-border/60 bg-muted/10">
          <div className="flex h-full min-h-0 flex-col">
            <div className="min-h-0 flex-[2] overflow-y-auto p-1">
              {workspaceFiles.map((file) => (
                <WorkspaceFileRow
                  key={file.path}
                  file={file}
                  active={file.path === selectedPath}
                  onSelect={() =>
                    setSelectedPath(
                      file.isDir ? null : file.path === selectedPath ? null : file.path,
                    )
                  }
                />
              ))}
            </div>
            <div className="min-h-0 flex-[3] border-t border-border/60 bg-background/50">
              {selectedFile ? (
                <WorkspaceFilePreview path={selectedFile.path} name={selectedFile.name} />
              ) : directPath ? (
                <WorkspaceFilePreview
                  path={directPath}
                  name={directPath.split(/[\\/]/).pop() ?? directPath}
                />
              ) : (
                <p className="px-3 py-2 text-[10px] text-muted-foreground/50">
                  {t("rightPanel.artifactsSelectFile")}
                </p>
              )}
            </div>
          </div>
        </div>
      )}
    </section>
  );
}

export function WorkspaceFilesPanel() {
  const { t } = useTranslation();
  const mode = useAppStore((s) => s.mode);
  const pendingPath = useRightPanelStore((s) => s.pendingFile[mode]);
  const clearPendingFile = useRightPanelStore((s) => s.clearPendingFile);
  if (!isTauri) {
    return (
      <section className="flex h-full min-h-0 flex-col space-y-2 p-3">
        <SectionHeader
          icon={FolderOpen}
          label={t("rightPanel.artifactsWorkspace")}
          count={0}
        />
        <EmptyHint text={t("rightPanel.filesDesktopOnly")} />
      </section>
    );
  }
  return (
    <WorkspaceFileBrowser
      pendingPath={pendingPath}
      onPendingConsumed={() => clearPendingFile(mode)}
    />
  );
}
