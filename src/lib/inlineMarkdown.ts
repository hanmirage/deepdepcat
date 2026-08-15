/**
 * inlineMarkdown — streaming-safe inline markdown parser.
 *
 * Streaming safety rule: only COMPLETE markers produce styled tokens; the
 * unclosed tail stays plain text. This means a half-typed `**bold` renders
 * as plain text and never flashes into a `<strong>` before the closing `**`
 * arrives — the DOM stays stable across flushes.
 *
 * Scope (by design):
 * - strong `**…**`, em `*…*`, del `~~…~~`, inline code `` `…` ``, links `[t](u)`
 * - single level only (no nesting) — the final MarkdownRenderer pass at
 *   turn_end guarantees full fidelity; this renderer only needs to look right
 *   WHILE streaming.
 * - underscore `_…_` emphasis is NOT supported — `snake_case` identifiers
 *   would false-positive mid-stream.
 * - backslash escapes are not processed.
 *
 * Row-level formats (headings / lists / blockquotes) are detected by
 * `leadingFormat` on a per-line basis.
 */

export type InlineToken =
  | { type: "text"; text: string }
  | { type: "strong"; text: string }
  | { type: "em"; text: string }
  | { type: "del"; text: string }
  | { type: "code"; text: string }
  | { type: "file"; text: string; path: string }
  | { type: "link"; text: string; href: string };

/** Result of one parse — tokens plus the streaming-tail flag. */
export interface InlineParse {
  tokens: InlineToken[];
  /** True when the scan stopped at an unclosed marker. The tail sits in the
   *  final text token and must stream through the typewriter. */
  hasTail: boolean;
}

/** Row-leading format — rendered per line, streaming-safe at the row level. */
export type LeadingFormat =
  | { kind: "heading"; level: number }
  | { kind: "bullet"; marker: string; indent: number }
  | { kind: "ordered"; marker: string; indent: number }
  | { kind: "task"; checked: boolean; indent: number }
  | { kind: "quote" }
  | { kind: "hr" }
  | { kind: "table" }
  | { kind: "table-sep" }
  | { kind: "plain" };

/** A markdown table separator row (`|---|---|`). */
// Avoid a bracket-delimited character class here: Tailwind scans source
// files and would misread it as an arbitrary-value CSS class.
const TABLE_SEP_RE = /^\s*\|(?:\s|-|:|\|)+\|\s*$/;

/** Match the row-leading format of a line. Task rows come FIRST: `- [x]`
 *  also matches the bullet pattern, so the checkbox check must win. */
export function leadingFormat(line: string): LeadingFormat {
  if (/^#{1,6}\s+/.test(line)) {
    return { kind: "heading", level: line.match(/^#+/)![0].length };
  }
  const task = line.match(/^(\s*)[-*+]\s+\[([ xX])\]\s+/);
  if (task) return { kind: "task", checked: task[2] !== " ", indent: task[1].length };
  const bullet = line.match(/^(\s*)[-*+]\s+/);
  if (bullet) return { kind: "bullet", marker: "•", indent: bullet[1].length };
  const ordered = line.match(/^(\s*)\d+[.)]\s+/);
  if (ordered) return { kind: "ordered", marker: ordered[0].trim(), indent: ordered[1].length };
  if (/^(\s*)>\s?/.test(line)) return { kind: "quote" };
  if (/^[-*_]{3,}\s*$/.test(line.trim())) return { kind: "hr" };
  if (/^\s*\|/.test(line)) {
    // Separator rows stay full-width placeholders (weak text) so the row
    // rhythm never shifts mid-stream; turn_end renders the real table.
    if (TABLE_SEP_RE.test(line) && line.includes("-")) return { kind: "table-sep" };
    return { kind: "table" };
  }
  return { kind: "plain" };
}

/** Length of the leading-format prefix that should be stripped from a line. */
export function leadingFormatWidth(line: string): number {
  if (/^#{1,6}\s+/.test(line)) {
    return line.match(/^(#+\s+)/)![1].length;
  }
  const task = line.match(/^(\s*)[-*+]\s+\[([ xX])\]\s+/);
  if (task) return task[0].length;
  const bullet = line.match(/^(\s*)[-*+]\s+/);
  if (bullet) return bullet[0].length;
  const ordered = line.match(/^(\s*)\d+[.)]\s+/);
  if (ordered) return ordered[0].length;
  const quote = line.match(/^(\s*>)\s?/);
  if (quote) return quote[0].length;
  return 0;
}

/** Find the closing marker; returns the index AFTER it, or -1 when unclosed. */
function findClose(text: string, from: number, marker: string): number {
  const idx = text.indexOf(marker, from);
  return idx === -1 ? -1 : idx + marker.length;
}

/** Schemes allowed in streaming markdown link hrefs. Everything else
 *  (javascript:, data:, vbscript:, file:, …) falls back to plain text. */
const ALLOWED_LINK_SCHEMES = /^(https?|mailto):/i;

/** True when the href is safe to render: no scheme (relative / fragment /
 *  protocol-relative) or an http(s)/mailto scheme. */
function isSafeHref(href: string): boolean {
  const trimmed = href.trim();
  if (!trimmed) return false;
  if (/^[a-z][a-z0-9+.-]*:/i.test(trimmed)) {
    return ALLOWED_LINK_SCHEMES.test(trimmed);
  }
  return true;
}

/** Try to parse a link starting at `[`. Returns [end, href] or null. */
function tryLink(text: string, open: number): { end: number; href: string } | null {
  const closeBracket = text.indexOf("](", open + 1);
  if (closeBracket === -1) return null;
  const closeParen = text.indexOf(")", closeBracket + 2);
  if (closeParen === -1) return null;
  const href = text.slice(closeBracket + 2, closeParen);
  if (!isSafeHref(href)) return null;
  return { end: closeParen + 1, href };
}

/** Extensions recognized as file references (code/source/doc/media). The
 *  LAST path segment must end with one of these — optionally a `:line` —
 *  for the token to render as a clickable file reference instead of plain
 *  inline code. Kept deliberately tight so identifiers like `npm`, `v1.2`
 *  or `foo.bar` (a method call) never light up as files. */
const FILE_EXT_RE = /\.(?:tsx?|jsx?|mjs|cjs|vue|svelte|rs|go|py|java|rb|php|html?|css|scss|less|md|json|toml|ya?ml|sh|bash|cpp|c|h|hpp|cc|cs|kt|kts|swift|sql|lock|docx|pptx|xlsx|pdf|png|jpe?g|gif|svg|webp)(?::\d+)?$/i;

/** True when the inline-code content reads as a file path (not a generic
 *  code token): its last path segment carries a recognized extension, with
 *  or without a directory prefix. `src/main.ts`, `D:\docs\报告.docx`,
 *  `main.rs:42`, `package.json` all match; `npm`, `\n`, `foo.bar()` don't.
 *  Exported so the completed MarkdownRenderer reuses the SAME rule. */
export function looksLikeFilePath(s: string): boolean {
  const t = s.trim();
  if (!t || t.length > 200) return false;
  // A bare URL inside backticks must stay a code token, not a file.
  if (/^https?:\/\//i.test(t)) return false;
  const tail = t.split(/[\\/]/).pop() ?? t;
  return FILE_EXT_RE.test(tail);
}

/** Bare-URL matcher for streaming text — `https://…` (optionally `www.`)
 *  typed inline becomes a link AS IT STREAMS, matching the completed
 *  MarkdownRenderer (which autolinks via remark-gfm). Returns [end, href]
 *  or null. Trailing sentence punctuation is trimmed so `…here.` doesn't
 *  swallow the period. */
const BARE_URL_RE = /^https?:\/\/[^\s<>()"']+/i;
function matchBareUrl(text: string, from: number): { end: number; href: string } | null {
  const rest = text.slice(from);
  if (!/^https?:\/\//i.test(rest)) return null;
  const m = rest.match(BARE_URL_RE);
  if (!m) return null;
  const href = m[0].replace(/[.,;:!?]+$/, "");
  if (href.length < 9) return null; // "https://" alone is not a link yet
  // `end` uses the TRIMMED length so trailing sentence punctuation falls
  // through to the next scan as plain text, not swallowed by the link.
  return { end: from + href.length, href };
}

/**
 * Parse the CLOSED inline markers of `text`. Scanning stops at the first
 * unclosed marker — everything from there onward lands in a trailing text
 * token (`hasTail: true`), so callers can stream that tail append-only.
 * Single scan, O(n) — safe to run every store flush.
 */
export function parseInline(text: string): InlineParse {
  const tokens: InlineToken[] = [];
  let hasTail = false;
  let i = 0;
  let plainStart = 0;

  const flushPlain = (end: number) => {
    if (end > plainStart) {
      tokens.push({ type: "text", text: text.slice(plainStart, end) });
    }
  };

  while (i < text.length) {
    const rest = text.slice(i);
    const strong = rest.startsWith("**");
    const del = rest.startsWith("~~");
    const code = rest.startsWith("`");
    const em = !strong && rest.startsWith("*");
    if (strong || del || code || em) {
      const openLen = strong || del ? 2 : 1;
      const close = strong ? "**" : del ? "~~" : em ? "*" : "`";
      const closeAt = findClose(text, i + openLen, close);
      if (closeAt !== -1) {
        flushPlain(i);
        const inner = text.slice(i + openLen, closeAt - close.length);
        // Inline code that reads as a file path becomes a clickable file
        // reference (colored + opens in the workspace); otherwise it stays
        // a plain code token.
        if (code && looksLikeFilePath(inner)) {
          tokens.push({ type: "file", text: inner, path: inner });
        } else {
          tokens.push(
            strong
              ? { type: "strong", text: inner }
              : del
                ? { type: "del", text: inner }
                : code
                  ? { type: "code", text: inner }
                  : { type: "em", text: inner },
          );
        }
        i = closeAt;
        plainStart = i;
        continue;
      }
      // Unclosed — the rest is the streaming tail.
      hasTail = true;
      break;
    }
    // Bare URL (https://…) typed inline — link it AS IT STREAMS, so a raw
    // URL never shows as plain text then "pops" into a link at turn end.
    const bareUrl = matchBareUrl(text, i);
    if (bareUrl) {
      flushPlain(i);
      tokens.push({ type: "link", text: bareUrl.href, href: bareUrl.href });
      i = bareUrl.end;
      plainStart = i;
      continue;
    }
    if (rest.startsWith("[")) {
      const link = tryLink(text, i);
      if (link) {
        flushPlain(i);
        const closeBracket = text.indexOf("](", i);
        tokens.push({
          type: "link",
          text: text.slice(i + 1, closeBracket),
          href: link.href,
        });
        i = link.end;
        plainStart = i;
        continue;
      }
      // `[` without a completed `](…)` — might still become a link.
      hasTail = true;
      break;
    }
    i += 1;
  }

  flushPlain(text.length);
  return { tokens, hasTail };
}
