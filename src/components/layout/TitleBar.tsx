/**
 * Custom window title bar (frameless window).
 *
 * Supports two styles, controlled by settingsStore.general.titleBarStyle:
 *
 * 1. "mac" — macOS traffic lights on the left, centered title.
 *
 *    [🔴 🟡 🟢]        DeepDepCat — Code          [drag region]
 *
 * 2. "windows" — title on the left, window controls on the right.
 *
 *    DeepDepCat — Code                    [─] [□] [✕]
 *                                         min  max close
 *
 * Theme toggle is removed from the title bar (it's in Settings → 常规).
 * The entire bar is a drag region for window movement.
 */

import { Minus, Square, X, Copy, PanelRight, PanelRightClose, Download, Loader2, PackageCheck } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useTodoStore, selectSessionTodos } from "@/stores/todoStore";
import { useWindowControls } from "@/hooks/useWindowControls";
import { ProductMenu } from "@/components/layout/ProductMenu";
import { cn } from "@/lib/utils";
import appIcon from "/icon.png";

export function TitleBar() {
  const { t } = useTranslation();
  const mode = useAppStore((s) => s.mode);
  const isMaximized = useAppStore((s) => s.isMaximized);
  const rightPanelOpen = useRightPanelStore((s) => s.open);
  const toggleRightPanel = useRightPanelStore((s) => s.toggle);
  const activitySignal = useRightPanelStore((s) => s.activitySignal[mode]);
  const clearActivitySignal = useRightPanelStore((s) => s.clearActivitySignal);
  const sidebarCollapsed = useAppStore((s) => s.sidebarCollapsed);
  const toggleSidebar = useAppStore((s) => s.toggleSidebar);
  const updateInfo = useAppStore((s) => s.updateInfo);
  const updateDownloading = useAppStore((s) => s.updateDownloading);
  const updateProgress = useAppStore((s) => s.updateProgress);
  const updateError = useAppStore((s) => s.updateError);
  const silentUpdate = useAppStore((s) => s.silentUpdate);
  const downloadAndInstallUpdate = useAppStore((s) => s.downloadAndInstallUpdate);
  const clearUpdateError = useAppStore((s) => s.clearUpdateError);
  const { minimize, toggleMaximize, close } = useWindowControls();
  const titleBarStyle = useSettingsStore((s) => s.general.titleBarStyle);
  // Task-progress badge — the right panel is the progress home, so the
  // title-bar toggle lights up while the active session has open items.
  const codeSessionId = useChatStore((s) => s.currentSessionId);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);
  const codeStreaming = useChatStore((s) => s.isStreaming);
  const depworkStreaming = useDepworkChatStore((s) => s.isStreaming);
  const activeSessionId = mode === "depwork" ? depworkSessionId : codeSessionId;
  const todos = useTodoStore(selectSessionTodos(activeSessionId));
  const hasLiveTodos = todos.length > 0;

  const downloadFraction =
    updateProgress?.phase === "progress" ? updateProgress.fraction : 0;

  /**
   * Silent-update indicator — a small static badge (no interaction): the
   * update was downloaded in the background and installs on the next exit.
   * "downloading" shows a spinner; "staged" shows a ready badge.
   */
  const SilentUpdateIndicator =
    silentUpdate.state === "downloading" ? (
      <div
        data-no-drag
        className="flex h-7 items-center gap-1.5 rounded-md bg-muted px-2 text-muted-foreground"
        title={t("settings.about.silentDownloading")}
      >
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
      </div>
    ) : silentUpdate.state === "staged" ? (
      <div
        data-no-drag
        className="flex h-7 items-center gap-1.5 rounded-md px-2 text-muted-foreground"
        title={t("settings.about.silentStaged", { version: silentUpdate.version ?? "" })}
      >
        <PackageCheck className="h-3.5 w-3.5 text-primary" />
        {silentUpdate.version && (
          <span className="text-[10px] font-medium text-primary">v{silentUpdate.version}</span>
        )}
      </div>
    ) : null;

  /** Title-bar update button — only visible when a newer version exists. */
  const UpdateButton = updateInfo ? (
    updateDownloading ? (
      <div
        data-no-drag
        className="flex h-7 items-center gap-1.5 rounded-md bg-muted px-2 text-muted-foreground"
        title={
          updateProgress?.phase === "finished"
            ? t("settings.about.updateReady")
            : t("settings.about.downloading")
        }
      >
        {updateProgress?.phase === "finished" ? (
          <Download className="h-3.5 w-3.5 text-primary" />
        ) : (
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
        )}
        {updateProgress?.phase !== "finished" && (
          <span className="text-[10px] tabular-nums">{Math.round(downloadFraction * 100)}%</span>
        )}
      </div>
    ) : (
      <div className="relative">
        <button
          onClick={downloadAndInstallUpdate}
          data-no-drag
          className={cn(
            "group relative flex h-7 items-center gap-1.5 rounded-md px-2 transition-colors",
            "text-primary hover:bg-primary/10",
            updateError ? "text-destructive hover:bg-destructive/10" : "",
          )}
          title={t("settings.about.newVersion") + ` v${updateInfo.version}`}
          aria-label={t("settings.about.downloadAndInstall")}
        >
          <Download className="h-3.5 w-3.5" />
          <span className="text-[10px] font-medium">v{updateInfo.version}</span>
        </button>
        {updateError && (
          // Error bubble — a SIBLING of the button (never nested inside it).
          // Not cleared on hover: moving the mouse toward it to read the
          // message must not make it vanish — explicit close instead.
          <div className="pointer-events-auto absolute right-0 top-7 z-50 flex items-center gap-1.5 whitespace-nowrap rounded bg-background px-2 py-1 text-[10px] text-destructive shadow-md">
            <span className="max-w-64 truncate">{updateError}</span>
            <button
              onClick={clearUpdateError}
              className="shrink-0 rounded p-0.5 text-destructive/70 hover:bg-destructive/10 hover:text-destructive"
              aria-label={t("common.close")}
            >
              <X className="h-3 w-3" />
            </button>
          </div>
        )}
      </div>
    )
  ) : null;

  /** Toolbar buttons shown to the right of the title — window controls flank them. */
  const ToolbarButtons = (
    <>
      {SilentUpdateIndicator}
      {UpdateButton}
      <button
        onClick={() => {
          toggleRightPanel(mode);
          clearActivitySignal(mode);
        }}
        data-no-drag
        className={cn(
          "relative flex h-7 w-7 items-center justify-center rounded-md transition-colors",
          rightPanelOpen
            ? "bg-muted text-foreground"
            : "text-muted-foreground hover:bg-muted/80 hover:text-foreground",
        )}
        title={rightPanelOpen ? t("rightPanel.collapse") : t("rightPanel.toggle")}
        aria-label={rightPanelOpen ? t("rightPanel.collapse") : t("rightPanel.toggle")}
        aria-pressed={rightPanelOpen}
      >
        {rightPanelOpen ? (
          <PanelRightClose className="h-3.5 w-3.5" />
        ) : (
          <PanelRight className="h-3.5 w-3.5" />
        )}
        {(activitySignal || hasLiveTodos) && (
          <span
            className={cn(
              "absolute -right-0.5 -top-0.5 h-2 w-2 rounded-full",
              activitySignal ? "bg-primary animate-pulse" : "bg-primary",
            )}
            title={
              activitySignal
                ? t("rightPanel.activityBadge")
                : t("layout.todoBadge")
            }
            aria-label={
              activitySignal
                ? t("rightPanel.activityBadge")
                : t("layout.todoBadge")
            }
          />
        )}
      </button>
    </>
  );

  // ── macOS style ──────────────────────────────────────────
  if (titleBarStyle === "mac") {
    return (
      <header
        data-tauri-drag-region
        className={cn(
          "no-select relative flex h-10 items-center justify-between px-3",
          "bg-[hsl(var(--titlebar-bg))] shadow-[var(--shadow-paper-sm)]",
        )}
      >
        {/* Left: Traffic lights + icon */}
        <div className="flex items-center gap-2">
          <button
            onClick={toggleSidebar}
            data-no-drag
            className="rounded p-0.5 transition-colors hover:bg-muted/80"
            title={sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
            aria-label={sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
            aria-pressed={!sidebarCollapsed}
          >
            <img src={appIcon} alt="DeepDepCat" className="h-4 w-4 rounded-sm" />
          </button>
        <button
          onClick={close}
          data-no-drag
          className="group flex h-5 w-5 items-center justify-center rounded-full transition-[filter] hover:brightness-110"
          aria-label={t("common.close")}
        >
          <span className="h-3 w-3 rounded-full bg-[hsl(var(--mac-close))]">
            <X className="h-2 w-2 text-black/50 opacity-0 group-hover:opacity-100" strokeWidth={3} />
          </span>
        </button>
        <button
          onClick={minimize}
          data-no-drag
          className="group flex h-5 w-5 items-center justify-center rounded-full transition-[filter] hover:brightness-110"
          aria-label={t("common.minimize")}
        >
          <span className="h-3 w-3 rounded-full bg-[hsl(var(--mac-minimize))]">
            <Minus className="h-2 w-2 text-black/50 opacity-0 group-hover:opacity-100" strokeWidth={3} />
          </span>
        </button>
        <button
          onClick={toggleMaximize}
          data-no-drag
          className="group flex h-5 w-5 items-center justify-center rounded-full transition-[filter] hover:brightness-110"
          aria-label={isMaximized ? t("common.restore") : t("common.maximize")}
        >
          <span className="h-3 w-3 rounded-full bg-[hsl(var(--mac-maximize))]">
            {isMaximized ? (
              <Copy className="h-2 w-2 text-black/50 opacity-0 group-hover:opacity-100" strokeWidth={2.5} />
            ) : (
              <Square className="h-2 w-2 text-black/50 opacity-0 group-hover:opacity-100" strokeWidth={3} />
            )}
          </span>
        </button>
        </div>

        {/* Center: product title — click to switch surfaces */}
        <div className="absolute left-1/2 flex -translate-x-1/2 items-center">
          <ProductMenu
            codeStreaming={codeStreaming}
            depworkStreaming={depworkStreaming}
          />
        </div>

        {/* Right: Toolbar */}
        <div className="flex items-center gap-0.5">{ToolbarButtons}</div>
      </header>
    );
  }

  // ── Windows style ────────────────────────────────────────
  return (
    <header
      data-tauri-drag-region
      className={cn(
        "no-select relative flex h-10 items-center justify-between",
        "bg-[hsl(var(--titlebar-bg))] shadow-[var(--shadow-paper-sm)]",
      )}
    >
      {/* Left: sidebar toggle + product title (click to switch surfaces) */}
      <div className="flex items-center gap-1 pl-3">
        <button
          onClick={toggleSidebar}
          data-no-drag
          className="flex h-7 w-7 items-center justify-center rounded-md p-0.5 transition-colors hover:bg-muted/80"
          title={sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
          aria-label={sidebarCollapsed ? t("sidebar.expand") : t("sidebar.collapse")}
          aria-pressed={!sidebarCollapsed}
        >
          <img src={appIcon} alt="DeepDepCat" className="h-4 w-4 rounded-sm" />
        </button>
        <ProductMenu
          codeStreaming={codeStreaming}
          depworkStreaming={depworkStreaming}
        />
      </div>

      {/* Right: Toolbar + window controls (flush to edge) */}
      <div className="flex h-full items-center">
        <div className="mr-1 flex items-center gap-0.5 pr-1">{ToolbarButtons}</div>

        {/* Minimize */}
        <button
          onClick={minimize}
          data-no-drag
          className="flex w-[46px] items-center justify-center text-muted-foreground transition-colors hover:bg-muted/80"
          aria-label={t("common.minimize")}
        >
          <Minus className="h-3.5 w-3.5" strokeWidth={2} />
        </button>

        {/* Maximize / Restore */}
        <button
          onClick={toggleMaximize}
          data-no-drag
          className="flex w-[46px] items-center justify-center text-muted-foreground transition-colors hover:bg-muted/80"
          aria-label={isMaximized ? t("common.restore") : t("common.maximize")}
        >
          {isMaximized ? (
            <Copy className="h-3 w-3" strokeWidth={2} />
          ) : (
            <Square className="h-3 w-3" strokeWidth={2} />
          )}
        </button>

        {/* Close (red hover) */}
        <button
          onClick={close}
          data-no-drag
          className="flex w-[46px] items-center justify-center text-muted-foreground transition-colors hover:bg-destructive hover:text-destructive-foreground"
          aria-label={t("common.close")}
        >
          <X className="h-3.5 w-3.5" strokeWidth={2} />
        </button>
      </div>
    </header>
  );
}
