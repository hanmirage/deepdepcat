/**
 * AppShell — top-level layout container.
 *
 * Paper-cut aesthetic: each panel is a separate "sheet of paper"
 * with shadows creating depth hierarchy.
 *
 * Structure:
 * ┌─────────────────────────────────────────────────────────┐
 * │                      TitleBar (h-10)                      │
 * ├──────────┬────────────────────────────┬────────┬──────────┤
 * │          │                            │        │          │
 * │  Sidebar │      Main Panel            │  Drawer (on-demand)  │
 * │  (drag)  │      (flex-1)              │  hidden until needed │
 * ├──────────┴────────────────────────────┴────────┴──────────┤
 * └────────────────────────────────────────────────────────────┘
 */

import { useEffect } from "react";
import { TitleBar } from "./TitleBar";
import { Sidebar } from "./Sidebar";
import { MainPanel } from "./MainPanel";
import { RightPanel } from "./RightPanel";
import { TakeoverOverlay } from "./TakeoverOverlay";
import { ForceUpdateDialog } from "./ForceUpdateDialog";
import { useSidebarResize } from "./useSidebarResize";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useRightPanelActivity } from "@/hooks/useRightPanelActivity";
import { useRightPanelBrowser } from "@/hooks/useRightPanelBrowser";
import { useAgentBrowserPane } from "@/hooks/useAgentBrowserPane";
import { usePlanSummaryPane } from "@/hooks/usePlanSummaryPane";
import { useSubagentPanel } from "@/hooks/useSubagentPanel";
import { useTaskPanel } from "@/hooks/useTaskPanel";
import { useTodoEvents } from "@/hooks/useTodoEvents";
import { useDelayedUnmount } from "@/hooks/useDelayedUnmount";

/** Window widths below this auto-collapse the sidebar. */
const AUTO_COLLAPSE_BREAKPOINT = 1100;

export function AppShell() {
  const mode = useAppStore((s) => s.mode);
  const rightPanelOpen = useRightPanelStore((s) => s.open);
  const rightPanelMounted = useDelayedUnmount(rightPanelOpen, 200);
  const sidebarCollapsed = useAppStore((s) => s.sidebarCollapsed);
  const sidebarUserManaged = useAppStore((s) => s.sidebarUserManaged);
  const setSidebarCollapsed = useAppStore((s) => s.setSidebarCollapsed);
  const {
    effectiveWidth,
    dragActive,
    handlePointerDown,
    handlePointerMove,
    handlePointerUp,
  } = useSidebarResize(sidebarCollapsed, setSidebarCollapsed);
  // Event-driven right-panel signals, live at the shell now that the
  // always-present rail is gone: activity auto-opens the drawer once (L3),
  // and the agent opening the browser surfaces the embedded dev browser
  // (depwork).
  useRightPanelActivity(mode);
  useRightPanelBrowser();
  useAgentBrowserPane();
  usePlanSummaryPane(mode);
  useSubagentPanel(mode);
  useTaskPanel(mode);
  // The todo pipeline must be subscribed at the shell, NOT inside the task
  // pane — the pane auto-opens only when todos appear, and todos only load
  // while this subscription is live. Gating it behind the pane mount dead-
  // locked the code-mode task pane (plan never auto-opened).
  const codeSessionId = useChatStore((s) => s.currentSessionId);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);
  const todoSessionId = mode === "depwork" ? depworkSessionId : codeSessionId;
  useTodoEvents(todoSessionId);

  // ── Auto-collapse on narrow windows ─────────────────────────
  // Always collapse when the window is genuinely narrow (gives the main panel
  // room), but only while the user hasn't manually chosen a state — once
  // they've collapsed or expanded, the window width no longer fights them.
  useEffect(() => {
    const apply = () => {
      const narrow = window.innerWidth < AUTO_COLLAPSE_BREAKPOINT;
      if (narrow && !sidebarUserManaged) {
        setSidebarCollapsed(true);
      } else if (!narrow && !sidebarUserManaged) {
        setSidebarCollapsed(false);
      }
    };
    apply();
    window.addEventListener("resize", apply);
    return () => window.removeEventListener("resize", apply);
  }, [sidebarUserManaged, setSidebarCollapsed]);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background">
      <TitleBar />
      <ForceUpdateDialog />
      <TakeoverOverlay />
      <div className="flex flex-1 overflow-hidden gap-0">
        <div
          className="relative shrink-0"
          style={{
            width: effectiveWidth,
            // Animate collapse/expand, but NOT while dragging the resize handle.
            // Inline style so the disable is synchronous (React batch would
            // otherwise let one transition fire before the class lands).
            transition: dragActive ? "none" : "width 0.2s ease",
          }}
        >
          <Sidebar />
          <div
            className="drag-handle"
            onPointerDown={handlePointerDown}
            onPointerMove={handlePointerMove}
            onPointerUp={handlePointerUp}
            onPointerCancel={handlePointerUp}
          />
        </div>
        <ErrorBoundary resetKey="main-panel">
          <MainPanel />
        </ErrorBoundary>
        {rightPanelMounted && (
          <ErrorBoundary resetKey="right-panel">
            <div
              className={
                rightPanelOpen
                  ? "h-full"
                  : "h-full animate-out fade-out slide-out-to-right duration-200"
              }
            >
              <RightPanel />
            </div>
          </ErrorBoundary>
        )}
      </div>
    </div>
  );
}
