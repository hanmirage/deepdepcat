/**
 * JsonHighlight — VSCode-style JSON token coloring for raw JSON strings.
 *
 * A lightweight regex tokenizer (no parse tree): keys are sky, string
 * values green, numbers amber, booleans/null purple, punctuation muted.
 * Non-JSON input falls through unchanged (the tokenizer simply matches
 * nothing), so it is safe to run on arbitrary text.
 *
 * Used by the expanded tool details (ArgsBlock values, tool results,
 * permission request details) where raw JSON used to be colorless mono.
 */

import { memo, type ReactNode } from "react";
import { cn } from "@/lib/utils";

const JSON_TOKEN_RE =
  /("(?:[^"\\]|\\.)*")(\s*:)?|(-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)|(true|false|null)|([{}[\],:])/g;

function JsonHighlightImpl({ json, className }: { json: string; className?: string }) {
  const parts: ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  JSON_TOKEN_RE.lastIndex = 0;
  while ((m = JSON_TOKEN_RE.exec(json)) !== null) {
    const [, str, colon, num, kw, punct] = m;
    if (m.index > last) parts.push(json.slice(last, m.index));
    if (str !== undefined) {
      if (colon !== undefined) {
        // A string followed by `:` is a KEY — sky, like VSCode.
        parts.push(
          <span key={m.index} className="text-sky-600 dark:text-sky-400">
            {str}
          </span>,
        );
        parts.push(
          <span key={`${m.index}:`} className="text-muted-foreground/50">
            {colon}
          </span>,
        );
      } else {
        parts.push(
          <span key={m.index} className="text-green-700 dark:text-green-400">
            {str}
          </span>,
        );
      }
    } else if (num !== undefined) {
      parts.push(
        <span key={m.index} className="text-amber-600 dark:text-amber-400">
          {num}
        </span>,
      );
    } else if (kw !== undefined) {
      parts.push(
        <span key={m.index} className="text-purple-600 dark:text-purple-400">
          {kw}
        </span>,
      );
    } else if (punct !== undefined) {
      parts.push(
        <span key={m.index} className="text-muted-foreground/50">
          {punct}
        </span>,
      );
    }
    last = m.index + m[0].length;
  }
  if (last < json.length) parts.push(json.slice(last));

  return <span className={cn("font-mono", className)}>{parts}</span>;
}

export const JsonHighlight = memo(JsonHighlightImpl);

/** True when `text` parses as a JSON object or array. */
export function looksLikeJson(text: string): boolean {
  const trimmed = text.trim();
  if (!trimmed.startsWith("{") && !trimmed.startsWith("[")) return false;
  try {
    JSON.parse(trimmed);
    return true;
  } catch {
    return false;
  }
}
