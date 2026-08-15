/**
 * FileDiffPreview — GitHub-style file diff view for edit_file tool.
 *
 * Shows unified diff with:
 * - File path header
 * - Line numbers (old / new)
 * - Syntax highlighting for added/removed lines
 * - Collapsible sections
 * - Context-hunk windowing (a one-line edit in a huge file renders only
 *   the lines around the change, not the whole file)
 */

import { useState, useMemo } from "react";
import { ChevronDown, ChevronRight, FileCode } from "lucide-react";
import i18n from "@/i18n";
import { cn } from "@/lib/utils";

interface DiffLine {
  type: "context" | "add" | "remove";
  oldLine?: number;
  newLine?: number;
  content: string;
}

interface FileDiffPreviewProps {
  filePath: string;
  oldText: string;
  newText: string;
  className?: string;
}

/** Context lines kept around each change (GitHub-style hunk window). */
const CONTEXT_WINDOW = 8;

/** Max characters rendered per diff line — a pathological single-line blob
 *  (minified code, huge JSON, a failed edit's whole-file payload) would
 *  otherwise stretch the table to infinite width. The full line is still
 *  reachable via the title tooltip. */
const MAX_LINE_CHARS = 400;

/** Render a diff line's content with a hard length cap. */
function renderLine(content: string): string {
  if (content.length <= MAX_LINE_CHARS) return content;
  return `${content.slice(0, MAX_LINE_CHARS)}${i18n.t("chat.diffTruncated", { count: content.length })}`;
}

/**
 * Compress long runs of unchanged (context) lines into a windowed hunk:
 * [first N lines] ⋯ [omitted count] [last N lines]. Without this, a
 * one-line edit in a 2000-line file renders all 2000 context rows — the
 * table grows unbounded and blows out of its container.
 */
function compressContext(lines: DiffLine[]): DiffLine[] {
  const out: DiffLine[] = [];
  let run: DiffLine[] = [];
  const flushRun = () => {
    if (run.length <= CONTEXT_WINDOW * 2 + 1) {
      out.push(...run);
    } else {
      out.push(...run.slice(0, CONTEXT_WINDOW));
      const omitted = run.length - CONTEXT_WINDOW * 2;
      out.push({ type: "context", content: i18n.t("chat.diffLinesOmitted", { count: omitted }) });
      out.push(...run.slice(run.length - CONTEXT_WINDOW));
    }
    run = [];
  };
  for (const line of lines) {
    if (line.type === "context") {
      run.push(line);
    } else {
      flushRun();
      out.push(line);
    }
  }
  flushRun();
  return out;
}

/**
 * Compute unified diff between old and new text.
 * Simple line-by-line comparison (not Myers diff, but good enough for small edits).
 */
function computeDiff(oldText: string, newText: string): DiffLine[] {
  const oldLines = oldText.split("\n");
  const newLines = newText.split("\n");
  const result: DiffLine[] = [];
  
  let oldIdx = 0;
  let newIdx = 0;
  
  // Find common prefix (context)
  while (
    oldIdx < oldLines.length &&
    newIdx < newLines.length &&
    oldLines[oldIdx] === newLines[newIdx]
  ) {
    result.push({
      type: "context",
      oldLine: oldIdx + 1,
      newLine: newIdx + 1,
      content: oldLines[oldIdx],
    });
    oldIdx++;
    newIdx++;
  }
  
  // Find removed lines
  const oldRemaining = oldLines.slice(oldIdx);
  const newRemaining = newLines.slice(newIdx);
  
  // Check if it's a pure addition (no removal)
  if (oldRemaining.length === 0 && newRemaining.length > 0) {
    newRemaining.forEach((line, i) => {
      result.push({
        type: "add",
        newLine: newIdx + i + 1,
        content: line,
      });
    });
    return compressContext(result);
  }
  
  // Check if it's a pure removal (no addition)
  if (newRemaining.length === 0 && oldRemaining.length > 0) {
    oldRemaining.forEach((line, i) => {
      result.push({
        type: "remove",
        oldLine: oldIdx + i + 1,
        content: line,
      });
    });
    return compressContext(result);
  }
  
  // Mixed changes - show all removed then all added
  oldRemaining.forEach((line, i) => {
    result.push({
      type: "remove",
      oldLine: oldIdx + i + 1,
      content: line,
    });
  });
  
  newRemaining.forEach((line, i) => {
    result.push({
      type: "add",
      newLine: newIdx + i + 1,
      content: line,
    });
  });
  
  return compressContext(result);
}

/**
 * Extract file extension for language detection.
 */
function getFileExtension(path: string): string {
  const match = path.match(/\.([^.]+)$/);
  return match?.[1]?.toLowerCase() ?? "";
}

/**
 * Map file extension to display name.
 */
function getLanguageName(ext: string): string {
  const map: Record<string, string> = {
    ts: "TypeScript",
    tsx: "TypeScript React",
    js: "JavaScript",
    jsx: "JavaScript React",
    py: "Python",
    rs: "Rust",
    go: "Go",
    java: "Java",
    kt: "Kotlin",
    swift: "Swift",
    c: "C",
    cpp: "C++",
    h: "C Header",
    hpp: "C++ Header",
    cs: "C#",
    rb: "Ruby",
    php: "PHP",
    html: "HTML",
    css: "CSS",
    scss: "SCSS",
    less: "Less",
    json: "JSON",
    yaml: "YAML",
    yml: "YAML",
    toml: "TOML",
    md: "Markdown",
    sql: "SQL",
    sh: "Shell",
    bash: "Bash",
    zsh: "Zsh",
    dockerfile: "Dockerfile",
    vue: "Vue",
    svelte: "Svelte",
    astro: "Astro",
  };
  return map[ext] ?? ext.toUpperCase() ?? "Text";
}

export function FileDiffPreview({
  filePath,
  oldText,
  newText,
  className,
}: FileDiffPreviewProps) {
  const [collapsed, setCollapsed] = useState(false);
  const diff = useMemo(() => computeDiff(oldText, newText), [oldText, newText]);
  
  const addedCount = diff.filter((l) => l.type === "add").length;
  const removedCount = diff.filter((l) => l.type === "remove").length;
  const ext = getFileExtension(filePath);
  const langName = getLanguageName(ext);

  // Shorten path for display
  const shortPath = filePath.split("/").slice(-2).join("/");

  if (diff.length === 0) {
    return null;
  }

  return (
    <div
      className={cn(
        "overflow-hidden rounded-lg border border-border bg-card",
        className
      )}
    >
      {/* Header */}
      <button
        onClick={() => setCollapsed(!collapsed)}
        className="flex w-full items-center justify-between gap-2 border-b border-border bg-muted/40 px-3 py-2 text-left hover:bg-muted/60"
      >
        <div className="flex items-center gap-2">
          <FileCode className="h-4 w-4 shrink-0 text-muted-foreground" />
          <span className="min-w-0 truncate text-xs font-medium text-foreground" title={shortPath}>
            {shortPath}
          </span>
          <span className="text-[10px] text-muted-foreground">({langName})</span>
          <span className="flex items-center gap-1 text-[10px] font-mono">
            {addedCount > 0 && (
              <span className="text-green-600 dark:text-green-400">+{addedCount}</span>
            )}
            {removedCount > 0 && (
              <span className="text-red-500 dark:text-red-400">-{removedCount}</span>
            )}
          </span>
        </div>
        {collapsed ? (
          <ChevronRight className="h-3.5 w-3.5 text-muted-foreground" />
        ) : (
          <ChevronDown className="h-3.5 w-3.5 text-muted-foreground" />
        )}
      </button>

      {/* Diff content — own height cap + scroll so a windowed hunk can
          never grow past the viewport (belt over the hunk-window braces:
          a pathological diff still scrolls instead of overflowing). */}
      {!collapsed && (
        <div className="max-h-[45vh] overflow-auto">
          <table className="w-full text-[11px] font-mono">
            <tbody>
              {diff.map((line, idx) => (
                <tr
                  key={idx}
                  className={cn(
                    line.type === "add" && "bg-green-500/10 dark:bg-green-500/5",
                    line.type === "remove" && "bg-red-500/10 dark:bg-red-500/5"
                  )}
                >
                  {/* Line numbers — the hunk-omitted row shows an ellipsis
                      marker instead of line numbers */}
                  <td
                    className={cn(
                      "w-12 select-none border-r border-border/50 py-0.5 pr-2 text-right text-muted-foreground/50",
                      line.type === "add" && "text-green-600/70 dark:text-green-400/70",
                      line.type === "remove" && "text-red-500/70 dark:text-red-400/70"
                    )}
                  >
                    {line.oldLine ?? ""}
                  </td>
                  <td
                    className={cn(
                      "w-12 select-none border-r border-border/50 py-0.5 pr-2 text-right text-muted-foreground/50",
                      line.type === "add" && "text-green-600/70 dark:text-green-400/70",
                      line.type === "remove" && "text-red-500/70 dark:text-red-400/70"
                    )}
                  >
                    {line.newLine ?? ""}
                  </td>

                  {/* Content */}
                  <td className="w-full">
                    <div className="flex">
                      {/* Marker */}
                      <span
                        className={cn(
                          "w-4 select-none text-center",
                          line.type === "add" && "text-green-600 dark:text-green-400",
                          line.type === "remove" && "text-red-500 dark:text-red-400",
                          line.type === "context" && "text-muted-foreground/30"
                        )}
                      >
                        {line.type === "add" && "+"}
                        {line.type === "remove" && "-"}
                        {line.type === "context" && " "}
                      </span>
                      {/* Text content — capped per line (see renderLine) so
                          the table can never exceed the container width */}
                      <span
                        title={line.content}
                        className={cn(
                          "whitespace-pre",
                          line.type === "add" && "text-green-900 dark:text-green-100",
                          line.type === "remove" && "text-red-900 dark:text-red-100",
                          line.type === "context" && "text-foreground/80"
                        )}
                      >
                        {line.content.length === 0 ? " " : renderLine(line.content)}
                      </span>
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
