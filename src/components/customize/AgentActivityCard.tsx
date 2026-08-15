/**
 * AgentActivityCard — live agent activity for the right panel.
 *
 * Claude Desktop style activity feed, polled from the backend:
 *   - Background sessions (list_running_sessions) with jump/stop
 *   - Background tasks (list_background_tasks) with terminate action
 *
 * The session goal lives in the TaskPanel; subagent execution lives in the
 * SubagentPanel — this card keeps background sessions/tasks only.
 *
 * Poll cadence: tasks every 5s; the elapsed-time ticker re-renders once per
 * second so the displayed durations stay live.
 */

import { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import {
  ArrowRight,
  Bot,
  CheckCircle2,
  Loader2,
  Square,
  Terminal,
  X,
} from "lucide-react";
import { CollapsibleCard } from "@/components/customize/CollapsibleCard";
import {
  agentApi,
  systemApi,
  type BackgroundTaskInfo,
  type AgentStatus,
  type RunningTurnInfo,
} from "@/lib/tauri";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { useSessionRestore } from "@/hooks/useSessionRestore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useRunningSessionsStore } from "@/stores/runningSessionsStore";

/** Poll cadence for worker/task state (ms) — the fallback safety net. The
 *  backend pushes `agent-status-changed` events for the common transitions,
 *  so the poll only catches task updates that don't emit an event. */
const POLL_INTERVAL_MS = 5000;
/** Elapsed-time ticker cadence (ms). */
const TICK_INTERVAL_MS = 1000;

/** Format ms → "mm:ss" (or "h:mm:ss" past an hour). Shared by the activity
 *  and subagent cards. */
export function formatElapsed(startedAtMs: number): string {
  const secs = Math.max(0, Math.floor((Date.now() - startedAtMs) / 1000));
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  const mm = String(m).padStart(2, "0");
  const ss = String(s).padStart(2, "0");
  return h > 0 ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

function TaskRow({
  task,
  onKill,
}: {
  task: BackgroundTaskInfo;
  onKill: (id: string) => void;
}) {
  const { t } = useTranslation();
  const running = task.status === "running" || task.status === "pending";

  return (
    <div className="flex items-center gap-1.5 rounded-md border border-border/60 bg-background/60 px-2 py-1.5">
      <span className="shrink-0">
        {running ? (
          <Loader2 className="h-3 w-3 animate-spin text-primary" />
        ) : (
          <CheckCircle2 className="h-3 w-3 text-green-500" />
        )}
      </span>
      <Terminal className="h-3 w-3 shrink-0 text-muted-foreground/60" />
      <span className="min-w-0 flex-1 truncate font-mono text-[10px]">
        {task.command.slice(0, 50)}
      </span>
      <span className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground">
        {formatElapsed(task.started_at_ms)}
      </span>
      {running && (
        <button
          onClick={() => onKill(task.id)}
          className="shrink-0 rounded p-0.5 text-muted-foreground/60 transition-colors hover:text-destructive"
          title={t("activity.killTask")}
          aria-label={t("activity.killTask")}
        >
          <X className="h-3 w-3" />
        </button>
      )}
    </div>
  );
}

/** One background main-agent turn — jump back or stop it. */
function BackgroundSessionRow({
  turn,
  onJump,
  onStop,
}: {
  turn: RunningTurnInfo;
  onJump: () => void;
  onStop: () => void;
}) {
  const { t } = useTranslation();
  const paused = turn.status === "paused";

  return (
    <div className="flex items-center gap-1.5 rounded-md border border-border/60 bg-background/60 px-2 py-1.5">
      <span className="shrink-0">
        {paused ? (
          <span className="inline-block h-3 w-3 rounded-full bg-amber-500" />
        ) : (
          <Loader2 className="h-3 w-3 animate-spin text-primary" />
        )}
      </span>
      <Bot className="h-3 w-3 shrink-0 text-muted-foreground/60" />
      <span className="min-w-0 flex-1 truncate text-[11px] font-medium">
        {turn.message_preview || turn.session_id}
      </span>
      <span className="shrink-0 rounded bg-muted px-1 py-px text-[9px] text-muted-foreground/70">
        {turn.work_mode === "depwork" ? "Depwork" : "Code"}
      </span>
      <span className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground">
        {formatElapsed(turn.started_at_ms)}
      </span>
      <button
        onClick={onJump}
        className="shrink-0 rounded p-0.5 text-muted-foreground/60 transition-colors hover:text-foreground"
        title={t("activity.jumpToSession")}
        aria-label={t("activity.jumpToSession")}
      >
        <ArrowRight className="h-3 w-3" />
      </button>
      <button
        onClick={onStop}
        className="shrink-0 rounded p-0.5 text-muted-foreground/60 transition-colors hover:text-destructive"
        title={t("activity.stopSession")}
        aria-label={t("activity.stopSession")}
      >
        <Square className="h-3 w-3" />
      </button>
    </div>
  );
}

export function AgentActivityCard({ isDepwork = false }: { isDepwork?: boolean }) {
  const { t } = useTranslation();
  const [tasks, setTasks] = useState<BackgroundTaskInfo[]>([]);
  // 1s ticker — forces re-render so elapsed durations stay live, but only
  // while there is something running to count (no busy-wait on idle states).
  const [, setTick] = useState(0);

  // The activity card is shared by both modes' activity pane. Pick the session
  // from the MODE's own store — merging `code ?? depwork` would leak the other
  // mode's stale session id (setMode doesn't clear it) and query the wrong
  // session's background tasks.
  const chatSessionId = useChatStore((s) => s.currentSessionId);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);
  const sessionId = isDepwork ? depworkSessionId : chatSessionId;
  const runningTurns = useRunningSessionsStore((s) => s.running);
  const refreshRunning = useRunningSessionsStore((s) => s.refresh);
  const { selectSessionById } = useSessionRestore();
  // Only THIS mode's background sessions — the registry is global, and the
  // other mode's turns must not bleed into this mode's activity pane.
  const modeTurns = runningTurns.filter(
    (t) => (t.work_mode === "depwork") === isDepwork,
  );

  const refresh = useCallback(async () => {
    try {
      const ts = sessionId
        ? await agentApi.listBackgroundTasks(sessionId)
        : [];
      setTasks(ts);
    } catch {
      // Backend unavailable — keep last state.
    }
  }, [sessionId]);

  useEffect(() => {
    void refresh();
    // The backend pushes status changes; refresh immediately on those so the
    // panel stays live without waiting for the next poll tick.
    // Background tabs don't need live data — skip poll ticks while hidden
    // and refresh once when the tab becomes visible again.
    const onVisibility = () => {
      if (!document.hidden) void refresh();
    };
    const iv = setInterval(() => {
      if (!document.hidden) void refresh();
    }, POLL_INTERVAL_MS);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      clearInterval(iv);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [refresh]);

  useTauriEvent<AgentStatus>("agent-status-changed", () => {
    void refresh();
  });

  const activeTurns = modeTurns.filter((t) => t.status === "running").length;
  const activeTasks = tasks.filter(
    (t) => t.status === "running" || t.status === "pending",
  ).length;
  const activeCount = activeTurns + activeTasks;
  const hasContent =
    tasks.length > 0 || modeTurns.length > 0;

  // Elapsed-time ticker: active only while there are live rows to time.
  useEffect(() => {
    if (!hasContent) return;
    const iv = setInterval(() => setTick((v) => v + 1), TICK_INTERVAL_MS);
    return () => clearInterval(iv);
  }, [hasContent]);

  const killTask = useCallback((taskId: string) => {
    void agentApi.killBackgroundTask(taskId).then((ok) => {
      if (ok) void refresh();
    });
  }, [refresh]);

  const stopRunningTurn = useCallback(
    (turn: RunningTurnInfo) => {
      void systemApi.cancelOperation(turn.session_id).then(() => {
        void refreshRunning();
        void refresh();
      });
    },
    [refresh, refreshRunning],
  );

  return (
    <CollapsibleCard
      icon={Bot}
      title={t("activity.title")}
      badge={
        activeCount > 0 ? t("activity.active", { count: activeCount }) : t("activity.idle")
      }
    >
      {/* Background sessions — main-agent turns still running */}
      {modeTurns.length > 0 && (
        <div className="space-y-1">
          <p className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            {t("activity.backgroundSessions")}
          </p>
          {modeTurns.map((turn) => (
            <BackgroundSessionRow
              key={turn.session_id}
              turn={turn}
              onJump={() => void selectSessionById(turn.session_id)}
              onStop={() => stopRunningTurn(turn)}
            />
          ))}
        </div>
      )}

      {/* Background tasks */}
      {tasks.length > 0 && (
        <div className="space-y-1">
          <p className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            {t("activity.backgroundTasks")}
          </p>
          {tasks.map((task) => (
            <TaskRow key={task.id} task={task} onKill={killTask} />
          ))}
        </div>
      )}

      {/* Empty state */}
      {!hasContent && (
        <p className="px-1 py-1 text-[11px] text-muted-foreground/60">
          {t("activity.noActivity")}
        </p>
      )}
    </CollapsibleCard>
  );
}
