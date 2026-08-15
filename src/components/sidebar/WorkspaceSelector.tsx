/**
 * WorkspaceSelector — Code-mode "current project" dropdown in the sidebar.
 *
 * Replaces the old Projects tab: the current workspace is one compact row,
 * and the dropdown hosts switching/removing projects, opening a new one,
 * and closing the workspace. Sessions always live in the single
 * conversation list below — no tab layer.
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Check, ChevronDown, FolderOpen, X } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Button } from "@/components/ui/button";
import { useAppStore } from "@/stores/appStore";
import { cn, shortPath } from "@/lib/utils";

/** One project row: select + remove, mirrors the old Projects tab row. */
function WorkspaceRow({
  ws,
  active,
  onSelect,
  onRemove,
}: {
  ws: string;
  active: boolean;
  onSelect: () => void;
  onRemove: () => void;
}) {
  const { t } = useTranslation();
  // Two-step confirm — removing a project also switches the active workspace
  // away, a destructive action that must not fire on a mis-click.
  const [armedRemove, setArmedRemove] = useState(false);
  const name = ws.split(/[\\/]/).pop() ?? ws;

  const handleRemove = () => {
    if (!armedRemove) {
      setArmedRemove(true);
      setTimeout(() => setArmedRemove(false), 3000);
      return;
    }
    setArmedRemove(false);
    onRemove();
  };

  return (
    <div
      className={cn(
        "group flex w-full items-center gap-1.5 rounded-md px-2 py-1",
        active ? "bg-primary/5" : "hover:bg-secondary/40",
      )}
    >
      <button
        type="button"
        onClick={onSelect}
        className="flex min-w-0 flex-1 items-center gap-2 text-left"
        title={ws}
      >
        <FolderOpen
          className={cn(
            "h-3.5 w-3.5 shrink-0",
            active ? "text-primary" : "text-muted-foreground",
          )}
        />
        <span className="min-w-0 flex-1">
          <span
            className={cn(
              "block truncate text-xs font-medium",
              active && "font-semibold",
            )}
          >
            {name}
          </span>
          <span className="block truncate text-[10px] text-muted-foreground">
            {shortPath(ws)}
          </span>
        </span>
        {active && <Check className="h-3 w-3 shrink-0 text-primary" />}
      </button>
      <button
        type="button"
        onClick={handleRemove}
        className={cn(
          "shrink-0 rounded p-0.5 transition-colors",
          armedRemove
            ? "bg-destructive text-destructive-foreground"
            : "text-muted-foreground/50 opacity-0 hover:bg-muted hover:text-destructive group-hover:opacity-100",
        )}
        title={
          armedRemove
            ? t("sidebar.confirmRemoveWorkspace", { defaultValue: "再次点击确认移除" })
            : t("sidebar.removeWorkspace")
        }
        aria-label={
          armedRemove
            ? t("sidebar.confirmRemoveWorkspace", { defaultValue: "再次点击确认移除" })
            : t("sidebar.removeWorkspace")
        }
      >
        {armedRemove ? <Check className="h-3 w-3" /> : <X className="h-3 w-3" />}
      </button>
    </div>
  );
}

/** Dropdown body — project list, browse, and close actions. */
function WorkspaceMenu({
  workspaces,
  workspacePath,
  onSelect,
  onRemove,
  onBrowse,
  onClose,
}: {
  workspaces: string[];
  workspacePath: string | null;
  onSelect: (ws: string) => void;
  onRemove: (ws: string) => void;
  onBrowse: () => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <DropdownMenuContent align="start" className="w-72">
      {workspaces.length > 0 && (
        <>
          <p className="px-2 pb-1 pt-1.5 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
            {t("sidebar.sectionProjects")}
          </p>
          <div className="space-y-0.5 px-1 pb-1">
            {workspaces.map((ws) => (
              <WorkspaceRow
                key={ws}
                ws={ws}
                active={ws === workspacePath}
                onSelect={() => onSelect(ws)}
                onRemove={() => onRemove(ws)}
              />
            ))}
          </div>
          <DropdownMenuSeparator />
        </>
      )}
      <DropdownMenuItem
        onClick={onBrowse}
        className="flex items-center gap-2 text-xs"
      >
        <FolderOpen className="h-3.5 w-3.5" />
        {t("sidebar.openProject")}
      </DropdownMenuItem>
      {workspacePath && (
        <>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            onClick={onClose}
            className="flex items-center gap-2 text-xs text-destructive"
          >
            <X className="h-3.5 w-3.5" />
            {t("sidebar.closeProject")}
          </DropdownMenuItem>
        </>
      )}
    </DropdownMenuContent>
  );
}

export function WorkspaceSelector() {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const workspacePath = useAppStore((s) => s.workspacePath);
  const workspaces = useAppStore((s) => s.workspaces);
  const selectWorkspace = useAppStore((s) => s.selectWorkspace);
  const removeWorkspace = useAppStore((s) => s.removeWorkspace);
  const openWorkspaceDialog = useAppStore((s) => s.openWorkspaceDialog);
  const setWorkspacePath = useAppStore((s) => s.setWorkspacePath);

  const displayName = workspacePath
    ? workspacePath.split(/[\\/]/).pop() ?? workspacePath
    : null;

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <div className="px-2.5 pb-1">
          <Button
            variant="ghost"
            size="sm"
            className={cn(
              "h-7 w-full justify-start gap-1.5 px-2.5 text-xs",
              workspacePath
                ? "text-primary hover:bg-primary/10"
                : "text-muted-foreground hover:bg-muted",
            )}
            aria-label={t("sidebar.openProject")}
          >
            <FolderOpen className="h-3.5 w-3.5 shrink-0" />
            <span className="min-w-0 flex-1 truncate text-left">
              {displayName ?? t("sidebar.noProject")}
            </span>
            <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" />
          </Button>
        </div>
      </DropdownMenuTrigger>
      <WorkspaceMenu
        workspaces={workspaces}
        workspacePath={workspacePath}
        onSelect={(ws) => {
          void selectWorkspace(ws);
          setOpen(false);
        }}
        onRemove={(ws) => void removeWorkspace(ws)}
        onBrowse={() => {
          void openWorkspaceDialog();
          setOpen(false);
        }}
        onClose={() => {
          setWorkspacePath(null);
          setOpen(false);
        }}
      />
    </DropdownMenu>
  );
}
