/**
 * Sidebar — left navigation column.
 *
 * Structure (one conversation list; Code adds a compact project dropdown):
 * ┌───────────────────────────────────┐
 * │ [🔍 搜索...        Ctrl+K]       │  SidebarToolbar (search + new)
 * │ [+ 新建对话]      [⏰] [⚡]       │
 * │ [📁 当前项目 ▾]（仅 Code）       │  WorkspaceSelector
 * │ [筛选: 全部/活跃]                │  SidebarFilterBar
 * │ 会话（全局列表）                 │  SessionList
 * │                                   │
 * │ ● [HZ] hanzi           [⚙]       │  SidebarFooter (single row)
 * └───────────────────────────────────┘
 */

import { useState, useCallback, useEffect, useRef, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { PanelLeft, UserCircle2 } from "lucide-react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { SidebarToolbar } from "@/components/sidebar/SidebarToolbar";
import { WorkspaceSelector } from "@/components/sidebar/WorkspaceSelector";
import { SidebarFilterBar } from "@/components/sidebar/SidebarFilterBar";
import { TaskSection } from "@/components/sidebar/TaskSection";
import { SidebarFooter } from "@/components/sidebar/SidebarFooter";
import { SessionList } from "@/components/sidebar/SessionList";
import { useAppStore } from "@/stores/appStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useAuthStore } from "@/stores/authStore";
import { useRunningSessionsStore } from "@/stores/runningSessionsStore";
import { useSessionList } from "@/hooks/useSessionList";
import { useSessionRestore } from "@/hooks/useSessionRestore";
import { useStreamingSessions } from "@/lib/streamingBus";
import { newChatInCurrentMode } from "@/lib/newChat";
import { focusChatTextarea } from "@/lib/refineSelection";
import appIcon from "/icon.png";

export function Sidebar() {
  const { t } = useTranslation();
  const [searchQuery, setSearchQuery] = useState("");
  const sidebarCollapsed = useAppStore((s) => s.sidebarCollapsed);
  const toggleSidebar = useAppStore((s) => s.toggleSidebar);

  // ── Hooks (data fetching) ──────────────────────────────────
  // Global session list — one list for both products and all workspaces.
  const sessionList = useSessionList(null);
  const { selectSession } = useSessionRestore();
  // Search input — owned here so Ctrl/Cmd+K keeps working even when the
  // sidebar is collapsed (the toolbar unmounts, the shortcut must not).
  const searchInputRef = useRef<HTMLInputElement>(null);

  // ── Store ──────────────────────────────────────────────────
  // Collapsed-rail account entry — avatar when signed in, generic icon when not.
  const collapsedUser = useAuthStore((s) => s.user);
  const hasCollapsedAvatar = Boolean(collapsedUser?.avatar);

  const currentSessionId = useChatStore((s) => s.currentSessionId);
  const isStreaming = useChatStore((s) => s.isStreaming);
  const depworkStreaming = useDepworkChatStore((s) => s.isStreaming);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);
  const appMode = useAppStore((s) => s.mode);

  // Highlight the active session of the CURRENT mode (depwork sessions are
  // highlighted when the app is in depwork mode, code ones otherwise).
  const activeSessionId = appMode === "depwork" ? depworkSessionId : currentSessionId;

  // ── Per-session live indicators ─────────────────────────────
  // Any session (code or depwork) may stream in the background while the
  // user works on another — poll the stores so the sidebar rows show a live
  // spinner for every running session (and the user can stop it there).
  // Event-driven: the chat stores push streaming flips into streamingBus,
  // so the sidebar re-renders on actual changes instead of polling 2×/s.
  const streamingSessions = useStreamingSessions();
  const runningTurns = useRunningSessionsStore((s) => s.running);
  const streamingSessionIds = useMemo(() => {
    const ids = new Set<string>();
    for (const s of sessionList.sessions) {
      if (streamingSessions.has(s.id)) ids.add(s.id);
    }
    // Background main-agent turns (persistence) keep the row live even if
    // the frontend event stream for that session was torn down.
    for (const t of runningTurns) ids.add(t.session_id);
    return ids;
  }, [sessionList.sessions, streamingSessions, runningTurns]);

  // ── Sync search query to the session list ─────────────────
  const handleSearchChange = useCallback((value: string) => {
    setSearchQuery(value);
    sessionList.setSearchQuery(value);
  }, [sessionList]);

  // ── Refresh session list on session/stream changes ────────
  const refreshSessions = sessionList.refresh;
  const prevStreamingRef = useRef(isStreaming);
  const prevDepworkStreamingRef = useRef(depworkStreaming);
  useEffect(() => {
    const codeEnded = prevStreamingRef.current && !isStreaming;
    const depworkEnded = prevDepworkStreamingRef.current && !depworkStreaming;
    prevStreamingRef.current = isStreaming;
    prevDepworkStreamingRef.current = depworkStreaming;
    if ((codeEnded && currentSessionId) || (depworkEnded && depworkSessionId)) {
      refreshSessions();
    }
  }, [isStreaming, depworkStreaming, currentSessionId, depworkSessionId, refreshSessions]);

  // Refresh once when a session is created directly.
  const prevSessionIdRef = useRef(currentSessionId);
  useEffect(() => {
    if (currentSessionId && currentSessionId !== prevSessionIdRef.current) {
      refreshSessions();
    }
    prevSessionIdRef.current = currentSessionId;
  }, [currentSessionId, refreshSessions]);

  // ── New conversation — stays in the CURRENT mode (same path as Ctrl+N) ──
  const handleNewSession = useCallback(() => {
    newChatInCurrentMode();
  }, []);

  // Shared SessionList props.
  const sessionListProps = {
    activeSessionId,
    streamingSessionIds,
    onSelect: selectSession,
    onDelete: sessionList.deleteSession,
    onRename: sessionList.renameSession,
    onStopStream: async (sessionId: string) => {
      // The session may belong to either surface — stop it in the store
      // that owns it.
      if (useChatStore.getState().isSessionStreaming(sessionId)) {
        await useChatStore.getState().stopSessionStreaming(sessionId);
      } else if (useDepworkChatStore.getState().isSessionStreaming(sessionId)) {
        await useDepworkChatStore.getState().stopSessionStreaming(sessionId);
      }
    },
    onTogglePin: sessionList.togglePin,
    isSearching: searchQuery.trim().length > 0,
    onRetry: () => void sessionList.refresh(),
    onNewSession: handleNewSession,
  };

  // Ctrl/Cmd+K → focus search — lives HERE (not in SidebarToolbar) so it
  // keeps working when the sidebar is collapsed and the toolbar unmounts.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        // A modal dialog owns focus — don't yank it into the search box.
        if (document.querySelector('[aria-modal="true"]')) return;
        if (searchInputRef.current) {
          searchInputRef.current.focus();
        } else {
          // Sidebar collapsed → no search input; focus the chat textarea
          // instead so the shortcut still lands somewhere useful.
          focusChatTextarea();
        }
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  return (
    <aside
      className="flex h-full flex-col bg-[hsl(var(--sidebar-bg))] border-r border-[hsl(var(--sidebar-border))]"
    >
      {sidebarCollapsed ? (
        /* ── Collapsed: icon rail ─────────────────────────────── */
        <div className="flex h-full w-full flex-col items-center py-2">
          <button
            onClick={toggleSidebar}
            className="flex h-8 w-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted/80 hover:text-foreground"
            title={t("sidebar.expand")}
            aria-label={t("sidebar.expand")}
          >
            <PanelLeft className="h-4 w-4" />
          </button>
          {/* Account entry — expand the rail to sign in / see the user */}
          <div className="mt-auto">
            <button
              onClick={toggleSidebar}
              className="flex h-8 w-8 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-muted/80 hover:text-foreground"
              title={t("sidebar.expand")}
              aria-label={t("sidebar.expand")}
            >
              {collapsedUser ? (
                <Avatar className="h-6 w-6">
                  {hasCollapsedAvatar && (
                    <AvatarImage src={collapsedUser.avatar!} alt={collapsedUser.username} />
                  )}
                  <AvatarFallback className="bg-secondary p-0.5">
                    <img src={appIcon} alt="DeepDepCat" className="h-full w-full rounded-sm" />
                  </AvatarFallback>
                </Avatar>
              ) : (
                <UserCircle2 className="h-4 w-4" />
              )}
            </button>
          </div>
        </div>
      ) : (
        <>
          <SidebarToolbar
            searchQuery={searchQuery}
            onSearchChange={handleSearchChange}
            inputRef={searchInputRef}
            onNewTask={handleNewSession}
          />

          {appMode === "code" && <WorkspaceSelector />}

          <div className="mt-0.5">
            <SidebarFilterBar
              filter={sessionList.filter}
              onFilterChange={sessionList.setFilter}
            />
          </div>

          <TaskSection />

          {/* ── The single global conversation list ── */}
          <ScrollArea className="flex-1">
            <div className="space-y-1 px-2 py-2">
              <SessionList
                sessions={sessionList.sessions}
                loading={sessionList.loading}
                error={sessionList.error}
                {...sessionListProps}
              />
            </div>
          </ScrollArea>

          <SidebarFooter />
        </>
      )}
    </aside>
  );
}
