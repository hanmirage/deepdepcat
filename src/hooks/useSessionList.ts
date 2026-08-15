/**
 * useSessionList — loads chat sessions from backend and provides
 * search filtering, refresh, and delete operations.
 *
 * Components call this hook; they never touch sessionApi directly.
 */

import { useState, useEffect, useCallback, useMemo } from "react";
import { logError } from "@/lib/logger";
import { sessionApi } from "@/lib/tauri";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useTodoStore } from "@/stores/todoStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { Session } from "@/types";

export interface UseSessionListResult {
  sessions: Session[];
  loading: boolean;
  error: string | null;
  searchQuery: string;
  setSearchQuery: (q: string) => void;
  filter: SessionFilter;
  setFilter: (f: SessionFilter) => void;
  refresh: () => Promise<void>;
  deleteSession: (id: string) => Promise<void>;
  /** Rename a session — persists via the backend and updates the local list. */
  renameSession: (id: string, title: string) => Promise<void>;
  /** Pin/unpin a session — persists via the backend and flips it locally. */
  togglePin: (id: string) => Promise<void>;
}

export type SessionFilter = "all" | "active" | "archived";

/** When set, only sessions bound to this workspace path are listed
 *  (multi-project sidebar). Pass null for all workspaces. */
export type WorkspaceFilter = string | null;

export function useSessionList(workspaceFilter?: WorkspaceFilter): UseSessionListResult {
  const [allSessions, setAllSessions] = useState<Session[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [filter, setFilter] = useState<SessionFilter>("all");

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await sessionApi.listSessions(50);
      setAllSessions(result);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : "Failed to load sessions");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Live-update on backend session lifecycle changes (idle-reaper evictions
  // flip a session to "idle", deletions remove it, renames bump the title).
  useTauriEvent<{ type: string; session_id: string }>("session-lifecycle", () => {
    void refresh();
  });

  const deleteSession = useCallback(async (id: string) => {
    // Deleting the active session would leave either chat store pointing at
    // a dead id — clear it first so a subsequent send starts a fresh session
    // instead of hitting "SessionNotFound". BOTH stores get the same guard
    // (previously only chatStore was checked — deleting the ACTIVE depwork
    // session left its currentSessionId + messages pointing at the dead id).
    if (useChatStore.getState().currentSessionId === id) {
      useChatStore.getState().clearMessages();
    }
    if (useDepworkChatStore.getState().currentSessionId === id) {
      useDepworkChatStore.getState().clearMessages();
    }
    // Drop the session's stream state (listener + queue) in BOTH stores —
    // the sidebar lists sessions from either surface and can't know which
    // one owns it. disposeSession no-ops for unknown ids.
    useChatStore.getState().disposeSession(id);
    useDepworkChatStore.getState().disposeSession(id);
    // Drop the session's todo list (per-session, otherwise accumulates).
    useTodoStore.getState().clearSession(id);
    try {
      await sessionApi.deleteSession(id);
      setAllSessions((prev) => prev.filter((s) => s.id !== id));
    } catch (e) {
      logError("useSessionList", "Failed to delete session:", e);
      throw e;
    }
  }, []);

  const renameSession = useCallback(async (id: string, title: string) => {
    const trimmed = title.trim();
    if (!trimmed) return;
    try {
      await sessionApi.updateSessionTitle(id, trimmed);
      setAllSessions((prev) =>
        prev.map((s) => (s.id === id ? { ...s, title: trimmed } : s)),
      );
    } catch (e) {
      logError("useSessionList", "Failed to rename session:", e);
      // Rethrow so the caller (SessionList row) can surface an inline error —
      // a silent revert leaves the user thinking the rename succeeded.
      throw e;
    }
  }, []);

  const togglePin = useCallback(async (id: string) => {
    try {
      const pinned = allSessions.find((s) => s.id === id)?.pinned ?? false;
      await sessionApi.setSessionPinned(id, !pinned);
      setAllSessions((prev) =>
        prev.map((s) => (s.id === id ? { ...s, pinned: !pinned } : s)),
      );
    } catch (e) {
      logError("useSessionList", "Failed to toggle pin:", e);
      throw e;
    }
  }, [allSessions]);

  const filteredSessions = useMemo(() => {
    let result = allSessions;

    if (workspaceFilter) {
      result = result.filter((s) => s.workspace_path === workspaceFilter);
    }

    if (filter === "active") {
      result = result.filter((s) => s.status === "active" || s.status === "idle");
    } else if (filter === "archived") {
      result = result.filter((s) => s.status === "archived");
    }

    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      result = result.filter((s) => s.title.toLowerCase().includes(q));
    }

    return result;
  }, [allSessions, filter, searchQuery, workspaceFilter]);

  return {
    sessions: filteredSessions,
    loading,
    error,
    searchQuery,
    setSearchQuery,
    filter,
    setFilter,
    refresh,
    deleteSession,
    renameSession,
    togglePin,
  };
}
