/**
 * DocumentContextCard — the depwork session's attached documents as a
 * right-panel section (papers, files, folders, URLs attached to the input).
 *
 * Mirror of the ContextChips strip above the input, but in section form:
 * type-colored icon stamp + file name + full path + hover-to-remove.
 * Hidden entirely when nothing is attached (absolute-clean panel).
 */

import { FileText, Folder, Globe, X, type LucideIcon } from "lucide-react";
import { useTranslation } from "react-i18next";
import { SectionHeader } from "@/components/customize/panelParts";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { cn } from "@/lib/utils";

const TYPE_CONFIG: Record<string, { icon: LucideIcon; color: string; bg: string }> = {
  file: {
    icon: FileText,
    color: "text-blue-600 dark:text-blue-400",
    bg: "bg-blue-500/10",
  },
  folder: {
    icon: Folder,
    color: "text-amber-600 dark:text-amber-400",
    bg: "bg-amber-500/10",
  },
  url: {
    icon: Globe,
    color: "text-purple-600 dark:text-purple-400",
    bg: "bg-purple-500/10",
  },
  paper: {
    icon: FileText,
    color: "text-emerald-600 dark:text-emerald-400",
    bg: "bg-emerald-500/10",
  },
};

export function DocumentContextCard() {
  const { t } = useTranslation();
  const chips = useDepworkChatStore((s) => s.contextChips);
  const removeContextChip = useDepworkChatStore((s) => s.removeContextChip);

  // Absolute-clean: no documents attached → no section at all.
  if (chips.length === 0) return null;

  return (
    <div className="space-y-1.5">
      <SectionHeader
        icon={FileText}
        label={t("depworkContext.title")}
        count={chips.length}
      />
      <div className="space-y-1">
        {chips.map((chip) => {
          const config = TYPE_CONFIG[chip.type] ?? TYPE_CONFIG.file;
          const Icon = config.icon;
          return (
            <div
              key={chip.id}
              className="group flex items-center gap-2 rounded-md border border-border/60 bg-background/60 px-2 py-1.5"
            >
              <span
                className={cn(
                  "flex h-6 w-6 shrink-0 items-center justify-center rounded-md",
                  config.bg,
                )}
              >
                <Icon className={cn("h-3.5 w-3.5", config.color)} />
              </span>
              <div className="min-w-0 flex-1">
                <p className="truncate text-[11px] font-medium text-foreground/90">
                  {chip.name}
                </p>
                <p className="truncate font-mono text-[9px] text-muted-foreground/60">
                  {chip.path}
                </p>
              </div>
              <button
                onClick={() => removeContextChip(chip.id)}
                className="shrink-0 rounded p-0.5 text-muted-foreground/50 opacity-0 transition-opacity hover:bg-muted hover:text-foreground group-hover:opacity-100"
                title={t("common.delete")}
                aria-label={t("common.delete")}
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}
