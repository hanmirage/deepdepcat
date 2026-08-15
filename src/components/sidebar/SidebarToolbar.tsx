/**
 * SidebarToolbar — top section of the sidebar.
 *
 * Layout:
 * ┌────────────────────────────────────┐
 * │ [🔍 搜索...          ⌘/Ctrl+K]    │  Search input
 * │ [+ 新建对话]          [⏰] [⚡]    │  Primary CTA + notifications
 * └────────────────────────────────────┘
 *
 * Search input is controlled — parent owns the query state. The Ctrl/Cmd+K
 * shortcut lives in the PARENT (Sidebar), so it survives sidebar collapse;
 * this component only renders the hint (platform-adaptive).
 * New Chat starts a fresh conversation in the CURRENT product mode.
 */

import type { Ref } from "react";
import { Search, Plus, Clock } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { NotificationBell } from "@/components/sidebar/NotificationBell";
import { useAppStore } from "@/stores/appStore";
import { cn } from "@/lib/utils";

export interface SidebarToolbarProps {
  searchQuery: string;
  onSearchChange: (value: string) => void;
  onNewTask: () => void;
  /** Search input ref — the parent owns the Ctrl/Cmd+K shortcut. */
  inputRef?: Ref<HTMLInputElement>;
  className?: string;
}

export function SidebarToolbar({
  searchQuery,
  onSearchChange,
  onNewTask,
  inputRef,
  className,
}: SidebarToolbarProps) {
  const { t } = useTranslation();
  const openScheduled = useAppStore((s) => s.openScheduled);

  return (
    <div className={cn("space-y-2 px-2.5 pt-3", className)}>
      {/* ── Search input with Ctrl+K hint ───────────────────── */}
      <div className="relative">
        <Search className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
        <Input
          ref={inputRef}
          type="text"
          aria-label={t("sidebar.searchPlaceholder")}
          placeholder={t("sidebar.searchPlaceholder")}
          value={searchQuery}
          onChange={(e) => onSearchChange(e.target.value)}
          className="h-8 border-border/60 bg-muted/40 pl-8 pr-12 text-xs shadow-none focus-visible:bg-background"
        />
        {/* Platform-adaptive hint: ⌘K on macOS, Ctrl+K elsewhere. */}
        <kbd className="absolute right-2 top-1/2 -translate-y-1/2 rounded border border-border/60 bg-muted px-1.5 py-0.5 text-[9px] font-medium text-muted-foreground">
          {navigator.platform.toLowerCase().includes("mac") ? "⌘K" : "Ctrl K"}
        </kbd>
      </div>

      {/* ── New Task button + notification bell ─────────────── */}
      <div className="flex items-center gap-1.5">
        <Button
          size="sm"
          className="h-8 flex-1 justify-start gap-2 text-xs font-medium"
          onClick={onNewTask}
        >
          <Plus className="h-3.5 w-3.5" />
          {t("sidebar.newChat")}
        </Button>
        <Button
          size="icon"
          variant="ghost"
          className="h-8 w-8 shrink-0 text-muted-foreground hover:text-foreground"
          onClick={openScheduled}
          title={t("sidebar.scheduledTasks")}
          aria-label={t("sidebar.scheduledTasks")}
        >
          <Clock className="h-4 w-4" />
        </Button>
        <NotificationBell />
      </div>
    </div>
  );
}
