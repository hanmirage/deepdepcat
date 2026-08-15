/**
 * ToolCallCard — terminal-log tool line, NO container.
 *
 * Claude Code style: a bare text row in the stream —
 *
 *   ● 读取中 ihrm.html    00:42 >
 *   · 已编辑 ihrm.html  +12 -3    00:42 >
 *   ✗ 编辑失败 ihrm.html  未找到匹配文本
 *
 * No border, no background, no frosted glass — just the narrative. While a
 * tool RUNS the verb+target row carries a text shimmer (the single focal
 * motion). Details (arguments / diff / result) live inside the lightweight
 * expanded area, which is the only boxed part.
 *
 * Verbs come from toolNarrative (i18n-driven); targets are extracted per
 * tool family; elapsed time ticks live while running. The row is deliberately
 * bare — no badges, no chips — ending in a muted terminal-prompt `>` that
 * also expands the details.
 */

import { useState, useMemo, memo, useEffect, useRef } from "react";
import * as Collapsible from "@radix-ui/react-collapsible";
import {
  XCircle,
  Check,
  Loader2,
  Bot,
} from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ToolCallState } from "@/types";
import { getToolIcon } from "@/config/toolIcons";
import {
  toolVerbKey,
  extractTarget,
  formatElapsedMs,
} from "@/config/toolNarrative";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useChatStore, type ChatState } from "@/stores/chatStore";
import { useStore } from "zustand";
import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { computeDiffStats } from "@/lib/diffStats";
import { cn } from "@/lib/utils";
import {
  DiffBadge,
  ResultBlock,
  LiveProgress,
  ArgsBlock,
  parseArgs,
  parseExitCode,
  toolCardFor,
} from "@/components/chat/ToolCallCardParts";

export { liveTail } from "@/components/chat/ToolCallCardParts";
import { VerbSwap } from "@/components/chat/VerbSwap";
import { McpAppView } from "@/components/chat/McpAppView";

export interface ToolCallCardProps {
  tool: ToolCallState;
}

function targetClassName(name: string): string {
  if (name === "bash" || name === "run_command") {
    return "font-mono text-[11px] text-foreground/90";
  }
  if (name === "grep" || name === "glob" || name === "memory_search") {
    return "font-mono text-[11px] text-amber-600/90 dark:text-amber-400/90";
  }
  if (name === "agent") {
    return "truncate text-xs text-foreground/80";
  }
  return "font-mono text-[11px] text-sky-600 dark:text-sky-400";
}

/**
 * ToolElapsed — self-ticking elapsed label.
 *
 * Owns its own 1s interval so a running tool re-renders ONLY this span,
 * not the whole card (the card hosts LiveProgress with an 800-char ANSI
 * tail — re-parsing that every second was the streaming jank source with
 * several parallel tools).
 */
function ToolElapsed({ startedAt, running }: { startedAt?: number; running: boolean }) {
  const [, setTick] = useState(0);
  useEffect(() => {
    if (!running) return;
    const iv = setInterval(() => setTick((v) => v + 1), 1000);
    return () => clearInterval(iv);
  }, [running]);
  const elapsedMs = startedAt ? Math.max(0, Date.now() - startedAt) : 0;
  if (elapsedMs <= 0) return null;
  return (
    <span className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground/50">
      {formatElapsedMs(elapsedMs)}
    </span>
  );
}

// ── Main component ─────────────────────────────────────────

function ToolCallCardImpl({ tool }: ToolCallCardProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const isRunning = tool.status === "running";
  const isError = tool.status === "error";

  // MCP Apps arrive after the tool result — auto-expand the card so the
  // interactive UI is immediately visible (one-shot: only when the app
  // first appears).
  const mcpApp = tool.mcpApp;
  useEffect(() => {
    if (mcpApp) setOpen(true);
  }, [mcpApp]);

  // Errors auto-expand — the failure details are the point of the row.
  useEffect(() => {
    if (isError) setOpen(true);
  }, [isError]);

  // "Settle" — a tool the user expanded to watch its streaming arguments
  // folds back the moment it completes (running → done), so a finished turn
  // reads as compact one-liners (the folded row now carries a result summary
  // instead). Errors and MCP apps stay open (guarded above); a tool re-opened
  // AFTER completion stays open (no further transition to re-fold it).
  const wasRunning = useRef(false);
  useEffect(() => {
    const prev = wasRunning.current;
    wasRunning.current = isRunning;
    if (prev && !isRunning && !isError && !mcpApp) setOpen(false);
  }, [isRunning, isError, mcpApp]);

  const appMode = useAppStore((s) => s.mode);
  const chatStore = appMode === "depwork" ? useDepworkChatStore : useChatStore;

  const Icon = getToolIcon(tool.name);
  const args = useMemo(() => parseArgs(tool.arguments), [tool.arguments]);
  const target = useMemo(() => extractTarget(tool.name, args), [tool.name, args]);
  const filePath = useMemo(() => {
    if (!["read_file", "write_file", "edit_file", "search_replace"].includes(tool.name)) {
      return null;
    }
    return typeof args.path === "string" ? args.path : null;
  }, [tool.name, args]);
  const diffStats = useMemo(
    () => computeDiffStats(tool.name, tool.arguments),
    [tool.name, tool.arguments],
  );
  const runningVerb = toolVerbKey(tool.name, "running");
  const doneVerb = toolVerbKey(tool.name, isError ? "error" : "done");

  // Live subagent state lives in the ACTIVE mode's store — code sessions
  // and depwork sessions keep separate subagent maps.
  const linkedSubagent = useStore(
    chatStore,
    (s: ChatState) =>
      isRunning && tool.name === "agent"
        ? Object.values(s.subagents).find((sa) => sa.tool_call_id === tool.id) ?? null
        : null,
  );

  const errorSummary = useMemo(() => {
    if (!isError || !tool.result) return null;
    // Full first error line — the row's `truncate` class clips display
    // without losing text the user needs.
    return tool.result.split("\n")[0]?.trim() ?? null;
  }, [isError, tool.result]);

  // A non-zero bash exit is "ran but failed" — a distinct warning on the
  // folded row (status is still done; the BashCard shows the exact code).
  const exitFailed = useMemo(
    () =>
      (tool.name === "bash" || tool.name === "run_command") &&
      tool.status === "done" &&
      (parseExitCode(tool.result) ?? 0) !== 0,
    [tool.name, tool.status, tool.result],
  );

  // Compact success summary — a folded done-row should say what happened,
  // not just "✓ 完成". Only for COMPACT results (a grep hit list, a shell
  // echo, a search answer); a file dump or long page text stays behind the
  // chevron so the folded row never turns into a content preview. A LONG
  // grep result keeps its hit-count line instead of vanishing silently.
  const doneSummary = useMemo(() => {
    if (tool.status !== "done" || !tool.result) return null;
    if ((tool.name === "grep" || tool.name === "glob") && tool.result.length > 400) {
      const found = tool.result.match(/Found (\d+) matches? in (\d+) files?/);
      return found ? `Found ${found[1]} matches in ${found[2]} files` : null;
    }
    if (tool.result.length > 400) return null;
    const firstLine = tool.result.split("\n").find((l) => l.trim()) ?? "";
    const oneLine = firstLine.replace(/\s+/g, " ").trim();
    if (!oneLine) return null;
    return oneLine.length > 60 ? `${oneLine.slice(0, 59)}…` : oneLine;
  }, [tool.status, tool.result, tool.name]);
  const revealFile = useRightPanelStore((s) => s.revealFile);

  return (
    <Collapsible.Root
      open={open}
      onOpenChange={setOpen}
      data-tool-id={tool.id}
      // Slide-in on mount (stable key = plays once, never on stream flushes).
      className="tool-card-enter w-full"
    >
      <div className="group flex w-full items-center gap-1.5">
        <Collapsible.Trigger asChild>
          {/* Bare text row — no container. Running rows carry the shimmer. */}
          <button
            className={cn(
              // min-h-5: every tool row is the same height — verb swaps,
              // badges and state changes never make the column jump.
              "flex min-h-5 min-w-0 flex-1 items-center gap-1.5 py-0.5 text-left transition-colors",
              isRunning && "tool-running-accent",
              isError ? "hover:bg-destructive/5" : "hover:bg-muted/20",
            )}
            aria-expanded={open}
            aria-label={`${t("toolCall.ariaLabel", { name: tool.name })} — ${
              isRunning
                ? t("toolCall.ariaRunning")
                : isError
                  ? t("toolCall.ariaError")
                  : t("toolCall.ariaDone")
            }`}
          >
          {/* Status — the running spinner carries a soft breathing glow
              (box-shadow only, CSS composited — reads as "alive, working").
              Done is a hairline check in muted gray (carries the "done"
              semantics without a loud green — only errors show the red X,
              so the row reads at a glance). */}
          <span className={cn("flex w-3.5 shrink-0 justify-center", isRunning && "animate-glow-pulse")}>
            {isRunning ? (
              <Loader2 className="h-3 w-3 animate-spin text-primary" />
            ) : isError ? (
              <XCircle className="h-3 w-3 text-destructive" />
            ) : (
              <Check className="h-2.5 w-2.5 text-muted-foreground/40" strokeWidth={2.5} />
            )}
          </span>

          {/* Tool icon */}
          <Icon className="h-3 w-3 shrink-0 opacity-60" />

          {/* Narrative — verb + target. The verb is a cross-fading
              state word (VerbSwap): shimmer while running, then the done
              verb fades in with a width transition so the row never jumps.
              The target is visible while running too ("读取中 x.rs"), but
              the file jump stays locked until the call completes. */}
          <span className="flex min-w-0 flex-1 items-baseline gap-1.5">
            <VerbSwap
              activeText={t(runningVerb)}
              doneText={t(doneVerb)}
              active={isRunning}
              className={cn(
                "shrink-0 text-[11px] font-medium",
                isError ? "text-destructive/90" : "text-foreground/75",
              )}
            />

            {target && (
              filePath && !isRunning ? (
                <span
                  role="button"
                  tabIndex={0}
                  title={filePath}
                  onClick={(e) => {
                    e.stopPropagation();
                    revealFile(appMode, filePath);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      e.stopPropagation();
                      revealFile(appMode, filePath);
                    }
                  }}
                  className={cn(
                    "truncate cursor-pointer transition-opacity hover:underline",
                    isRunning ? "opacity-100" : "opacity-70 hover:opacity-100",
                    targetClassName(tool.name),
                  )}
                >
                  {target}
                </span>
              ) : (
                <span
                  className={cn(
                    "truncate transition-opacity",
                    isRunning ? "opacity-100" : "opacity-70",
                    // A non-zero exit renders the target amber — "ran but
                    // failed", distinct from a hard tool error.
                    exitFailed
                      ? "text-amber-600 dark:text-amber-400"
                      : targetClassName(tool.name),
                  )}
                >
                  {target}
                </span>
              )
            )}
          </span>

          {/* Diff stats — changes render inline in the chat already. */}
          {diffStats && <DiffBadge stats={diffStats} />}

          {/* Error summary */}
          {isError && errorSummary && (
            <span className="hidden min-w-0 max-w-[40%] truncate text-[11px] text-destructive/80 sm:inline">
              — {errorSummary}
            </span>
          )}

          {/* Done summary — a compact result snippet so the folded row reads
              "✓ 已读取 x.rs · <result>" without expanding. */}
          {!isError && doneSummary && (
            <span className="hidden min-w-0 max-w-[40%] truncate text-[11px] text-muted-foreground/70 sm:inline">
              · {doneSummary}
            </span>
          )}

          {/* Elapsed — self-ticking child, the card itself never re-renders
              on the 1s cadence. */}
          <ToolElapsed startedAt={tool.startedAt} running={isRunning} />

          {/* Expand chevron — always available: while running it reveals
              the live arguments + output (observable autonomy). A muted
              terminal-prompt `>` that rotates when open. */}
          <span
            className={cn(
              "shrink-0 font-mono text-[11px] font-semibold text-muted-foreground/40 transition-transform duration-200 group-hover:text-muted-foreground",
              open && "rotate-90",
            )}
            aria-hidden="true"
          >
            &gt;
          </span>
        </button>
      </Collapsible.Trigger>
    </div>

      {/* Linked subagent live state — its own bare row under the tool line */}
      {linkedSubagent && (
        <div className="flex items-center gap-1.5 py-0.5 pl-5 text-[10px] text-muted-foreground">
          <Bot className="h-3 w-3 shrink-0 text-primary" />
          <span className="min-w-0 flex-1 truncate">
            <span className="font-medium text-foreground/70">{linkedSubagent.agent_type}</span>
            {" · "}
            {linkedSubagent.turn}/{linkedSubagent.total_turns || "?"}
            {linkedSubagent.lastMessage ? ` · ${linkedSubagent.lastMessage}` : ""}
          </span>
          <span className="shrink-0 font-mono tabular-nums">
            {formatElapsedMs(Date.now() - linkedSubagent.startedAt)}
          </span>
        </div>
      )}

      {/* Live streamed progress (running only) */}
      {isRunning && <div className="pl-5 pt-1"><LiveProgress tool={tool} /></div>}

      {/* Expanded details — the ONLY boxed part: arguments / diff / result.
          The height cap is an INLINE style (not a Tailwind class) so it can
          never be purged or overridden: a huge result/args/diff scrolls
          inside the box instead of growing the card. overscroll-contain
          keeps the inner scroll from chaining into the message list. */}
      <Collapsible.Content
        className="tool-expand-enter pl-5 pt-1"
        style={{
          maxHeight: "24rem",
          overflowY: "auto",
          overflowX: "hidden",
          overscrollBehavior: "contain",
        }}
      >
        <div className="space-y-1.5 rounded-lg border border-border/60 bg-card/60 p-1.5 shadow-sm">
          {mcpApp && (
            <McpAppView app={mcpApp} argumentsJson={tool.arguments} resultText={tool.result} />
          )}
          {toolCardFor(tool) ?? <ArgsBlock tool={tool} />}
          <ResultBlock tool={tool} />
        </div>
      </Collapsible.Content>
    </Collapsible.Root>
  );
}

const areToolsEqual = (prev: ToolCallCardProps, next: ToolCallCardProps) =>
  prev.tool.id === next.tool.id &&
  prev.tool.status === next.tool.status &&
  prev.tool.arguments === next.tool.arguments &&
  prev.tool.result === next.tool.result &&
  prev.tool.progressKind === next.tool.progressKind &&
  prev.tool.progressDelta === next.tool.progressDelta &&
  prev.tool.progressTotalBytes === next.tool.progressTotalBytes &&
  prev.tool.mcpApp === next.tool.mcpApp;

export const ToolCallCard = memo(ToolCallCardImpl, areToolsEqual);
