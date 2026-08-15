/**
 * ContextChips — enhanced context chips with labels and improved visuals.
 *
 * Features:
 * - Full label display (not just icon)
 * - Icon + text layout with subtle background
 * - Type-specific colors (file: blue, folder: amber, url: purple, paper: emerald)
 * - Hover shows full path
 * - Smooth remove animation
 */

import { FileText, Folder, Globe, X, Image as ImageIcon, type LucideIcon } from "lucide-react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import type { ContextChip } from "@/types";
import type { DepworkContextChip } from "@/types/depwork";
import { toAssetUrl } from "@/lib/tauri";
import { cn } from "@/lib/utils";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";

export interface ContextChipsProps {
  chips: Array<ContextChip | DepworkContextChip>;
  onRemove: (id: string) => void;
  className?: string;
}

const CHIP_CONFIG: Record<
  string,
  { icon: LucideIcon; color: string; bg: string }
> = {
  file: {
    icon: FileText,
    color: "text-blue-600 dark:text-blue-400",
    bg: "bg-blue-50 dark:bg-blue-500/10 hover:bg-blue-100 dark:hover:bg-blue-500/20",
  },
  folder: {
    icon: Folder,
    color: "text-amber-600 dark:text-amber-400",
    bg: "bg-amber-50 dark:bg-amber-500/10 hover:bg-amber-100 dark:hover:bg-amber-500/20",
  },
  url: {
    icon: Globe,
    color: "text-purple-600 dark:text-purple-400",
    bg: "bg-purple-50 dark:bg-purple-500/10 hover:bg-purple-100 dark:hover:bg-purple-500/20",
  },
  paper: {
    icon: FileText,
    color: "text-emerald-600 dark:text-emerald-400",
    bg: "bg-emerald-50 dark:bg-emerald-500/10 hover:bg-emerald-100 dark:hover:bg-emerald-500/20",
  },
};

/**
 * Shorten name for display.
 */
function shortenName(name: string, maxLen = 20): string {
  if (name.length <= maxLen) return name;
  return name.slice(0, maxLen - 3) + "...";
}

const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico"]);

/** Is this chip a picture file (thumbnail preview instead of an icon)? */
function isImageChip(chip: ContextChip | DepworkContextChip): boolean {
  if (chip.type !== "file") return false;
  const ext = chip.name.split(".").pop()?.toLowerCase() ?? "";
  return IMAGE_EXTS.has(ext);
}

/** Thumbnail with an icon fallback when the image fails to load. */
function ImageThumb({ src, alt }: { src: string; alt: string }) {
  const [failed, setFailed] = useState(false);
  if (failed) {
    return (
      <span className="flex h-4 w-4 shrink-0 items-center justify-center rounded bg-muted">
        <ImageIcon className="h-3 w-3 text-muted-foreground" />
      </span>
    );
  }
  return (
    <img
      src={src}
      alt={alt}
      draggable={false}
      className="h-4 w-4 shrink-0 rounded object-cover"
      onError={() => setFailed(true)}
    />
  );
}

export function ContextChips({ chips, onRemove, className }: ContextChipsProps) {
  const { t } = useTranslation();
  if (chips.length === 0) return null;

  return (
    <div className={cn("flex flex-wrap gap-1.5 px-3 pt-2", className)}>
      {chips.map((chip) => {
        const config = CHIP_CONFIG[chip.type] ?? CHIP_CONFIG.file;
        const Icon = config.icon;
        const isImage = isImageChip(chip);

        return (
          <Tooltip key={chip.id}>
            <TooltipTrigger asChild>
              <span
                className={cn(
                  "group flex items-center gap-1.5 rounded-md px-2 py-1 text-xs",
                  "border border-transparent transition-colors",
                  "cursor-default select-none",
                  config.bg,
                  "hover:border-current/20"
                )}
              >
                {isImage ? (
                  <ImageThumb
                    src={chip.dataUrl ?? toAssetUrl(chip.path)}
                    alt={chip.name}
                  />
                ) : (
                  <Icon className={cn("h-3.5 w-3.5 shrink-0", config.color)} />
                )}
                <span className="max-w-[120px] truncate text-foreground/80">
                  {shortenName(chip.name)}
                </span>
                <button
                  onClick={() => onRemove(chip.id)}
                  className={cn(
                    "ml-0.5 flex h-4 w-4 items-center justify-center rounded",
                    "opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100",
                    "hover:bg-foreground/10 focus-visible:bg-foreground/10 focus-visible:outline-none",
                  )}
                  aria-label={t("chat.removeChip", { name: chip.name })}
                >
                  <X className="h-3 w-3 text-muted-foreground" />
                </button>
              </span>
            </TooltipTrigger>
            <TooltipContent side="top" className="text-[11px] max-w-[300px]">
              <div className="font-medium">{chip.name}</div>
              <div className="text-muted-foreground/70">
                {chip.dataUrl ? t("chat.imageReadyHint") : chip.path}
              </div>
            </TooltipContent>
          </Tooltip>
        );
      })}
    </div>
  );
}
