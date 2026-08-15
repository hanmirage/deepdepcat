/**
 * ChangesSummaryCard — end-of-turn file-change summary.
 *
 * VS Code `showFileChanges`-style aggregation: one compact card listing
 * every file this turn edited, with +/- line stats per file and an
 * expandable unified diff per file. The result view (what changed) takes
 * over from the process view (each tool card) — the tool cards stay, but
 * the summary is the single place to review the outcome.
 */

import { useMemo, useState } from "react";
import * as Collapsible from "@radix-ui/react-collapsible";
import { ChevronDown, ChevronRight, Files } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import { computeDiffStats } from "@/lib/diffStats";
import { FileDiffPreview } from "@/components/chat/FileDiffPreview";
import type { FileChange } from "@/types";

/** Total added/removed lines across all files. */
function totalStats(changes: FileChange[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const c of changes) {
    const stats = computeDiffStats(
      c.oldText ? "edit_file" : "write_file",
      JSON.stringify(
        c.oldText
          ? { path: c.path, old_text: c.oldText, new_text: c.newText }
          : { path: c.path, content: c.newText },
      ),
    );
    if (stats) {
      added += stats.added;
      removed += stats.removed;
    }
  }
  return { added, removed };
}

/** Short display path — last two segments (keeps the list scannable). */
function shortPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.length > 2 ? parts.slice(-2).join("/") : path;
}

function ChangeRow({ change }: { change: FileChange }) {
  const [open, setOpen] = useState(false);
  const stats = computeDiffStats(
    change.oldText ? "edit_file" : "write_file",
    JSON.stringify(
      change.oldText
        ? { path: change.path, old_text: change.oldText, new_text: change.newText }
        : { path: change.path, content: change.newText },
    ),
  );

  return (
    <Collapsible.Root open={open} onOpenChange={setOpen}>
      <div className="group flex w-full items-center gap-1.5">
        <Collapsible.Trigger asChild>
          <button className="flex min-w-0 flex-1 items-center gap-1.5 py-1 text-left hover:bg-muted/40">
            <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-foreground/85">
              {shortPath(change.path)}
            </span>
            {stats && (
              <span className="shrink-0 font-mono text-[10px]">
                {stats.added > 0 && (
                  <span className="text-green-600 dark:text-green-400">+{stats.added}</span>
                )}
                {stats.removed > 0 && (
                  <span className="ml-1 text-red-500 dark:text-red-400">-{stats.removed}</span>
                )}
              </span>
            )}
            <ChevronRight
              className={cn(
                "h-3 w-3 shrink-0 text-muted-foreground/40 transition-transform",
                open && "rotate-90",
              )}
            />
          </button>
        </Collapsible.Trigger>
      </div>
      <Collapsible.Content className="pb-1 pl-2">
        <FileDiffPreview filePath={change.path} oldText={change.oldText} newText={change.newText} />
      </Collapsible.Content>
    </Collapsible.Root>
  );
}

export function ChangesSummaryCard({ changes }: { changes: FileChange[] }) {
  const { t } = useTranslation();
  const [collapsed, setCollapsed] = useState(false);
  const [showAll, setShowAll] = useState(false);
  const stats = useMemo(() => totalStats(changes), [changes]);
  const visible = showAll ? changes : changes.slice(0, 10);
  const hiddenCount = changes.length - visible.length;

  if (changes.length === 0) return null;

  return (
    <div className="paper-settle overflow-hidden rounded-lg border border-border/70 bg-card/60">
      <button
        onClick={() => setCollapsed(!collapsed)}
        className="flex w-full items-center justify-between gap-2 border-b border-border/50 px-3 py-2 text-left hover:bg-muted/40"
        aria-expanded={!collapsed}
      >
        <div className="flex items-center gap-2">
          <Files className="h-4 w-4 shrink-0 text-primary/80" />
          <span className="text-xs font-medium text-foreground">
            {t("chat.changesSummary", { defaultValue: "本轮改动" })}
          </span>
          <span className="text-[10px] text-muted-foreground">
            {t("chat.changesFiles", { defaultValue: "{{count}} 个文件", count: changes.length })}
          </span>
          <span className="flex items-center gap-1 font-mono text-[10px]">
            {stats.added > 0 && (
              <span className="text-green-600 dark:text-green-400">+{stats.added}</span>
            )}
            {stats.removed > 0 && (
              <span className="text-red-500 dark:text-red-400">-{stats.removed}</span>
            )}
          </span>
        </div>
        {collapsed ? (
          <ChevronRight className="h-3.5 w-3.5 text-muted-foreground/60" />
        ) : (
          <ChevronDown className="h-3.5 w-3.5 text-muted-foreground/60" />
        )}
      </button>
      {!collapsed && (
        <div className="max-h-[50vh] overflow-y-auto px-2 py-1">
          {visible.map((change) => (
            <ChangeRow key={change.path} change={change} />
          ))}
          {hiddenCount > 0 && (
            <button
              onClick={() => setShowAll(true)}
              className="w-full rounded px-2 py-1 text-left text-[11px] text-muted-foreground transition-colors hover:bg-muted/40 hover:text-foreground"
            >
              {t("chat.changesMore", {
                defaultValue: "还有 {{count}} 个文件",
                count: hiddenCount,
              })}
            </button>
          )}
        </div>
      )}
    </div>
  );
}
