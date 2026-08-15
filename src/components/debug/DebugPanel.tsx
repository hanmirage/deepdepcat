/**
 * DebugPanel — bottom-docked event log viewer.
 *
 * Shows when `debugMode` is enabled in appStore.
 * Displays a real-time scrollable list of DebugEvents from the backend.
 * Supports filtering by event type, pausing capture, and clearing.
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Bug, Pause, Play, Trash2, X, History } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { useAppStore } from "@/stores/appStore";
import { useDebugStore } from "@/stores/debugStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { sessionApi, type AgentEvent } from "@/lib/tauri";
import { FeatureFlagsSection } from "@/components/debug/FeatureFlagsSection";
import { DEBUG_EVENT_CATEGORIES, type DebugEvent, type DebugEventType } from "@/types";
import { cn } from "@/lib/utils";

/** Color per event category — drives the left-border accent. */
const CATEGORY_COLORS: Record<string, string> = {
  agent: "border-l-blue-500",
  llm: "border-l-purple-500",
  tool: "border-l-green-500",
  memory: "border-l-yellow-500",
  permission: "border-l-red-500",
  hook: "border-l-indigo-500",
  session: "border-l-cyan-500",
};

/** Map event type to its category label. */
const TYPE_TO_CATEGORY: Record<DebugEventType, string> = Object.fromEntries(
  DEBUG_EVENT_CATEGORIES.flatMap((cat) => cat.types.map((t) => [t, cat.label.toLowerCase() as string])),
) as Record<DebugEventType, string>;

/** Accent color per persisted event kind. */
const KIND_COLORS: Record<string, string> = {
  model_call: "border-l-purple-500",
  tool_run: "border-l-green-500",
  approval: "border-l-red-500",
  edit: "border-l-blue-500",
};

/** One-line summary of a persisted event's payload. */
function persistedSummary(event: AgentEvent): string {
  const p = event.payload;
  switch (event.kind) {
    case "model_call": {
      const usage = p.usage as { prompt?: number; completion?: number } | undefined;
      return `${String(p.model ?? "?")} · finish=${String(p.finish_reason ?? "?")} · ${usage?.prompt ?? 0}+${usage?.completion ?? 0} tok`;
    }
    case "tool_run":
      return `${String(p.tool ?? "?")} · ${p.is_error ? "error" : "ok"}${p.hook_blocked ? " · hook-blocked" : ""} · args=${String(p.args_len ?? 0)}B · result=${String(p.result_len ?? 0)}B`;
    case "approval": {
      const scope = p.scope === "none" ? "" : ` · ${String(p.scope)}`;
      return `${String(p.tool ?? "?")} · ${String(p.decision ?? "?")}${scope}`;
    }
    case "edit":
      return `${String(p.tool ?? "?")} · ${String(p.path ?? "?")}`;
    default:
      return JSON.stringify(p).slice(0, 120);
  }
}

/** Render a single persisted agent event row. */
function PersistedEventRow({ event }: { event: AgentEvent }) {
  const colorClass = KIND_COLORS[event.kind] ?? "border-l-muted";
  const time = new Date(event.created_at).toLocaleTimeString([], { hour12: false });
  return (
    <div className={cn("flex items-center gap-2 border-l-2 py-1 pl-2 pr-3 text-xs", colorClass)}>
      <span className="shrink-0 font-mono text-[10px] text-muted-foreground">{time}</span>
      <Badge variant="secondary" className="shrink-0 text-[9px] font-normal">
        #{event.seq} {event.kind}
      </Badge>
      <span className="truncate text-foreground">{persistedSummary(event)}</span>
    </div>
  );
}

/** Render a single debug event row. */
function DebugEventRow({ event }: { event: DebugEvent }) {
  const category = TYPE_TO_CATEGORY[event.type] ?? "unknown";
  const colorClass = CATEGORY_COLORS[category] ?? "border-l-muted";
  const time = new Date(event.timestamp).toLocaleTimeString([], { hour12: false });

  const detail = (() => {
    switch (event.type) {
      case "agent_turn_start":
        return `turn #${event.turn} · ${event.mode}`;
      case "agent_turn_end":
        return `turn #${event.turn} · ${event.duration_ms}ms`;
      case "llm_call_start":
        return `${event.model} · ${event.message_count} msgs`;
      case "llm_call_end":
        return `${event.model} · ${event.duration_ms}ms · ${event.usage.prompt_tokens}+${event.usage.completion_tokens} tok`;
      case "tool_dispatch":
        return `${event.tool_name}`;
      case "tool_result":
        return `${event.tool_name} · ${event.duration_ms}ms · ${event.is_error ? "error" : "ok"}`;
      case "memory_search":
        return `${event.results_count} results · ${event.duration_ms}ms`;
      case "memory_inject":
        return `${event.memories_count} memories`;
      case "permission_check":
        return `${event.resource} · ${event.action} · ${event.allowed ? "allow" : "deny"}`;
      case "hook_trigger":
        return event.event;
      case "hook_execute":
        return `${event.event} · ${event.hook_id} · ${event.duration_ms}ms`;
      case "compaction":
        return `${event.compacted_tokens} tokens`;
      case "session_create":
        return `${event.model} · ${event.provider}`;
    }
  })();

  return (
    <div className={cn("flex items-center gap-2 border-l-2 py-1 pl-2 pr-3 text-xs", colorClass)}>
      <span className="shrink-0 font-mono text-[10px] text-muted-foreground">{time}</span>
      <Badge variant="secondary" className="shrink-0 text-[9px] font-normal">
        {event.type}
      </Badge>
      <span className="shrink-0 font-mono text-[10px] text-muted-foreground">
        {event.session_id.slice(0, 8)}
      </span>
      <span className="truncate text-foreground">{detail}</span>
    </div>
  );
}

/** Filter toggle bar. */
function FilterBar() {
  const activeFilters = useDebugStore((s) => s.activeFilters);
  const toggleFilter = useDebugStore((s) => s.toggleFilter);

  return (
    <div className="flex flex-wrap gap-1">
      {DEBUG_EVENT_CATEGORIES.map((cat) => {
        const active = cat.types.some((t) => activeFilters.has(t));
        return (
          <button
            key={cat.label}
            onClick={() => cat.types.forEach((t) => toggleFilter(t))}
            className={cn(
              "rounded-full px-2 py-0.5 text-[10px] transition-colors",
              active
                ? "bg-primary text-primary-foreground"
                : "bg-secondary text-secondary-foreground hover:bg-secondary/80",
            )}
          >
            {cat.label}
          </button>
        );
      })}
    </div>
  );
}

export interface DebugPanelProps {
  className?: string;
}

export function DebugPanel({ className }: DebugPanelProps) {
  const { t } = useTranslation();
  const debugMode = useAppStore((s) => s.debugMode);
  const setDebugMode = useAppStore((s) => s.setDebugMode);
  const events = useDebugStore((s) => s.events);
  const paused = useDebugStore((s) => s.paused);
  const togglePause = useDebugStore((s) => s.togglePause);
  const clearEvents = useDebugStore((s) => s.clearEvents);
  const activeFilters = useDebugStore((s) => s.activeFilters);
  const chatSessionId = useChatStore((s) => s.currentSessionId);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);
  // Persisted replay-exact event log view (null = hidden).
  const [persistedEvents, setPersistedEvents] = useState<AgentEvent[] | null>(null);
  const [persistLoading, setPersistLoading] = useState(false);

  const scrollRef = useRef<HTMLDivElement>(null);
  // Auto-scroll ONLY while the user is already at the bottom — reading an
  // older event must not be yanked down by every new log line.
  const stickToBottomRef = useRef(true);

  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickToBottomRef.current) el.scrollTop = el.scrollHeight;
  }, [events]);

  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    stickToBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  };

  // Filtered events
  const filtered = activeFilters.size === 0
    ? events
    : events.filter((e) => activeFilters.has(e.type));

  const togglePersisted = async () => {
    if (persistedEvents !== null) {
      setPersistedEvents(null);
      return;
    }
    const sessionId =
      useAppStore.getState().mode === "depwork" ? depworkSessionId : chatSessionId;
    if (!sessionId) return;
    setPersistLoading(true);
    try {
      setPersistedEvents(await sessionApi.getSessionEvents(sessionId, 200));
    } catch {
      // best-effort — event list stays empty on failure
    } finally {
      setPersistLoading(false);
    }
  };

  if (!debugMode) return null;

  return (
    <div className={cn("flex h-48 flex-col border-t border-border bg-card", className)}>
      {/* ── Header ─────────────────────────────────────────────── */}
      <div className="flex items-center gap-2 border-b px-3 py-1.5">
        <Bug className="h-3.5 w-3.5 text-muted-foreground" />
        <span className="text-xs font-semibold">{t("debug.title")}</span>
        <Badge variant="secondary" className="text-[9px]">
          {filtered.length}
        </Badge>

        <div className="flex-1" />

        {/* Replay-exact persisted event log (session audit view) */}
        <Button
          variant="ghost"
          size="icon-sm"
          onClick={() => void togglePersisted()}
          disabled={persistLoading}
          aria-label={t("debug.persistLog", { defaultValue: "持久化事件" })}
          title={t("debug.persistLog", { defaultValue: "持久化事件" })}
        >
          <History className="h-3 w-3" />
        </Button>

        {/* Pause/Resume */}
        <Button variant="ghost" size="icon-sm" onClick={togglePause} aria-label={paused ? t("debug.resume", { defaultValue: "继续" }) : t("debug.pause", { defaultValue: "暂停" })}>
          {paused ? (
            <Play className="h-3 w-3" />
          ) : (
            <Pause className="h-3 w-3" />
          )}
        </Button>

        {/* Clear */}
        <Button variant="ghost" size="icon-sm" onClick={clearEvents} aria-label={t("debug.clear", { defaultValue: "清空" })}>
          <Trash2 className="h-3 w-3" />
        </Button>

        {/* Close */}
        <Button variant="ghost" size="icon-sm" onClick={() => setDebugMode(false)} aria-label={t("common.close")}>
          <X className="h-3 w-3" />
        </Button>
      </div>

      {/* ── Filter bar ──────────────────────────────────────────── */}
      <div className="flex items-center gap-2 border-b px-3 py-1">
        <FilterBar />
      </div>

      {/* ── Feature flags (developer toggles) ────────────────────── */}
      <FeatureFlagsSection />

      {/* ── Event list (live debug events OR persisted agent events) ── */}
      <div ref={scrollRef} onScroll={handleScroll} className="flex-1 overflow-y-auto p-1">
        <div className="space-y-0.5">
          {persistedEvents !== null ? (
            persistedEvents.length === 0 ? (
              <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
                {t("debug.persistEmpty", { defaultValue: "暂无持久化事件" })}
              </div>
            ) : (
              persistedEvents.map((event) => (
                <PersistedEventRow key={event.id} event={event} />
              ))
            )
          ) : filtered.length === 0 ? (
            <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
              {events.length === 0 ? t("debug.waiting") : t("debug.noMatch")}
            </div>
          ) : (
            filtered.map((event, i) => (
              <DebugEventRow key={`${event.timestamp}-${i}`} event={event} />
            ))
          )}
        </div>
      </div>
    </div>
  );
}
