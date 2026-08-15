/**
 * ContextUsageRing — small circular context/budget usage indicator.
 *
 * Renders an SVG ring (like Claude Desktop's context usage dot) showing how
 * much of the session's budget is used. The percentage is the session token
 * total against the session's REAL model context window (reported by the
 * backend from the live ChatState, so it follows model switches). When the
 * backend reports an unknown window (0), a fixed fallback budget is used.
 * Color shifts as usage grows:
 *   < 60%  neutral   60–90% amber    ≥90% red
 *
 * Click opens a context panel (self-contained popover — same pattern as
 * NotificationBell: external click / Esc closes): current usage vs window,
 * plus the recent compaction history (records fed by the chat store's
 * compaction handler), so "what did the model see / when was it compressed"
 * is one click away.
 *
 * Data comes from sessionApi.getSessionUsage() (backend tracks per-session
 * token totals).
 */

import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { History, Zap } from "lucide-react";
import { sessionApi, type SessionUsageSummary, type ContextBreakdown } from "@/lib/tauri";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useStore } from "zustand";
import { cn } from "@/lib/utils";
import { estimateTokens } from "@/components/chat/StreamTokenCounter";

const RADIUS = 5;
const CIRCUMFERENCE = 2 * Math.PI * RADIUS;

/** Fallback budget in tokens when the backend reports no context window. */
const FALLBACK_BUDGET = 1_000_000;

export interface ContextUsageRingProps {
  /** Active session id — when null (no session yet) the ring hides. */
  sessionId: string | null;
  /** Which chat store drives the stream state — refetch on stream end. */
  mode?: "code" | "depwork";
  className?: string;
}

function formatClock(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** One category row in the breakdown panel: colored bar + label + tokens. */
function BreakdownRow({
  label,
  tokens,
  total,
  color,
}: {
  label: string;
  tokens: number;
  total: number;
  color: string;
}) {
  if (tokens <= 0) return null;
  const pct = total > 0 ? Math.min((tokens / total) * 100, 100) : 0;
  return (
    <div className="flex items-center gap-2">
      <div className="relative h-1.5 w-14 shrink-0 overflow-hidden rounded-full bg-border/50">
        <div
          className="absolute inset-y-0 left-0 rounded-full"
          style={{ width: `${pct}%`, backgroundColor: color }}
        />
      </div>
      <span className="min-w-0 flex-1 truncate text-[10px] text-muted-foreground/80">
        {label}
      </span>
      <span className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground">
        {tokens.toLocaleString()}
      </span>
    </div>
  );
}

export function ContextUsageRing({ sessionId, mode = "code", className }: ContextUsageRingProps) {
  const { t } = useTranslation();
  const [usage, setUsage] = useState<SessionUsageSummary | null>(null);
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const chatStore = mode === "depwork" ? useDepworkChatStore : useChatStore;
  const isStreaming = useStore(chatStore, (s) => s.isStreaming);
  const prevStreaming = useRef(isStreaming);
  const compactions = useStore(chatStore, (s) => s.compactions);
  // Live token estimate of THIS turn's streamed output (text + reasoning) —
  // DeepSeek reports usage only at stream end, so while streaming the ring
  // adds this to the real anchor to project occupancy instead of sitting on
  // the previous turn's value. 0 when not streaming.
  const streamTokens = useStore(chatStore, (s) => {
    if (!s.isStreaming) return 0;
    let n = 0;
    for (const m of s.messages) {
      if (m.role !== "assistant" || !m.isStreaming) continue;
      for (const b of m.blocks) {
        if (b.type === "text" || b.type === "reasoning") n += estimateTokens(b.content);
      }
    }
    return n;
  });

  const fetchUsage = (sid: string) => {
    sessionApi
      .getSessionUsage(sid)
      .then((u) => setUsage(u))
      .catch(() => setUsage(null));
  };

  // Fetch on session change.
  useEffect(() => {
    if (!sessionId) {
      setUsage(null);
      return;
    }
    let cancelled = false;
    sessionApi
      .getSessionUsage(sessionId)
      .then((u) => {
        if (!cancelled) setUsage(u);
      })
      .catch(() => {
        if (!cancelled) setUsage(null);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId]);

  // Refetch when a stream turn ends — token usage is only final then.
  useEffect(() => {
    const ended = prevStreaming.current && !isStreaming;
    prevStreaming.current = isStreaming;
    if (ended && sessionId) fetchUsage(sessionId);
  }, [isStreaming, sessionId]);

  // Close on outside click / Esc.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (wrapperRef.current && !wrapperRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  if (!sessionId || !usage) return null;

  // API-billed tokens only — tool result tokens are a local estimate and are
  // never mixed into the ring's numbers.
  const totalTokens = usage.total_prompt_tokens + usage.total_completion_tokens;
  if (totalTokens === 0) return null;

  const budget = usage.context_window > 0 ? usage.context_window : FALLBACK_BUDGET;
  // Real context occupancy (last request's input size) — the metric
  // Claude/Grok indicators display. Fall back to the cumulative total only
  // when no turn has completed yet. While streaming this is projected
  // forward (anchor + this turn's streamed-output estimate) so the ring
  // rises live instead of freezing on the previous turn's value.
  const anchor =
    usage.current_context_tokens > 0
      ? usage.current_context_tokens
      : totalTokens;
  const current = isStreaming ? Math.round(anchor + streamTokens) : anchor;
  const fraction = Math.min(current / budget, 1);
  const percent = Math.round(fraction * 100);
  const dashOffset = CIRCUMFERENCE * (1 - fraction);

  const strokeColor =
    percent >= 90 ? "hsl(var(--destructive))" : percent >= 60 ? "hsl(var(--warning, 38 92% 50%))" : "hsl(var(--primary))";

  return (
    <div ref={wrapperRef} className={cn("relative", className)}>
      <button
        onClick={() => setOpen((o) => !o)}
        className="flex h-5 w-5 shrink-0 items-center justify-center rounded-sm transition-colors hover:bg-muted/60"
        title={`${t("chat.tokens")}: ${current.toLocaleString()} / ${budget.toLocaleString()} · ${percent}%\n${t("chat.contextRingHint", "含系统提示词与工具定义固定开销")}`}
        aria-label={t("chat.contextRingHint", "上下文占用")}
        aria-expanded={open}
      >
        <svg width="12" height="12" viewBox="0 0 12 12" className="rotate-[-90deg]">
          <circle cx="6" cy="6" r={RADIUS} stroke="hsl(var(--border))" strokeWidth="2" fill="none" />
          <circle
            cx="6"
            cy="6"
            r={RADIUS}
            stroke={strokeColor}
            strokeWidth="2"
            fill="none"
            strokeDasharray={CIRCUMFERENCE}
            strokeDashoffset={dashOffset}
            strokeLinecap="round"
          />
        </svg>
      </button>

      {/* ── Context panel ───────────────────────────────────── */}
      {open && (
        <div className="absolute bottom-full right-0 z-50 mb-2 w-72 overflow-hidden rounded-lg border bg-popover text-popover-foreground shadow-lg animate-in fade-in slide-in-from-bottom-2 duration-150">
          {/* Header — usage vs window */}
          <div className="flex items-center justify-between border-b px-3 py-2">
            <p className="text-xs font-medium">{t("chat.contextTitle", "上下文")}</p>
            <span
              className={cn(
                "rounded-full px-1.5 py-0.5 font-mono text-[10px] tabular-nums",
                percent >= 90
                  ? "bg-destructive/10 text-destructive"
                  : percent >= 60
                    ? "bg-amber-500/10 text-amber-600 dark:text-amber-400"
                    : "bg-primary/10 text-primary",
              )}
            >
              {current.toLocaleString()} / {budget.toLocaleString()} · {percent}%
            </span>
          </div>

          {/* Breakdown — what occupies the current context, by category */}
          <div className="space-y-1.5 border-b px-3 py-2">
            {(() => {
              const bd: ContextBreakdown | undefined = usage.context_breakdown;
              if (!bd) return null;
              const total = Math.max(
                bd.system_prompt_tokens +
                  bd.skill_tokens +
                  bd.tool_definition_tokens +
                  bd.conversation_tokens +
                  bd.tool_result_tokens,
                1,
              );
              return (
                <>
                  <p className="mb-1.5 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
                    {t("chat.contextBreakdown", "上下文占用分类")}
                  </p>
                  <BreakdownRow
                    label={t("chat.ctxSystem", "系统提示")}
                    tokens={bd.system_prompt_tokens}
                    total={total}
                    color="hsl(var(--primary))"
                  />
                  <BreakdownRow
                    label={t("chat.ctxSkills", "技能 Skills")}
                    tokens={bd.skill_tokens}
                    total={total}
                    color="hsl(262 70% 60%)"
                  />
                  <BreakdownRow
                    label={t("chat.ctxTools", "工具定义")}
                    tokens={bd.tool_definition_tokens}
                    total={total}
                    color="hsl(38 92% 55%)"
                  />
                  <BreakdownRow
                    label={t("chat.ctxConversation", "对话")}
                    tokens={bd.conversation_tokens}
                    total={total}
                    color="hsl(160 70% 45%)"
                  />
                  <BreakdownRow
                    label={t("chat.ctxToolResults", "工具结果")}
                    tokens={bd.tool_result_tokens}
                    total={total}
                    color="hsl(0 70% 60%)"
                  />
                </>
              );
            })()}
          </div>

          {/* Prefix-cache hit ratio — DeepSeek KV cache feedback loop.
              A live ratio over the recent turns surfaces a session with an
              unstable prefix (miss on every request) immediately, instead
              of hiding behind the first-request miss of a session average. */}
          {usage.cache_hit_ratio != null && (
            <div className="space-y-1.5 border-b px-3 py-2">
              <p className="mb-1.5 flex items-center gap-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
                <Zap className="h-3 w-3" />
                {t("chat.ctxCacheHit", "前缀缓存命中")}
              </p>
              <div className="flex items-center justify-between text-[11px]">
                <span className="text-foreground/80">
                  {t("chat.ctxCacheRatio", "最近 10 轮命中率")}
                </span>
                <span
                  className={cn(
                    "rounded-full px-1.5 py-0.5 font-mono text-[10px] tabular-nums",
                    usage.cache_hit_ratio >= 0.8
                      ? "bg-primary/10 text-primary"
                      : usage.cache_hit_ratio >= 0.5
                        ? "bg-amber-500/10 text-amber-600 dark:text-amber-400"
                        : "bg-destructive/10 text-destructive",
                  )}
                >
                  {Math.round(usage.cache_hit_ratio * 100)}%
                </span>
              </div>
              <div className="flex items-center justify-between text-[10px] text-muted-foreground">
                <span>
                  {t("chat.ctxCacheHitTokens", "命中")}{" "}
                  {usage.total_cache_hit_tokens.toLocaleString()} ·{" "}
                  {t("chat.ctxCacheMissTokens", "未命中")}{" "}
                  {usage.total_cache_miss_tokens.toLocaleString()}
                </span>
              </div>
              {usage.cache_hit_ratio < 0.5 && (
                <p className="text-[10px] leading-relaxed text-muted-foreground/80">
                  {t(
                    "chat.ctxCacheLowHint",
          t("chat.ctxCacheLowHint", {
            defaultValue: "前缀命中率低：保持同一会话连续提问可命中公共前缀缓存，降低费用。",
          }),
                  )}
                </p>
              )}
            </div>
          )}

          {/* Compaction history */}
          <div className="px-3 py-2">
            <p className="mb-1.5 flex items-center gap-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
              <History className="h-3 w-3" />
              {t("chat.contextCompactions", "压缩记录")}
            </p>
            {compactions.length === 0 ? (
              <p className="py-1 text-[11px] text-muted-foreground/70">
                {t("chat.contextNoCompactions", "暂无压缩记录")}
              </p>
            ) : (
              <div className="space-y-1.5">
                {compactions.map((c, i) => (
                  <div
                    key={i}
                    className="rounded-md border border-border/60 bg-muted/20 px-2 py-1.5"
                  >
                    <p className="flex items-center justify-between text-[10px]">
                      <span className="font-medium text-foreground/80">
                        {t("chat.contextSaved", "节省")} {c.tokens.toLocaleString()} tokens
                      </span>
                      <span className="font-mono tabular-nums text-muted-foreground">
                        {formatClock(c.at)}
                      </span>
                    </p>
                    {c.summary && (
                      <p
                        className="mt-0.5 line-clamp-2 text-[10px] leading-relaxed text-muted-foreground"
                        title={c.summary}
                      >
                        {c.summary}
                      </p>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>

          {/* Footer hint */}
          <div className="border-t px-3 py-1.5 text-[9px] text-muted-foreground/60">
            {t("chat.contextRingHint", "含系统提示词与工具定义固定开销")}
          </div>
        </div>
      )}
    </div>
  );
}
