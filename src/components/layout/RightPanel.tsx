/**
 * RightPanel — an event-driven transient context-pane stack, shared by Code
 * and Depwork.
 *
 * Context panes appear on demand:
 * - task (both): Code = session goal + todo tree; Depwork = goal + tool steps
 * - subagents (both): dispatched subagents, one card each (live progress + result)
 * - activity (both): live execution stream + background sessions/tasks
 * - files (both): workspace browser + preview (mode-specific renderers)
 * - plan (both): plan-mode status + pending interactions
 * - browser (Depwork only): dev browser preview
 *
 * The pane stack and panel width persist per mode (rightPanelStore).
 * Dismissing the panel suppresses auto-show for the rest of the run; the
 * title-bar badge keeps pulsing.
 */

import { useEffect } from "react";
import {
  ArrowLeft,
  Bot,
  ClipboardList,
  Compass,
  FileText,
  FolderOpen,
  ListChecks,
  ListTodo,
  X,
  type LucideIcon,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { AppMode } from "@/config/constants";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useAppStore } from "@/stores/appStore";
import {
  useRightPanelStore,
  DEFAULT_RIGHT_PANEL_WIDTH,
  type RightPaneId,
} from "@/stores/rightPanelStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { AgentActivityCard } from "@/components/customize/AgentActivityCard";
import { WorkspaceFilesPanel } from "@/components/customize/WorkspaceFilesPanel";
import { DepworkTaskPanel } from "@/components/depwork/DepworkTaskPanel";
import { WorkspacePanel } from "@/components/depwork/WorkspacePanel";
import { DocumentContextCard } from "@/components/depwork/DocumentContextCard";
import { HtmlPreviewPane } from "@/components/preview/HtmlPreviewPane";
import { BrowserLivePane } from "@/components/preview/BrowserLivePane";
import { PlanSummaryPanel } from "@/components/customize/PlanSummaryPanel";
import { SubagentPanel } from "@/components/customize/SubagentPanel";
import { TaskPanel } from "@/components/customize/TaskPanel";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { usePanelResize } from "@/components/layout/usePanelResize";

interface PaneDef {
  id: RightPaneId;
  icon: LucideIcon;
  label: string;
}

const PANE_DEFS: Record<RightPaneId, PaneDef> = {
  activity: { id: "activity", icon: ListChecks, label: "rightPanel.paneActivity" },
  files: { id: "files", icon: FolderOpen, label: "rightPanel.paneFiles" },
  browser: { id: "browser", icon: Compass, label: "rightPanel.paneBrowser" },
  preview: { id: "preview", icon: FileText, label: "rightPanel.panePreview" },
  plan: { id: "plan", icon: ClipboardList, label: "rightPanel.panePlan" },
  subagents: { id: "subagents", icon: Bot, label: "rightPanel.paneSubagents" },
  task: { id: "task", icon: ListTodo, label: "rightPanel.paneTask" },
};

/** One pane = one scrollable context (never a shared tab stack). */
function PanelPage({ children }: { children: React.ReactNode }) {
  return (
    <ScrollArea className="h-full">
      <div className="space-y-3 p-3">{children}</div>
    </ScrollArea>
  );
}

/** Pane content by id — mode-specific renderers live behind one page. */
function PaneContent({
  pane,
  mode,
  isDepwork,
  sessionId,
  depworkMessages,
  depworkStreaming,
}: {
  pane: RightPaneId;
  mode: AppMode;
  isDepwork: boolean;
  sessionId: string | null | undefined;
  depworkMessages: ReturnType<typeof useDepworkChatStore.getState>["messages"];
  depworkStreaming: boolean;
}) {
  // The browser (real-agent live view) and preview (artifact renderer) panes
  // fill the drawer flush — a frame should own the whole space, not sit
  // inside a scroll region.
  if (pane === "browser") {
    return <BrowserLivePane mode={mode} />;
  }
  if (pane === "preview") {
    return <HtmlPreviewPane mode={mode} />;
  }
  return (
    <PanelPage>
      {pane === "activity" && <AgentActivityCard isDepwork={isDepwork} />}
      {pane === "files" &&
        (isDepwork ? (
          <>
            <WorkspacePanel />
            <DocumentContextCard />
          </>
        ) : (
          <WorkspaceFilesPanel />
        ))}
      {pane === "plan" && <PlanSummaryPanel sessionId={sessionId} />}
      {pane === "subagents" && <SubagentPanel isDepwork={isDepwork} />}
      {pane === "task" &&
        (isDepwork ? (
          <DepworkTaskPanel
            messages={depworkMessages}
            isStreaming={depworkStreaming}
            sessionId={sessionId}
          />
        ) : (
          <TaskPanel sessionId={sessionId} />
        ))}
    </PanelPage>
  );
}

/**
 * RightPanel — the expanded context drawer. Exactly one pane is visible;
 * the header title follows the active pane.
 */
export function RightPanel() {
  const { t } = useTranslation();
  const mode = useAppStore((s) => s.mode);
  const open = useRightPanelStore((s) => s.open);
  const width = useRightPanelStore((s) => s.width[mode]);
  const panes = useRightPanelStore((s) => s.panes[mode]);
  const dismiss = useRightPanelStore((s) => s.dismiss);
  const setWidth = useRightPanelStore((s) => s.setWidth);
  const closePane = useRightPanelStore((s) => s.closePane);
  const clearActivitySignal = useRightPanelStore((s) => s.clearActivitySignal);
  const depworkMessages = useDepworkChatStore((s) => s.messages);
  const depworkStreaming = useDepworkChatStore((s) => s.isStreaming);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);
  const codeSessionId = useChatStore((s) => s.currentSessionId);

  const isDepwork = mode === "depwork";
  // The focused transient drives the title (no resident base anymore).
  const activePane = panes[panes.length - 1];
  const sessionId = isDepwork ? depworkSessionId : codeSessionId;

  const { handlePointerDown, handlePointerMove, handlePointerUp } = usePanelResize(
    width,
    (w) => setWidth(mode, w),
  );

  // Viewing the activity pane consumes the badge signal.
  useEffect(() => {
    if (open && panes.includes("activity")) clearActivitySignal(mode);
  }, [open, panes, mode, clearActivitySignal]);

  const pageTitle = activePane ? t(PANE_DEFS[activePane].label) : t("rightPanel.toggle");

  return (
    <ErrorBoundary resetKey={panes.join("-")}>
      <aside
        className="relative flex h-full shrink-0 flex-col border-l bg-[hsl(var(--card))] shadow-[var(--shadow-paper-lg)] animate-in slide-in-from-right duration-200"
        style={{ width }}
      >
        {/* Resize handle — left edge, captured pointer; double-click
            restores the default width. */}
        <div
          className="absolute inset-y-0 -left-1 z-20 w-1.5 cursor-col-resize"
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerUp}
          onDoubleClick={() => setWidth(mode, DEFAULT_RIGHT_PANEL_WIDTH)}
          aria-hidden="true"
        />
        <div className="flex items-center gap-2 border-b p-3">
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={() => dismiss(mode)}
            aria-label={t("rightPanel.collapse")}
            title={t("rightPanel.collapse")}
          >
            <ArrowLeft className="h-4 w-4" />
          </Button>
          <h2 className="text-sm font-semibold">{pageTitle}</h2>
        </div>

        <div className="flex min-h-0 flex-1 flex-col">
          {panes.length === 0 ? (
            <div className="flex flex-1 items-center justify-center p-4">
              <p className="text-xs text-muted-foreground">
                {t("rightPanel.empty")}
              </p>
            </div>
          ) : (
            panes.map((pane) => {
              const def = PANE_DEFS[pane];
              return (
                <section
                  key={pane}
                  className="flex min-h-0 flex-1 flex-col border-b border-border/60"
                >
                  <div className="flex shrink-0 items-center justify-between border-b border-border/60 bg-muted/40 px-3 py-1.5">
                    <span className="flex items-center gap-1.5 text-[11px] font-semibold text-muted-foreground">
                      <def.icon className="h-3 w-3" />
                      {t(def.label)}
                    </span>
                    <button
                      type="button"
                      onClick={() => closePane(mode, pane)}
                      aria-label={t("common.close")}
                      className="rounded p-0.5 text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground"
                    >
                      <X className="h-3 w-3" />
                    </button>
                  </div>
                  <div className="min-h-0 flex-1">
                    <PaneContent
                      pane={pane}
                      mode={mode}
                      isDepwork={isDepwork}
                      sessionId={sessionId}
                      depworkMessages={depworkMessages}
                      depworkStreaming={depworkStreaming}
                    />
                  </div>
                </section>
              );
            })
          )}
        </div>
      </aside>
    </ErrorBoundary>
  );
}
