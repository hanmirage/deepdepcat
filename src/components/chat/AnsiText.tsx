/**
 * AnsiText — renders a string containing ANSI SGR escape sequences with
 * terminal colors (bash/tool streaming output). Unsupported sequences
 * (256-color, truecolor, background) are stripped; anything without ANSI
 * codes passes through as a single plain span.
 */

import { memo, type ReactNode } from "react";
import { cn } from "@/lib/utils";

// eslint-disable-next-line no-control-regex -- ANSI escape sequences are intentional
const ANSI_SGR_RE = /\x1b\[([0-9;]*)m/g;

/** Foreground color mapping (30-37 dim, 90-97 bright). */
const FG_COLORS: Record<number, string> = {
  30: "text-foreground/60",
  31: "text-red-500 dark:text-red-400",
  32: "text-emerald-600 dark:text-emerald-400",
  33: "text-amber-600 dark:text-amber-400",
  34: "text-sky-600 dark:text-sky-400",
  35: "text-purple-600 dark:text-purple-400",
  36: "text-cyan-600 dark:text-cyan-400",
  37: "text-foreground",
  90: "text-foreground/70",
  91: "text-red-400",
  92: "text-emerald-400",
  93: "text-amber-400",
  94: "text-sky-400",
  95: "text-purple-400",
  96: "text-cyan-400",
  97: "text-foreground",
};

function AnsiTextImpl({ text, className }: { text: string; className?: string }) {
  if (!text.includes("\x1b")) {
    return <span className={className}>{text}</span>;
  }

  const parts: ReactNode[] = [];
  let cursor = 0;
  let color: string | null = null;
  let bold = false;
  let key = 0;
  let m: RegExpExecArray | null;
  ANSI_SGR_RE.lastIndex = 0;

  const flush = (to: number) => {
    if (to <= cursor) return;
    const chunk = text.slice(cursor, to);
    cursor = to;
    if (chunk.length === 0) return;
    parts.push(
      <span key={key++} className={cn(color, bold && "font-semibold")}>
        {chunk}
      </span>,
    );
  };

  while ((m = ANSI_SGR_RE.exec(text)) !== null) {
    flush(m.index);
    cursor = m.index + m[0].length;
    const codes = m[1] ? m[1].split(";").map((c) => Number(c)) : [];
    if (codes.length === 0 || codes[0] === 0) {
      // Reset — all attributes off.
      color = null;
      bold = false;
    } else {
      for (let i = 0; i < codes.length; i++) {
        const n = codes[i];
        if (n === 1) {
          bold = true;
        } else if (n === 38 || n === 48) {
          // Extended color: 38;5;N (indexed) or 38;2;R;G;B (truecolor).
          // Unsupported here — skip the parameter run so later codes
          // (e.g. a trailing reset) still parse.
          const marker = codes[i + 1];
          if (marker === 5) i += 2;
          else if (marker === 2) i += 4;
          else i += 1;
        } else if (n >= 30 && n <= 37) {
          color = FG_COLORS[n];
        } else if (n >= 90 && n <= 97) {
          color = FG_COLORS[n];
        }
        // Background (40-49), underline (4), blink, etc. — ignored.
      }
    }
  }
  flush(text.length);

  return <span className={className}>{parts}</span>;
}

export const AnsiText = memo(AnsiTextImpl);

/** Strip ANSI escapes entirely (plain text fallback). */
export function stripAnsi(text: string): string {
  // eslint-disable-next-line no-control-regex -- ANSI escape sequences are intentional
  return text.replace(/\x1b\[[0-9;]*m/g, "");
}
