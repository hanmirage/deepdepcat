/**
 * ToolCallCard parts — collapsed/expanded sub-views extracted from
 * ToolCallCard to keep the card file within the size budget.
 */

import { useState, useMemo } from "react";
import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Check, Copy, Activity, Terminal, Search } from "lucide-react";
import { JsonHighlight, looksLikeJson } from "@/components/chat/JsonHighlight";
import { FileDiffPreview } from "@/components/chat/FileDiffPreview";
import { AnsiText } from "@/components/chat/AnsiText";
import type { DiffStats } from "@/lib/diffStats";
import { formatBytes } from "@/config/toolNarrative";
import i18n from "@/i18n";
import { cn } from "@/lib/utils";
import type { ToolCallState } from "@/types";

// ── Helpers ────────────────────────────────────────────────

export function parseArgs(args: string): Record<string, unknown> {
  try {
    return args ? JSON.parse(args) : {};
  } catch {
    return {};
  }
}

/** Max live-stream characters shown in the progress panel — the stream
 *  renders the TAIL (progress lines, errors, final output); a long bash
 *  run's head is noise. */
const MAX_LIVE_TAIL_CHARS = 800;

export function liveTail(raw: string): string {
  if (raw.length <= MAX_LIVE_TAIL_CHARS) return raw;
  const omitted = raw.length - MAX_LIVE_TAIL_CHARS;
  return `${i18n.t("chat.omittedLiveTail", { count: omitted })}${raw.slice(-MAX_LIVE_TAIL_CHARS)}`;
}

export function DiffBadge({ stats }: { stats: DiffStats }) {
  if (stats.added === 0 && stats.removed === 0) return null;
  return (
    <span className="shrink-0 font-mono text-[10px]">
      {stats.added > 0 && (
        <span className="text-green-600 dark:text-green-400">+{stats.added}</span>
      )}
      {stats.removed > 0 && (
        <span className="text-red-500 dark:text-red-400">-{stats.removed}</span>
      )}
    </span>
  );
}

export function EditFileDiff({ tool }: { tool: ToolCallState }) {
  const args = useMemo(() => parseArgs(tool.arguments), [tool.arguments]);
  if (
    !args?.path ||
    typeof args.old_text !== "string" ||
    typeof args.new_text !== "string"
  ) {
    return null;
  }
  return (
    <FileDiffPreview
      filePath={args.path as string}
      oldText={args.old_text}
      newText={args.new_text}
    />
  );
}

export function ResultBlock({ tool }: { tool: ToolCallState }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [expanded, setExpanded] = useState(false);

  // JSON results (tool outputs are often JSON blobs) get VSCode-style
  // coloring; everything else stays plain mono text.
  const result = tool.result ?? "";
  const isJson = useMemo(() => looksLikeJson(result), [result]);
  const long = result.length > 800;
  const truncated = long ? `${result.slice(0, 800)}\n…` : result;
  const shown = expanded ? result : truncated;

  // Hooks must run before the early returns — a tool without a result would
  // otherwise change the hook count between renders.
  if (!tool.result) return null;
  // Tool-family cards already render the result (diff / terminal / search).
  if (
    tool.name === "edit_file" ||
    tool.name === "search_replace" ||
    tool.name === "bash" ||
    tool.name === "run_command" ||
    tool.name === "grep" ||
    tool.name === "glob"
  )
    return null;

  const copy = async () => {
    await navigator.clipboard.writeText(tool.result ?? "");
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="rounded-md border border-border bg-muted/20 p-1.5">
      <div className="mb-0.5 flex items-center justify-between">
        <p className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
          {t("chat.result", { defaultValue: "结果" })}
        </p>
        <span className="flex items-center gap-1">
          {long && (
            <button
              onClick={() => setExpanded((v) => !v)}
              className="rounded px-1 py-0.5 text-[9px] text-muted-foreground/50 transition-colors hover:bg-muted hover:text-foreground"
            >
              {expanded ? t("common.collapse") : t("common.expand")}
            </button>
          )}
          {tool.status === "done" && (
            <button
              onClick={copy}
              className={cn(
                "flex items-center gap-1 rounded px-1 py-0.5 text-[9px] transition-colors",
                copied
                  ? "text-green-600"
                  : "text-muted-foreground/50 hover:bg-muted hover:text-foreground",
              )}
              title={copied ? t("chat.copied") : t("chat.copyResult", { defaultValue: "复制结果" })}
            >
              {copied ? <Check className="h-2.5 w-2.5" /> : <Copy className="h-2.5 w-2.5" />}
              {copied ? t("chat.copied") : t("chat.copy")}
            </button>
          )}
        </span>
      </div>
      <pre
        className={cn(
          "min-w-0 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px]",
          // Expanded = a DEFINITE bounded container (fixed height + inner
          // scroll); collapsed stays a 2.5-line peek.
          expanded ? "h-64" : "max-h-28",
          tool.status === "error" ? "text-destructive/80" : "text-muted-foreground",
        )}
      >
        {isJson && tool.status !== "error" ? (
          <JsonHighlight json={shown} />
        ) : (
          shown
        )}
      </pre>
    </div>
  );
}

/**
 * LiveProgress — real-time tool output streamed via `tool_call_progress`.
 *
 * Rendered OUTSIDE the collapsible details (the row stays compact while
 * the tool runs). The delta area defaults to a 2-line tail preview — a
 * long stream (or an error text arriving mid-stream) must not stretch the
 * collapsed card; the full tail is one click away.
 */
export function LiveProgress({ tool }: { tool: ToolCallState }) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const hasProgress = !!(tool.progressDelta || tool.progressTotalBytes);
  if (!hasProgress || tool.status === "done") return null;

  const raw = tool.progressDelta ?? "";
  const tail = liveTail(raw);
  const lines = tail.split("\n");
  // Collapsed: last 2 lines only. The streamed content (including any
  // error text that arrives mid-stream) is one click away in the expanded
  // view — it does not belong on the collapsed card.
  const collapsedLines = lines.slice(-2);

  return (
    <div className="space-y-1 rounded-md border border-border/70 bg-muted/20 p-1.5">
      <p className="flex items-center gap-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
        <Activity className="h-3 w-3 animate-pulse text-primary" />
        {t("chat.liveProgress", { defaultValue: "实时进度" })}
        {raw.length > 0 && (
          <button
            onClick={() => setExpanded((v) => !v)}
            className="ml-auto rounded px-1 py-0.5 text-[9px] text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground"
          >
            {expanded ? t("chat.liveProgressCollapse", { defaultValue: "收起" }) : t("chat.liveProgressExpand", { defaultValue: "展开" })}
          </button>
        )}
      </p>

      {typeof tool.progressTotalBytes === "number" && tool.progressTotalBytes > 0 && (
        <p className="text-[10px] font-mono text-muted-foreground/70">
          {formatBytes(tool.progressTotalBytes)}
        </p>
      )}

      {tool.progressDelta && (
        <pre
          className={cn(
            "min-w-0 overflow-auto whitespace-pre-wrap break-all font-mono text-[10.5px] leading-relaxed",
            "text-foreground/70",
            expanded ? "max-h-28" : "max-h-10",
          )}
        >
          <AnsiText text={expanded ? tail : collapsedLines.join("\n")} />
        </pre>
      )}
    </div>
  );
}

export function ArgsBlock({ tool }: { tool: ToolCallState }) {
  const { t } = useTranslation();
  const args = useMemo(() => parseArgs(tool.arguments), [tool.arguments]);
  const entries = Object.entries(args);
  if (!tool.arguments) return null;
  // Arguments may still be streaming in (JSON not closed yet) — show the
  // raw accumulated fragment instead of an empty panel, so a running tool
  // is observable at a glance.
  if (entries.length === 0 && tool.arguments.trim() !== "{}") {
    return (
      <div className="rounded-md border border-border bg-muted/20 p-1.5">
        <p className="mb-1 flex items-center gap-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
          <Terminal className="h-3 w-3" />
          {t("chat.arguments", { defaultValue: "参数" })}
        </p>
        <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px] text-muted-foreground/80">
          {tool.arguments.slice(0, 400)}
          {tool.arguments.length > 400 ? "…" : ""}
        </pre>
      </div>
    );
  }
  return (
    <div className="rounded-md border border-border bg-muted/20 p-1.5">
      <p className="mb-1 flex items-center gap-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
        <Terminal className="h-3 w-3" />
        {t("chat.arguments", { defaultValue: "参数" })}
      </p>
      {/* Key → value rows instead of a raw JSON blob — readable at a glance,
          long values truncate with a hover tooltip for the full text. */}
      <dl className="max-h-64 space-y-0.5 overflow-auto pr-1">
        {entries.map(([key, value]) => {
          const text = typeof value === "string" ? value : JSON.stringify(value);
          return (
            <div key={key} className="flex items-baseline gap-2 text-[11px]">
              <dt className="shrink-0 font-mono text-muted-foreground/60">{key}</dt>
              {/* Values wrap INSIDE the container (break-all) — a long
                  command/path must stay visible, not vanish behind a
                  single-line ellipsis. */}
              <dd
                className="min-w-0 flex-1 whitespace-pre-wrap break-all font-mono"
                title={text}
              >
                {typeof value === "string" ? (
                  // VSCode-style JSON value colors — paths/queries included.
                  <span className="text-green-700 dark:text-green-400">{value}</span>
                ) : typeof value === "number" ? (
                  <span className="text-amber-600 dark:text-amber-400">{value}</span>
                ) : typeof value === "boolean" ? (
                  <span className="text-purple-600 dark:text-purple-400">{String(value)}</span>
                ) : (
                  <JsonHighlight json={text} />
                )}
              </dd>
            </div>
          );
        })}
      </dl>
    </div>
  );
}

/** Argument highlight style per tool family. */

// ── Tool-family cards ───────────────────────────────────────

/** Parse the trailing "Exit code: N" appended by the bash tool (bash.rs
 *  adds it after the output; non-zero means the command RAN but failed —
 *  a distinct signal from `tool.status`, which stays "done"). */
export function parseExitCode(result: string | undefined): number | null {
  const m = result?.match(/Exit code: (-?\d+)\s*$/);
  return m ? Number.parseInt(m[1], 10) : null;
}

/** BashCard — the tool as a terminal: command + ANSI output + exit-code
 *  pill, instead of generic args/result panels. A non-zero exit renders an
 *  amber "failed" pill (the row's status dot still reads "done"). */
export function BashCard({ tool }: { tool: ToolCallState }) {
  const { t } = useTranslation();
  const args = useMemo(() => parseArgs(tool.arguments), [tool.arguments]);
  const command = typeof args.command === "string" ? args.command : null;
  const exitCode = useMemo(() => parseExitCode(tool.result), [tool.result]);
  const failed = exitCode !== null && exitCode !== 0;
  const output = (tool.result ?? "")
    .replace(/\n\nExit code: -?\d+\s*$/, "")
    .trim();

  return (
    <div className="rounded-md border border-border bg-muted/20 p-1.5">
      <div className="mb-1 flex items-center justify-between gap-2">
        <p className="flex min-w-0 items-center gap-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
          <Terminal className="h-3 w-3 shrink-0" />
          {t("chat.bashCard", { defaultValue: "终端" })}
        </p>
        {exitCode !== null && (
          <span
            className={cn(
              "shrink-0 rounded px-1.5 py-0.5 font-mono text-[10px] tabular-nums",
              failed
                ? "bg-amber-500/10 text-amber-600 dark:text-amber-400"
                : "bg-muted text-muted-foreground",
            )}
          >
            {failed
              ? t("chat.exitFailed", { defaultValue: "退出 {{code}}", code: exitCode })
              : t("chat.exitOk", { defaultValue: "退出码 0" })}
          </span>
        )}
      </div>
      {command && (
        <pre className="mb-1 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px] text-foreground/90">
          {command}
        </pre>
      )}
      {output && (
        <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px] leading-relaxed text-foreground/70">
          <AnsiText text={output} />
        </pre>
      )}
    </div>
  );
}

/** One file's grep/glob matches — parsed from the "{file}:{line}: {text}"
 *  rows the grep tool emits (grep.rs), grouped per file for readability. */
export interface GrepGroup {
  file: string;
  lines: { line: number; text: string }[];
}

/** Parse grep output into per-file groups + the trailing summary line.
 *  Returns empty groups when the result is not grep-shaped (no matches). */
export function parseGrepResult(
  result: string,
): { groups: GrepGroup[]; summary: string | null } {
  const byFile = new Map<string, GrepGroup>();
  let summary: string | null = null;
  const rowRe = /^(.+?):(\d+): (.+)$/;
  for (const raw of result.split("\n")) {
    const line = raw.trim();
    if (!line) continue;
    const sum = line.match(/^Found (\d+) matches? in (\d+) files?/);
    if (sum) {
      summary = line;
      continue;
    }
    const m = line.match(rowRe);
    if (!m) continue;
    let group = byFile.get(m[1]);
    if (!group) {
      group = { file: m[1], lines: [] };
      byFile.set(m[1], group);
    }
    group.lines.push({ line: Number.parseInt(m[2], 10), text: m[3] });
  }
  return { groups: [...byFile.values()], summary };
}

/** SearchCard — grep/glob matches grouped per file with line numbers,
 *  replacing the generic args panel (which for grep is just a "pattern" key). */
export function SearchCard({ tool }: { tool: ToolCallState }) {
  const { t } = useTranslation();
  const parsed = useMemo(() => parseGrepResult(tool.result ?? ""), [tool.result]);
  const { groups, summary } = parsed;
  const body =
    groups.length > 0 ? (
      <div className="space-y-1">
        {groups.map((g) => (
          <div key={g.file}>
            <p className="truncate font-mono text-[10px] text-sky-600 dark:text-sky-400">
              {g.file}
            </p>
            <div className="space-y-0.5">
              {g.lines.map((l, i) => (
                <div key={i} className="flex gap-2 font-mono text-[11px]">
                  <span className="shrink-0 text-muted-foreground/50">{l.line}</span>
                  <span className="min-w-0 flex-1 truncate" title={l.text}>
                    {l.text}
                  </span>
                </div>
              ))}
            </div>
          </div>
        ))}
      </div>
    ) : tool.result?.trim() ? (
      // Not grep-shaped (no matches / plain error text) — show the raw result.
      <pre className="max-h-40 overflow-auto whitespace-pre-wrap break-all font-mono text-[11px] text-muted-foreground/80">
        {tool.result}
      </pre>
    ) : null;
  if (!body) return null;

  return (
    <div className="rounded-md border border-border bg-muted/20 p-1.5">
      <p className="mb-1 flex items-center gap-1 text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
        <Search className="h-3 w-3" />
        {t("chat.searchCard", { defaultValue: "匹配" })}
      </p>
      {body}
      {summary && <p className="mt-1 text-[10px] text-muted-foreground/60">{summary}</p>}
    </div>
  );
}

/** Tool-card dispatch — the expanded body for a tool call. New tool families
 *  register here instead of stacking if/else in ToolCallCard. Returns null for
 *  unhandled tools so the caller falls back to the generic args panel. */
export function toolCardFor(tool: ToolCallState): ReactNode | null {
  switch (tool.name) {
    case "edit_file":
    case "search_replace":
      return <EditFileDiff tool={tool} />;
    case "bash":
    case "run_command":
      return <BashCard tool={tool} />;
    case "grep":
    case "glob":
      return <SearchCard tool={tool} />;
    default:
      return null;
  }
}
