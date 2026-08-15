/**
 * SidebarFilterBar — compact session filter row.
 *
 * A single "筛选" button cycles the session filter:
 *   all → active → all
 *
 * Note: an "archived" state existed in the original design, but the app has no
 * archive action/backend call, so archived sessions can never exist — the
 * archive filter was dead UI and is intentionally not offered.
 */

import { SlidersHorizontal } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import type { SessionFilter } from "@/hooks/useSessionList";

export interface SidebarFilterBarProps {
  filter: SessionFilter;
  onFilterChange: (filter: SessionFilter) => void;
  className?: string;
}

export function SidebarFilterBar({ filter, onFilterChange, className }: SidebarFilterBarProps) {
  const { t } = useTranslation();

  // Cycle all → active → all (archived has no producer, so it's not offered).
  const handleFilterClick = () => {
    const next: SessionFilter = filter === "all" ? "active" : "all";
    onFilterChange(next);
  };

  const filterLabel = filter === "all" ? t("sidebar.filterAll") : t("sidebar.filterActive");

  return (
    <div className={cn("flex items-center px-2.5 py-1", className)}>
      <button
        onClick={handleFilterClick}
        aria-pressed={filter !== "all"}
        title={filter === "all" ? t("sidebar.filterShowActive", { defaultValue: "显示活跃会话" }) : t("sidebar.filterShowAll", { defaultValue: "显示全部会话" })}
        className={cn(
          "flex items-center gap-1.5 rounded-md px-2 py-1 text-[11px] transition-colors",
          filter !== "all"
            ? "bg-secondary text-foreground"
            : "text-muted-foreground/70 hover:bg-secondary/40 hover:text-muted-foreground",
        )}
      >
        <SlidersHorizontal className="h-3 w-3" />
        {filterLabel}
      </button>
    </div>
  );
}
