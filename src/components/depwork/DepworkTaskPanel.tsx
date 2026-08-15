/**
 * DepworkTaskPanel — task-style execution card for Depwork mode.
 *
 * Derives the current task's steps from the latest assistant message's
 * tool_call blocks (no backend changes): a total progress bar, per-step
 * status rows with live streamed output, produced artifacts, and a stop
 * button. Gives Depwork the "task execution" feel of a Codex-style agent:
 * steps decomposed, progress visible, results reviewable.
 *
 * Lives inside the RightPanel as a card (the panel header already carries
 * the "task execution" title, so the card keeps only its status badge +
 * progress bar — no duplicate chrome).
 */

import { useMemo, useState, useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import {
  CheckCircle2,
  XCircle,
  Loader2,
  Square,
  Wrench,
  Pause,
  Play,
  Copy,
  Check,
  Target,
} from "lucide-react";
import type { TFunction } from "i18next";
import type { DepworkMessage, DepworkToolCallState } from "@/types/depwork";
import { getToolIcon } from "@/config/toolIcons";
import { formatElapsedMs, formatBytes } from "@/config/toolNarrative";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { sessionApi } from "@/lib/tauri";
import { AnsiText } from "@/components/chat/AnsiText";
import { cn } from "@/lib/utils";

interface DepworkTaskPanelProps {
  messages: DepworkMessage[];
  isStreaming: boolean;
  sessionId?: string | null;
}

const VERB_KEY: Record<string, string> = {
  doc_read: "depworkTask.verbDocRead",
  docx_generate: "depworkTask.verbDocxGenerate",
  ppt_generate: "depworkTask.verbPptGenerate",
  table_process: "depworkTask.verbTableProcess",
  batch_file: "depworkTask.verbBatchFile",
  ui_automate: "depworkTask.verbUiAutomate",
  web_fetch: "depworkTask.verbWebFetch",
  web_fetch_depwork: "depworkTask.verbWebFetch",
  web_open: "depworkTask.verbWebOpen",
  media_probe: "depworkTask.verbMediaProbe",
  media_convert: "depworkTask.verbMediaConvert",
  ocr_image: "depworkTask.verbOcrImage",
  chart_generate: "depworkTask.verbChartGenerate",
  write_file: "depworkTask.verbWriteFile",
  read_file: "depworkTask.verbReadFile",
  list_dir: "depworkTask.verbListDir",
  agent: "depworkTask.verbAgent",
};

function toolLabel(name: string, t: TFunction): string {
  const key = VERB_KEY[name];
  return key ? t(key) : name.replace(/_/g, " ");
}

function parseArgs(args: string): Record<string, unknown> {
  try {
    return args ? (JSON.parse(args) as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

/** Short human-readable target for a step row. */
function stepTarget(name: string, args: Record<string, unknown>): string | null {
  const get = (k: string): string | null =>
    typeof args[k] === "string" && args[k] ? (args[k] as string) : null;
  const path = get("output") ?? get("path") ?? get("input") ?? get("dir") ?? get("url");
  if (path) {
    const parts = path.split(/[\\/]/);
    return parts[parts.length - 1] ?? path;
  }
  const query = get("text") ?? get("template") ?? get("kind") ?? get("action");
  return query ? query.slice(0, 28) : null;
}

/** Artifacts = done tools whose args carry an output/to destination. */
function extractArtifacts(tools: DepworkToolCallState[]): { name: string; path: string }[] {
  const out: { name: string; path: string }[] = [];
  for (const t of tools) {
    if (t.status !== "done") continue;
    const args = parseArgs(t.arguments);
    const target =
      (typeof args.output === "string" && args.output) ||
      (typeof args.to === "string" && args.to) ||
      null;
    if (target) out.push({ name: t.name, path: target });
  }
  return out;
}

/**
 * ArtifactRow — one produced file as a card: tool icon stamp + file name
 * + containing directory + copy-path action. (No shell/open API in the
 * frontend, so copy-path is the primary action.)
 */
function ArtifactRow({ name, path }: { name: string; path: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const Icon = getToolIcon(name);
  const parts = path.split(/[\\/]/);
  const fileName = parts[parts.length - 1] ?? path;
  const dir = parts.length > 1 ? parts.slice(0, -1).join("/") : "";

  const copy = async () => {
    await navigator.clipboard.writeText(path);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="flex items-center gap-2 rounded-md border border-border/60 bg-background/50 px-2 py-1.5 transition-colors hover:bg-muted/30">
      <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-primary/10">
        <Icon className="h-3.5 w-3.5 text-primary" />
      </span>
      <div className="min-w-0 flex-1">
        <p className="truncate text-[10.5px] font-medium text-foreground/90">{fileName}</p>
        <p className="truncate font-mono text-[9px] text-muted-foreground/60">
          {dir || "."}
        </p>
      </div>
      <button
        onClick={copy}
        className={cn(
          "flex shrink-0 items-center gap-1 rounded px-1 py-0.5 text-[9px] transition-colors",
          copied
            ? "text-green-600"
            : "text-muted-foreground/50 hover:bg-muted hover:text-foreground",
        )}
        title={copied ? t("depworkTask.copiedPath") : t("depworkTask.copyPath")}
      >
        {copied ? <Check className="h-2.5 w-2.5" /> : <Copy className="h-2.5 w-2.5" />}
        {copied ? t("depworkTask.copiedPath") : t("depworkTask.copyPath")}
      </button>
    </div>
  );
}

export function DepworkTaskPanel({ messages, isStreaming, sessionId }: DepworkTaskPanelProps) {
  const { t } = useTranslation();
  const stopStreaming = useDepworkChatStore((s) => s.stopStreaming);
  const isPaused = useDepworkChatStore((s) => s.isPaused);
  const pauseStreaming = useDepworkChatStore((s) => s.pauseStreaming);
  const resumeStreaming = useDepworkChatStore((s) => s.resumeStreaming);
  const [goal, setGoal] = useState<string | null>(null);
  // Elapsed ticker — re-renders once per second while a step runs so the
  // mm:ss counter on the running step stays live (long doc/media tasks
  // have no progress events, the timer is the only progress signal).
  const [, setTick] = useState(0);

  // Session goal — fetch on session change, refresh when the plan updates.
  // The session this panel's goal fetches are FOR. Each request captures the
  // id at call time and drops the response if the panel has moved on — a
  // single boolean alive-flag is NOT enough (switching sessions re-arms it
  // before the old session's late response arrives, letting it overwrite).
  const goalSessionRef = useRef<string | null | undefined>(null);
  useEffect(() => {
    goalSessionRef.current = sessionId;
    if (!sessionId) {
      setGoal(null);
      return;
    }
    void sessionApi
      .getGoal(sessionId)
      .then((g) => {
        if (goalSessionRef.current === sessionId) setGoal(g);
      })
      .catch(() => {
        /* best-effort */
      });
  }, [sessionId]);
  useTauriEvent<{ session_id: string }>("todo-list-updated", (e) => {
    if (!sessionId || e.session_id !== sessionId) return;
    void sessionApi
      .getGoal(sessionId)
      .then((g) => {
        if (goalSessionRef.current === sessionId) setGoal(g);
      })
      .catch(() => {});
  });
  useTauriEvent<{ session_id: string; goal: string | null }>("goal-updated", (e) => {
    if (!sessionId || e.session_id !== sessionId) return;
    setGoal(e.goal ?? null);
  });

  useEffect(() => {
    if (!isStreaming) return;
    const iv = setInterval(() => setTick((v) => v + 1), 1000);
    return () => clearInterval(iv);
  }, [isStreaming]);

  // Latest assistant message → the current turn's tool steps.
  const { tools, artifacts } = useMemo(() => {
    const lastAssistant = [...messages].reverse().find((m) => m.role === "assistant");
    const tools: DepworkToolCallState[] = [];
    if (lastAssistant) {
      for (const block of lastAssistant.blocks) {
        if (block.type === "tool_call") tools.push(block.tool);
      }
    }
    return { tools, artifacts: extractArtifacts(tools) };
  }, [messages]);

  const { total, done, running, failed } = useMemo(() => {
    let done = 0;
    let running = 0;
    let failed = 0;
    for (const t of tools) {
      if (t.status === "done") done++;
      else if (t.status === "error") failed++;
      else if (t.status === "running") running++;
    }
    return { total: tools.length, done, running, failed };
  }, [tools]);

  const progressPct = total === 0 ? 0 : Math.round(((done + failed) / total) * 100);
  // Stop is destructive (kills the agent's current run) — two-step confirm.
  const [armedStop, setArmedStop] = useState(false);
  const handleStop = () => {
    if (!armedStop) {
      setArmedStop(true);
      setTimeout(() => setArmedStop(false), 3000);
      return;
    }
    setArmedStop(false);
    void stopStreaming();
  };

  if (total === 0) {
    return (
      <div className="space-y-2 p-3">
        {goal && (
          <div className="flex items-start gap-1.5 rounded-md bg-primary/5 px-2 py-1.5">
            <Target className="mt-0.5 h-3 w-3 shrink-0 text-primary" />
            <span className="min-w-0 flex-1 text-[11px] leading-snug text-foreground/80">
              {goal}
            </span>
          </div>
        )}
        <p className="px-1 py-1 text-[11px] text-muted-foreground/60">
          {t("depworkTask.empty")}
        </p>
      </div>
    );
  }

  return (
    <div className="space-y-2 p-3">
      {goal && (
        <div className="flex items-start gap-1.5 rounded-md bg-primary/5 px-2 py-1.5">
          <Target className="mt-0.5 h-3 w-3 shrink-0 text-primary" />
          <span className="min-w-0 flex-1 text-[11px] leading-snug text-foreground/80">
            {goal}
          </span>
        </div>
      )}
      <div className="overflow-hidden rounded-lg border border-border bg-muted/20">
      {/* ── Header: status + progress ── */}
      <div className="border-b border-border/60 px-3 py-2.5">
        <div className="flex items-center justify-between">
          <span
            className={cn(
              "rounded-full px-2 py-0.5 text-[10px] font-medium",
              isStreaming
                ? "bg-primary/10 text-primary"
                : failed > 0
                  ? "bg-destructive/10 text-destructive"
                  : "bg-muted text-muted-foreground",
            )}
          >
            {isStreaming
              ? t("depworkTask.running")
              : failed > 0
                ? t("depworkTask.hasFailed")
                : t("depworkTask.completed")}
          </span>
        </div>

        {/* Total progress bar — failed steps count as consumed (a run that
            failed everywhere must not be stuck at 0%). */}
        <div className="mt-2 h-1.5 w-full overflow-hidden rounded-full bg-muted">
          <div
            className={cn(
              "h-full rounded-full transition-all duration-300",
              failed > 0 ? "bg-destructive" : "bg-primary",
            )}
            style={{ width: `${progressPct}%` }}
          />
        </div>
        <p className="mt-1 text-[10px] tabular-nums text-muted-foreground/70">
          {t("depworkTask.stepsComplete", { done, total })}
          {running > 0 ? t("depworkTask.stepsRunning", { running }) : ""}
          {failed > 0 ? t("depworkTask.stepsFailed", { failed }) : ""}
        </p>
      </div>

      {/* ── Steps ── */}
      <div className="px-2 py-2">
        <div className="space-y-1">
          {tools.map((tool) => {
            const Icon = getToolIcon(tool.name);
            const args = parseArgs(tool.arguments);
            const target = stepTarget(tool.name, args);
            const isRunningStep = tool.status === "running";
            const isError = tool.status === "error";
            return (
              <div
                key={tool.id}
                className={cn(
                  "rounded-md border px-2 py-1.5",
                  isRunningStep
                    ? "border-primary/40 bg-primary/5"
                    : isError
                      ? "border-destructive/40 bg-destructive/5"
                      : "border-border/60 bg-muted/20",
                )}
              >
                <div className="flex items-center gap-1.5">
                  <span className="w-4 shrink-0">
                    {isRunningStep ? (
                      <Loader2 className="h-3 w-3 animate-spin text-primary" />
                    ) : isError ? (
                      <XCircle className="h-3 w-3 text-destructive" />
                    ) : (
                      <CheckCircle2 className="h-3 w-3 text-green-500" />
                    )}
                  </span>
                  <Icon className="h-3 w-3 shrink-0 opacity-70" />
                  <span className="shrink-0 text-[10px] font-semibold tracking-wide text-muted-foreground/70">
                    {toolLabel(tool.name, t)}
                  </span>
                  {target && (
                    <span className="min-w-0 flex-1 truncate font-mono text-[10.5px] text-sky-600 dark:text-sky-400">
                      {target}
                    </span>
                  )}
                  {/* Live step metadata — elapsed while running (the only
                      progress signal for tools without progress events),
                      processed bytes when the backend streams them. */}
                  {isRunningStep && (
                    <span className="flex shrink-0 items-center gap-1.5 font-mono text-[9.5px] tabular-nums text-muted-foreground/60">
                      {typeof tool.progressTotalBytes === "number" &&
                        tool.progressTotalBytes > 0 && (
                          <span className="text-primary/80">
                            {formatBytes(tool.progressTotalBytes)}
                          </span>
                        )}
                      {tool.startedAt ? formatElapsedMs(Date.now() - tool.startedAt) : ""}
                    </span>
                  )}
                </div>

                {/* Live output of the running step — ANSI-colored */}
                {isRunningStep && tool.progressDelta && (
                  <pre className="mt-1 max-h-20 overflow-auto whitespace-pre-wrap break-words rounded bg-background/60 p-1 font-mono text-[10px] leading-relaxed text-foreground/60">
                    <AnsiText text={tool.progressDelta} />
                  </pre>
                )}
                {isError && tool.result && (
                  <p className="mt-1 truncate text-[10px] text-destructive/80" title={tool.result}>
                    {tool.result.split("\n")[0]}
                  </p>
                )}
              </div>
            );
          })}
        </div>

        {/* ── Artifacts — produced files as cards ── */}
        {artifacts.length > 0 && (
          <div className="mt-3 border-t border-border/70 pt-2">
            <p className="mb-1 flex items-center gap-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
              <Wrench className="h-3 w-3" />
              {t("depworkTask.artifacts")}
            </p>
            <div className="space-y-1">
              {artifacts.map((a) => (
                <ArtifactRow key={a.path} name={a.name} path={a.path} />
              ))}
            </div>
          </div>
        )}
      </div>

      {/* ── Footer: pause / resume / stop ── */}
      {isStreaming && (
        <div className="border-t border-border/60 px-3 py-2">
          <div className="flex gap-1.5">
            <button
              onClick={() => void (isPaused ? resumeStreaming() : pauseStreaming())}
              className="flex flex-1 items-center justify-center gap-1.5 rounded-md border border-border bg-muted/30 px-2 py-1.5 text-[11px] font-medium text-foreground transition-colors hover:bg-muted/60"
            >
              {isPaused ? (
                <Play className="h-3 w-3 fill-current" />
              ) : (
                <Pause className="h-3 w-3 fill-current" />
              )}
              {isPaused ? t("depworkTask.resume") : t("depworkTask.pause")}
            </button>
            <button
              onClick={handleStop}
              className={cn(
                "flex flex-1 items-center justify-center gap-1.5 rounded-md border px-2 py-1.5 text-[11px] font-medium transition-colors",
                armedStop
                  ? "border-destructive bg-destructive text-destructive-foreground"
                  : "border-destructive/40 bg-destructive/5 text-destructive hover:bg-destructive/10",
              )}
            >
              <Square className="h-3 w-3" />
              {armedStop
                ? t("depworkTask.confirmStop", { defaultValue: "再次点击确认停止" })
                : t("depworkTask.stop")}
            </button>
          </div>
          <p className="mt-1.5 text-center text-[10px] text-muted-foreground/60">
            {isPaused ? t("depworkTask.pausedHint") : t("depworkTask.streamingHint")}
          </p>
        </div>
      )}
    </div>
    </div>
  );
}
