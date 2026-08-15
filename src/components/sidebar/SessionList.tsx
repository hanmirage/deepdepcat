/**
 * SessionList — renders a list of chat sessions in the sidebar.
 *
 * Sessions are grouped by recency (today / yesterday / this week / earlier)
 * so long histories stay scannable. Each row supports:
 * - select (click)
 * - inline rename (hover ✎ → edit → Enter saves, Esc cancels, blur saves)
 * - delete with two-step confirmation
 *
 * Pure component: receives sessions + callbacks as props.
 */

import { useState, useEffect, useRef, useMemo } from "react";
import { useTranslation } from "react-i18next";
import {
  MessageSquare,
  ChevronRight,
  Trash2,
  Pencil,
  Square,
  Loader2,
  Moon,
  Folder,
  Pin,
} from "lucide-react";
import type { Session } from "@/types";
import { cn, timeAgo } from "@/lib/utils";

export interface SessionListProps {
  sessions: Session[];
  loading: boolean;
  error: string | null;
  activeSessionId: string | null;
  /** Sessions with a live stream (in any mode) — shows a spinner row. */
  streamingSessionIds?: ReadonlySet<string>;
  onSelect: (session: Session) => void;
  onDelete: (id: string) => Promise<void>;
  /** Persist a renamed title. Called with the trimmed title. */
  onRename: (id: string, title: string) => Promise<void>;
  /** True while the user has an active search query (drives the no-match state). */
  isSearching?: boolean;
  /** Stop a streaming session from the list row (default: hidden). */
  onStopStream?: (sessionId: string) => Promise<void>;
  /** Toggle a session's pinned state (sidebar top-of-list placement). */
  onTogglePin?: (sessionId: string) => Promise<void>;
  /** Re-run the failed list load (shows a retry button in the error state). */
  onRetry?: () => void;
  /** Start a fresh conversation (empty-state CTA). */
  onNewSession?: () => void;
}

// ── Recency grouping ───────────────────────────────────────

type GroupKey = "pinned" | "today" | "yesterday" | "week" | "earlier";

const GROUP_ORDER: GroupKey[] = ["pinned", "today", "yesterday", "week", "earlier"];

function groupKey(ts: number): GroupKey {
  const now = new Date();
  const that = new Date(ts);
  // Compare calendar days (midnight-to-midnight) — a session at 23:00
  // yesterday is "yesterday", not "today".
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const thatStart = new Date(that.getFullYear(), that.getMonth(), that.getDate()).getTime();
  const days = Math.round((todayStart - thatStart) / 86_400_000);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 7) return "week";
  return "earlier";
}

// ── Inline rename input ────────────────────────────────────

function RenameInput({
  initial,
  onSave,
  onCancel,
}: {
  initial: string;
  onSave: (title: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLInputElement>(null);

  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);

  const commit = () => {
    const v = value.trim();
    if (v && v !== initial) {
      onSave(v);
    } else {
      onCancel();
    }
  };

  return (
    <input
      ref={ref}
      value={value}
      onChange={(e) => setValue(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") onCancel();
      }}
      onBlur={commit}
      className="w-full rounded border border-primary/50 bg-background px-1.5 py-0.5 text-xs focus-visible:outline-none"
    />
  );
}

// ── Main component ─────────────────────────────────────────

export function SessionList({
  sessions,
  loading,
  error,
  activeSessionId,
  streamingSessionIds,
  onSelect,
  onDelete,
  onRename,
  isSearching = false,
  onStopStream,
  onTogglePin,
  onRetry,
  onNewSession,
}: SessionListProps) {
  const { t } = useTranslation();

  // Session awaiting delete confirmation.
  const [confirmId, setConfirmId] = useState<string | null>(null);
  // Session currently being renamed inline.
  const [renamingId, setRenamingId] = useState<string | null>(null);
  // Inline delete error feedback — keyed by session id so a failure shows
  // ONLY under the row that failed, never on every row.
  const [deleteErrors, setDeleteErrors] = useState<Record<string, string>>({});

  // Sessions grouped by recency, in display order.
  const groups = useMemo(() => {
    const map = new Map<GroupKey, Session[]>();
    for (const s of sessions) {
      const ts = new Date(s.updated_at ?? s.created_at).getTime();
      // Pinned sessions get their own top group (never double-appear in a
      // recency group).
      const key: GroupKey = s.pinned
        ? "pinned"
        : Number.isNaN(ts)
          ? "earlier"
          : groupKey(ts);
      const list = map.get(key) ?? [];
      list.push(s);
      map.set(key, list);
    }
    return GROUP_ORDER.filter((k) => map.has(k)).map((k) => ({ key: k, items: map.get(k)! }));
  }, [sessions]);

  const handleRenameSave = async (id: string, title: string) => {
    setRenamingId(null);
    try {
      await onRename(id, title);
      // Clear any prior rename error on success.
      setDeleteErrors((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
    } catch {
      // Surface an inline error so a failed rename isn't silent (the title
      // reverts and the row shows why).
      setDeleteErrors((prev) => ({ ...prev, [id]: t("sidebar.renameFailed") }));
    }
  };

  const handleConfirmDelete = async (id: string) => {
    try {
      await onDelete(id);
      setConfirmId(null);
      setDeleteErrors((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
    } catch {
      setDeleteErrors((prev) => ({ ...prev, [id]: t("sidebar.deleteFailed") }));
      setConfirmId(null);
    }
  };

  if (loading) {
    return (
      <div className="space-y-0.5" aria-busy="true">
        {[...Array(3)].map((_, i) => (
          <div key={i} className="flex items-center gap-2 rounded-md px-2 py-1.5">
            <div className="h-3.5 w-3.5 shrink-0 animate-pulse rounded bg-muted" />
            <div className="flex-1 space-y-1">
              <div className="h-3 w-3/4 animate-pulse rounded bg-muted" />
              <div className="h-2 w-1/2 animate-pulse rounded bg-muted" />
            </div>
          </div>
        ))}
      </div>
    );
  }

  if (error) {
    return (
      <div className="space-y-1.5 px-2 py-2">
        <div className="flex items-center gap-2">
          <MessageSquare className="h-3 w-3 text-destructive/60" />
          <span className="text-[11px] text-destructive/80">{t("sidebar.loadFailed")}: {error}</span>
        </div>
        {onRetry && (
          <button
            onClick={() => void onRetry()}
            className="rounded border border-border bg-muted/40 px-2 py-1 text-[11px] text-foreground/80 transition-colors hover:bg-muted hover:text-foreground"
          >
            {t("common.retry")}
          </button>
        )}
      </div>
    );
  }

  if (sessions.length === 0) {
    return (
      <div className="flex flex-col items-center gap-2 px-2 py-4 text-center">
        <MessageSquare className="h-6 w-6 text-muted-foreground/30" />
        <span className="text-[11px] text-muted-foreground/60">
          {isSearching ? t("sidebar.noMatch") : t("sidebar.noSessionYet")}
        </span>
        {isSearching && (
          <span className="text-[10px] text-muted-foreground/40">
            {t("sidebar.searchScopeHint")}
          </span>
        )}
        {!isSearching && onNewSession && (
          <button
            onClick={onNewSession}
            className="mt-1 rounded-md border border-primary/30 bg-primary/5 px-3 py-1 text-[11px] font-medium text-primary transition-colors hover:bg-primary/10"
          >
            {t("sidebar.newChat")}
          </button>
        )}
      </div>
    );
  }

  const handleDeleteClick = (id: string) => {
    setConfirmId(id);
    setDeleteErrors((prev) => {
      const next = { ...prev };
      delete next[id];
      return next;
    });
  };

  return (
    <div className="space-y-2">
      {groups.map((group) => (
        <div key={group.key} className="space-y-0.5">
          {/* Group header */}
          <p className="px-2 pb-0.5 pt-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/50">
            {t(`sidebar.${group.key}`)}
          </p>

          {group.items.map((session) => {
            const isActive = session.id === activeSessionId;
            const isConfirming = confirmId === session.id;
            const isRenaming = renamingId === session.id;
            const isStreaming = streamingSessionIds?.has(session.id) ?? false;
            const isPinned = session.pinned ?? false;
            return (
              <div
                key={session.id}
                onClick={() => {
                  // The whole row selects the session — only the explicit
                  // action buttons (stop/rename/delete) stopPropagation.
                  if (!isConfirming && !isRenaming) onSelect(session);
                }}
                className={cn(
                  "group flex w-full items-start gap-2 rounded-md border-l-2 px-2 py-1.5 text-left transition-colors",
                  isActive
                    ? "border-primary/70 bg-primary/5"
                    : "border-transparent hover:bg-secondary/40 hover:text-secondary-foreground",
                )}
              >
                {isConfirming ? (
                  <div className="flex w-full items-center gap-1.5 py-0.5">
                    <span className="flex-1 text-[11px] text-muted-foreground">
                      {t("sidebar.deleteSession")}?
                    </span>
                    <button
                      onClick={() => handleConfirmDelete(session.id)}
                      className="min-h-7 rounded bg-destructive px-2.5 py-1 text-[11px] text-destructive-foreground hover:bg-destructive/90"
                    >
                      {t("common.yes")}
                    </button>
                    <button
                      onClick={() => setConfirmId(null)}
                      className="min-h-7 rounded px-2.5 py-1 text-[11px] text-muted-foreground hover:bg-muted"
                    >
                      {t("common.no")}
                    </button>
                  </div>
                ) : (
                  <>
                    <button
                      className="flex flex-1 items-start min-w-0"
                      onClick={(e) => {
                        // Row-level onClick already selects; stop here so a
                        // single click doesn't call onSelect twice.
                        e.stopPropagation();
                        onSelect(session);
                      }}
                    >
                      <div className="flex-1 min-w-0">
                        {isRenaming ? (
                          <RenameInput
                            initial={session.title}
                            onSave={(title) => void handleRenameSave(session.id, title)}
                            onCancel={() => setRenamingId(null)}
                          />
                        ) : (
                          <p className="flex items-center gap-1.5 truncate text-xs font-medium">
                            {isStreaming && (
                              <Loader2 className="h-3 w-3 shrink-0 animate-spin text-primary" />
                            )}
                            <span className="truncate">
                              {session.title || t("sidebar.untitledSession")}
                            </span>
                          </p>
                        )}
                        {session.last_message ? (
                          <p className="truncate text-[11px] text-muted-foreground/60">
                            {session.last_message}
                          </p>
                        ) : null}
                        <p className="truncate text-[11px] text-muted-foreground/70">
                          {session.work_mode === "depwork" && (
                            <span className="mr-1 rounded bg-violet-500/15 px-1 py-px text-[10px] font-medium text-violet-500 dark:text-violet-400">
                              Depwork
                            </span>
                          )}
                          {session.workspace_path && (
                            <span className="mr-1 inline-flex items-center gap-0.5 rounded bg-sky-500/10 px-1 py-px text-[10px] font-medium text-sky-600 dark:text-sky-400">
                              <Folder className="h-2.5 w-2.5" />
                              {session.workspace_path.split(/[\\/]/).pop() ??
                                session.workspace_path}
                            </span>
                          )}
                          {session.status === "idle" && (
                            <span
                              className="mr-1 inline-flex items-center gap-0.5 rounded bg-muted px-1 py-px text-[10px] font-medium text-muted-foreground"
                              title={t("sidebar.dormantHint")}
                            >
                              <Moon className="h-2.5 w-2.5" />
                              {t("sidebar.dormant")}
                            </span>
                          )}
                        </p>
                        {deleteErrors[session.id] && (
                          <p className="text-[10px] text-destructive">{deleteErrors[session.id]}</p>
                        )}
                      </div>
                    </button>
                    <div className="flex shrink-0 items-center gap-0.5">
                      <span className="text-[10px] text-muted-foreground">
                        {timeAgo(session.updated_at ?? session.created_at, t)}
                      </span>
                      {isStreaming && onStopStream && (
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            void onStopStream(session.id);
                          }}
                          className="mt-0.5 rounded p-0.5 text-muted-foreground transition-colors hover:text-destructive"
                          title={t("common.stop")}
                          aria-label={t("common.stop")}
                        >
                          <Square className="h-3 w-3 fill-current" />
                        </button>
                      )}
                      {onTogglePin && (
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            void onTogglePin(session.id);
                          }}
                          className={cn(
                            "mt-0.5 rounded p-0.5 transition-colors",
                            isPinned
                              ? "text-primary"
                              : "text-muted-foreground opacity-0 hover:text-primary group-hover:opacity-100 focus-visible:opacity-100",
                          )}
                          title={isPinned ? t("sidebar.unpin") : t("sidebar.pin")}
                          aria-label={isPinned ? t("sidebar.unpin") : t("sidebar.pin")}
                        >
                          <Pin className={cn("h-3 w-3", isPinned && "fill-current")} />
                        </button>
                      )}
                      <ChevronRight className="h-3 w-3 text-muted-foreground/50" />
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          setRenamingId(session.id);
                        }}
                        className="mt-0.5 rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover:opacity-100 focus-visible:opacity-100"
                        title={t("sidebar.rename")}
                        aria-label={t("sidebar.rename")}
                      >
                        <Pencil className="h-3 w-3" />
                      </button>
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          handleDeleteClick(session.id);
                        }}
                        className="mt-0.5 rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:text-destructive group-hover:opacity-100 focus-visible:opacity-100"
                        aria-label={t("sidebar.deleteSession")}
                      >
                        <Trash2 className="h-3 w-3" />
                      </button>
                    </div>
                  </>
                )}
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}
