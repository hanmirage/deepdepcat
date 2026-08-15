/**
 * InlineMarkdown — streaming-safe row-level markdown renderer.
 *
 * Renders ONE line of streaming content:
 * - row-leading formats (heading / bullet / ordered / quote) get their chrome
 * - inline tokens (strong/em/del/code/link) render only when their markers
 *   are CLOSED — the unclosed tail stays plain text (see lib/inlineMarkdown)
 *
 * The static `InlineMarkdown` is used for completed lines (memoized by the
 * parent — unchanged lines never re-render). The active line is assembled by
 * StreamingMarkdown's ActiveLine from the same exported pieces + StreamingText
 * for the growing tail.
 */

import type { ReactNode } from "react";
import { Check } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  parseInline,
  leadingFormat,
  leadingFormatWidth,
  type InlineToken,
  type LeadingFormat,
} from "@/lib/inlineMarkdown";
import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";

/** Heading size per level — mirrors the prose h1–h6 hierarchy loosely. */
const HEADING_CLASS = [
  "text-2xl font-bold",
  "text-xl font-bold",
  "text-lg font-semibold",
  "text-base font-semibold",
  "text-sm font-semibold",
  "text-sm font-semibold text-muted-foreground/80",
];

/** Row shell — the per-format chrome around the inline content. */
export function LineShell({
  format,
  children,
}: {
  format: LeadingFormat;
  children: ReactNode;
}) {
  switch (format.kind) {
    case "heading":
      return (
        <div className={`leading-snug text-sm ${HEADING_CLASS[format.level - 1] ?? HEADING_CLASS[5]}`}>
          {children}
        </div>
      );
    case "bullet":
      return (
        <div className="flex items-baseline gap-2 text-sm leading-relaxed" style={{ paddingLeft: format.indent * 14 }}>
          <span className="shrink-0 select-none text-muted-foreground/70">
            {format.marker}
          </span>
          <span className="min-w-0 flex-1">{children}</span>
        </div>
      );
    case "ordered":
      return (
        <div className="flex items-baseline gap-2 text-sm leading-relaxed" style={{ paddingLeft: format.indent * 14 }}>
          <span className="shrink-0 select-none text-muted-foreground/70">
            {format.marker}
          </span>
          <span className="min-w-0 flex-1">{children}</span>
        </div>
      );
    case "task":
      // Streaming-safe task row — a real checkbox (non-interactive while
      // streaming; the turn_end pass may render a proper <ul>). The box
      // keeps its place while the text types out after it.
      return (
        <div className="flex items-start gap-2 text-sm leading-relaxed" style={{ paddingLeft: format.indent * 14 }}>
          <span
            className={cn(
              "mt-[3px] flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-[3px] border transition-colors",
              format.checked
                ? "border-primary/60 bg-primary/15"
                : "border-muted-foreground/40",
            )}
            aria-hidden="true"
          >
            {format.checked && <Check className="h-2.5 w-2.5 text-primary" strokeWidth={3} />}
          </span>
          <span className="min-w-0 flex-1">{children}</span>
        </div>
      );
    case "quote":
      return (
        <div className="border-l-2 border-border/70 pl-2 text-sm leading-relaxed text-foreground/85">
          {children}
        </div>
      );
    case "hr":
      // `---` streams in as a hairline the moment it's complete (the dash
      // run may still be typing — the divider doesn't need the content).
      return <div className="my-1 h-px bg-border/70" />;
    case "table":
      // Streaming-safe table approximation: mono rows with the `|` cells
      // kept verbatim (prefix-stable, no pops). turn_end replaces the whole
      // block with a real rendered table.
      return (
        <div className="my-0.5 px-1 font-mono text-[11px] leading-relaxed text-foreground/85">
          <span className="whitespace-pre-wrap break-words">{children}</span>
        </div>
      );
    case "table-sep":
      // Separator row keeps its line height as a weak placeholder — the
      // row rhythm must not shift while the table is still streaming.
      return (
        <div className="my-0.5 px-1 font-mono text-[10px] leading-relaxed text-muted-foreground/30">
          <span className="whitespace-pre-wrap break-words">{children}</span>
        </div>
      );
    default:
      // min-h keeps the line's height while it's still empty (mid-typing).
      return <div className="min-h-[1.375em] text-sm leading-relaxed">{children}</div>;
  }
}

/** Render a parsed inline token list. */
export function InlineTokens({ tokens }: { tokens: InlineToken[] }) {
  return (
    <>
      {tokens.map((tok, i) => {
        switch (tok.type) {
          case "text":
            return <span key={i}>{tok.text}</span>;
          case "strong":
            return <strong key={i} className="font-semibold">{tok.text}</strong>;
          case "em":
            return <em key={i} className="italic">{tok.text}</em>;
          case "del":
            return (
              <del key={i} className="text-muted-foreground/70 line-through">
                {tok.text}
              </del>
            );
          case "code":
            return (
              <code
                key={i}
                className="rounded bg-muted px-1 py-0.5 font-mono text-[0.9em]"
              >
                {tok.text}
              </code>
            );
          case "file":
            return (
              <button
                key={i}
                type="button"
                title={tok.path}
                onClick={(e) => {
                  e.stopPropagation();
                  const mode = useAppStore.getState().mode;
                  useRightPanelStore.getState().revealFile(mode, tok.path);
                }}
                className="code-tok-file cursor-pointer rounded bg-muted/60 px-1 py-0.5 font-mono text-[0.9em] align-baseline transition-colors hover:underline hover:underline-offset-2"
              >
                {tok.text}
              </button>
            );
          case "link":
            return (
              <a
                key={i}
                href={tok.href}
                target="_blank"
                rel="noopener noreferrer"
                className="text-primary underline underline-offset-2"
              >
                {tok.text}
              </a>
            );
        }
      })}
    </>
  );
}

/** Static row renderer — one COMPLETED line, full inline formatting. */
export function InlineMarkdown({ text }: { text: string }) {
  const format = leadingFormat(text);
  const width = leadingFormatWidth(text);
  const { tokens } = parseInline(text.slice(width));
  return (
    <LineShell format={format}>
      <InlineTokens tokens={tokens} />
    </LineShell>
  );
}
