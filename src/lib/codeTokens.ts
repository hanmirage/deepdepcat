/**
 * codeTokens — shared types + helpers for worker-based streaming highlighting.
 *
 * The Shiki worker highlights the growing code prefix on EVERY flush and
 * returns the full token list. The React tree renders tokens with stable
 * index keys, so unchanged prefix tokens reuse their DOM spans natively
 * (React reconciliation) — the visible effect is "only the tail grows",
 * while the worker keeps ALL parsing off the main thread.
 */

/** One highlighted token — text plus a color (undefined = default color). */
export interface HighlightToken {
  text: string;
  /** Inline color from Shiki; undefined renders with the block default. */
  color?: string;
}

export interface HighlightPayload {
  /** Full token list for the highlighted text. */
  tokens: HighlightToken[];
  /** The text the tokens were computed from. */
  text: string;
  language: string;
  /** True when the highlight failed and the caller should fall back. */
  error?: boolean;
}

/** Flatten Shiki's line-based token arrays into one token list. */
export function flattenShikiTokens(
  lines: { content: string; color?: string }[][],
): HighlightToken[] {
  const out: HighlightToken[] = [];
  for (const line of lines) {
    for (const tok of line) {
      out.push({ text: tok.content, color: tok.color });
    }
    if (line.length === 0) {
      out.push({ text: "\n" });
    } else {
      // Shiki strips the newline per line — re-add it between lines.
      const last = out[out.length - 1];
      if (last) last.text += "\n";
    }
  }
  return out;
}
