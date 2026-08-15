/**
 * StreamingMarkdown parts — the per-block streaming renderers (completed
 * lines, active line, streaming code blocks, copy button, segment router).
 */

import { memo, useEffect, useId, useMemo, useState } from "react";
import { Check, Copy } from "lucide-react";
import { useTranslation } from "react-i18next";
import { InlineMarkdown, InlineTokens, LineShell } from "@/components/chat/InlineMarkdown";
import { MarkdownRenderer } from "@/components/chat/MarkdownRenderer";
import { StreamingCursor } from "@/components/chat/StreamingText";
import { getLanguageIcon, getLanguageDisplayName, getLineCount } from "@/components/chat/CodeBlock";
import { parseInline, leadingFormat, leadingFormatWidth } from "@/lib/inlineMarkdown";
import { healInline } from "@/lib/markdownHeal";
import { highlightTokens } from "@/lib/codeHighlight";
import {
  highlightStreaming,
  disposeHighlight,
  highlightWorkerAvailable,
} from "@/lib/highlightClient";
import { cn } from "@/lib/utils";
import type { HighlightToken } from "@/lib/codeTokens";
import type { StreamSegment, RenderToken } from "./StreamingMarkdown";

export function parseActive(text: string): StreamSegment[] {
  const lines = text.split("\n");
  const lastActive = !text.endsWith("\n");
  const completeLines = lastActive ? lines.slice(0, -1) : lines;
  const activeLine = lastActive ? lines[lines.length - 1] : null;

  const segments: StreamSegment[] = [];
  let mdBuf: string[] = [];
  let inFence = false;
  let fenceMarker = "";
  let fenceLang = "";
  let codeBuf: string[] = [];

  const flushMd = () => {
    if (mdBuf.length > 0) {
      segments.push({ kind: "md", lines: mdBuf.map((t) => ({ text: t })) });
      mdBuf = [];
    }
  };

  // ── Complete lines ──
  for (const line of completeLines) {
    const trimmed = line.trimStart();
    if (inFence) {
      if (trimmed.startsWith(fenceMarker)) {
        inFence = false;
        segments.push({ kind: "code", text: codeBuf.join("\n"), lang: fenceLang, active: false });
        codeBuf = [];
      } else {
        codeBuf.push(line);
      }
      continue;
    }
    if (trimmed.startsWith("```") || trimmed.startsWith("~~~")) {
      flushMd();
      fenceMarker = trimmed.slice(0, 3);
      fenceLang = trimmed.slice(3).trim();
      inFence = true;
      continue;
    }
    mdBuf.push(line);
  }
  flushMd();

  // ── Active (incomplete) line ──
  if (activeLine !== null) {
    const trimmed = activeLine.trimStart();
    if (inFence) {
      if (!trimmed.startsWith(fenceMarker)) codeBuf.push(activeLine);
      segments.push({ kind: "code", text: codeBuf.join("\n"), lang: fenceLang, active: true });
    } else if (trimmed.startsWith("```") || trimmed.startsWith("~~~")) {
      segments.push({
        kind: "code",
        text: "",
        lang: trimmed.slice(3).trim(),
        active: true,
      });
    } else {
      segments.push({ kind: "md-active", line: { text: activeLine } });
    }
  } else if (inFence) {
    segments.push({ kind: "code", text: codeBuf.join("\n"), lang: fenceLang, active: false });
  }

  return segments;
}


export const CompletedLine = memo(
  function CompletedLine({ text }: { text: string }) {
    return <InlineMarkdown text={text} />;
  },
  (prev, next) => prev.text === next.text,
);

/**
 * Active row — the reveal typewriter lives at BLOCK level (StreamingLines),
 * so this row is always a complete (or block-truncated) line: the revealed
 * prefix is healed (unclosed markers closed for parsing) then rendered
 * inline, so emphasis renders AS IT TYPES. The cursor sits at the reveal
 * frontier.
 */
export function ActiveLine({
  text,
  stalled,
  showCursor = true,
}: {
  text: string;
  stalled: boolean;
  showCursor?: boolean;
}) {
  const format = leadingFormat(text);
  const width = leadingFormatWidth(text);
  const parsed = useMemo(() => parseInline(healInline(text.slice(width))), [text, width]);

  return (
    <LineShell format={format}>
      <InlineTokens tokens={parsed.tokens} />
      {showCursor && <StreamingCursor stalled={stalled} />}
    </LineShell>
  );
}

/** Fenced code segment — container + language label + highlight + cursor.
 *  The reveal typewriter lives at BLOCK level; this segment receives the
 *  already-revealed code text. Highlighting runs in a Shiki web worker (off
 *  the main thread); the worker result renders as tokens with stable index
 *  keys, so React keeps the unchanged prefix DOM untouched and only the
 *  tail re-renders. When the worker is unavailable (jsdom tests, worker
 *  crash) it falls back to the lightweight tokenizer. */
/** Trailing debounce for worker highlight requests (ms). The reveal
 *  advances every ~24ms tick; re-requesting a full Shiki pass on every tick
 *  would flood the worker with growing buffers. One request per quiet
 *  window covers every intermediate state with far fewer worker passes.
 *  80ms keeps the color window small — code colors appear almost as it
 *  types (the reveal ticks every 24ms, so the worker runs ~3 ticks behind,
 *  not 8). */
const HIGHLIGHT_DEBOUNCE_MS = 80;

export function StreamingCodeBlock({
  text,
  lang,
  stalled,
  showCursor = true,
}: {
  text: string;
  lang: string;
  stalled: boolean;
  showCursor?: boolean;
}) {
  const blockId = useId();
  // Worker highlight result, tagged with the text it was computed from.
  // While the worker is debounced/latent behind the growing stream, the
  // stored text won't match the current `text` — the tokens memo then falls
  // back to the lightweight tokenizer so NEW characters are visible
  // immediately instead of waiting for the worker to catch up.
  const [workerResult, setWorkerResult] = useState<{
    text: string;
    tokens: HighlightToken[];
  } | null>(null);
  const [workerFailed, setWorkerFailed] = useState(false);
  const workerEnabled =
    typeof window !== "undefined" && highlightWorkerAvailable();
  // Follow the app theme: dark mode requests github-dark, light github-light.
  const theme: "light" | "dark" =
    typeof document !== "undefined" &&
    document.documentElement.classList.contains("dark")
      ? "dark"
      : "light";

  // Keep the worker in sync with the revealed prefix (debounced — the
  // highlight must never chase every typewriter tick).
  useEffect(() => {
    if (!workerEnabled) {
      setWorkerResult(null);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      void highlightStreaming(blockId, text, lang, theme).then((payload) => {
        if (cancelled || !payload) return; // superseded / unmounted
        if (payload.error) {
          setWorkerFailed(true);
          setWorkerResult(null);
          return;
        }
        setWorkerFailed(false);
        setWorkerResult({ text: payload.text, tokens: payload.tokens });
      });
    }, HIGHLIGHT_DEBOUNCE_MS);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [blockId, text, lang, theme, workerEnabled]);

  // Release the block's worker state when it unmounts / the fence closes.
  useEffect(() => {
    if (!workerEnabled) return;
    return () => disposeHighlight(blockId);
  }, [blockId, workerEnabled]);

  // Worker highlight is authoritative when it matches the CURRENT text;
  // fall back to the lightweight tokenizer when it is unavailable, failed,
  // still pending, OR the stream has outrun it (workerResult.text !== text)
  // — the lightweight pass keeps every typed character visible, and the
  // worker result lights the full text up once it catches up.
  const tokens = useMemo((): RenderToken[] => {
    if (
      workerFailed ||
      !workerEnabled ||
      workerResult === null ||
      workerResult.text !== text
    ) {
      return highlightTokens(text, lang);
    }
    return workerResult.tokens.map((t) => ({
      text: t.text,
      className: null,
      color: t.color,
    }));
  }, [workerFailed, workerEnabled, workerResult, text, lang]);

  const displayLang = lang ? getLanguageDisplayName(lang) : "Code";
  const Icon = getLanguageIcon(lang);
  const lineCount = getLineCount(text);
  const lines = text.split("\n").map((_, i) => i + 1);

  // Chrome mirrors the completed CodeBlock EXACTLY (header chrome, line-number
  // gutter, font size, container) so the streaming → completed swap is a pure
  // content replacement (raw tokens → hljs) with zero header/gutter/font jump.
  return (
    <div className="my-3 overflow-hidden rounded-lg border border-border bg-card">
      <div className="flex items-center justify-between border-b border-border bg-muted/60 px-3 py-2">
        <div className="flex items-center gap-2">
          <Icon className="h-4 w-4 text-muted-foreground" />
          <span className="text-xs uppercase tracking-wider text-muted-foreground">
            {displayLang}
          </span>
          {lineCount > 1 && (
            <span className="ml-1 text-[10px] text-muted-foreground/50">
              {lineCount} lines
            </span>
          )}
        </div>
        <CopyButton text={text} className="text-[10px]" />
      </div>
      <div className="relative flex">
        {lines.length > 0 && (
          <div className="select-none border-r border-border/50 bg-muted/30 py-3 px-2 text-right">
            {lines.map((line) => (
              <div
                key={line}
                className="font-mono text-[11px] leading-5 text-muted-foreground/40"
              >
                {line}
              </div>
            ))}
          </div>
        )}
        <div className="flex-1 overflow-x-auto">
          <pre className="p-3 text-xs leading-5">
            <code>
              {tokens.map((tok, i) => (
                <span
                  key={i}
                  // Soft color transitions — the worker highlight result
                  // "lights up" the code instead of snapping colors in.
                  className={cn(tok.className ?? undefined, "transition-colors duration-150")}
                  style={tok.color ? { color: tok.color } : undefined}
                >
                  {tok.text}
                </span>
              ))}
              {showCursor && <StreamingCursor stalled={stalled} />}
            </code>
          </pre>
        </div>
      </div>
    </div>
  );
}

/** Copy button for STREAMING code blocks — copies the text revealed so far
 *  (the finished CodeBlock already has one; streaming blocks had none). */
export function CopyButton({ text, className }: { text: string; className?: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);

  const copy = () => {
    navigator.clipboard.writeText(text).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    }).catch(() => {
      // Clipboard unavailable — code remains selectable.
    });
  };

  return (
    <button
      onClick={copy}
      className={cn(
        "flex items-center gap-1 rounded px-1.5 py-0.5 transition-colors",
        copied ? "text-green-600" : "text-muted-foreground/60 hover:bg-muted hover:text-foreground",
        className,
      )}
      aria-label={copied ? t("chat.copied", { defaultValue: "已复制" }) : t("chat.copyCode", { defaultValue: "复制代码" })}
    >
      {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
    </button>
  );
}

export function RenderSegment({
  segment,
  stalled,
  showCursor = true,
}: {
  segment: StreamSegment;
  stalled: boolean;
  showCursor?: boolean;
}) {
  if (segment.kind === "md") {
    return (
      <>
        {segment.lines.map((line, i) => (
          <CompletedLine key={i} text={line.text} />
        ))}
      </>
    );
  }
  if (segment.kind === "md-active") {
    return <ActiveLine text={segment.line.text} stalled={stalled} showCursor={showCursor} />;
  }
  return (
    <StreamingCodeBlock
      text={segment.text}
      lang={segment.lang}
      stalled={stalled}
      showCursor={showCursor}
    />
  );
}

/** The active block — renders the already-revealed tail of the streamed
 *  content (the typewriter lives in StreamingMarkdown; this is a pure row
 *  renderer). The tail is parsed into rows; the cursor sits at its end — a
 *  fresh empty row when the reveal paused right after a newline. Wrapped in
 *  one element so the parent's space-y-2 applies to the BLOCK, not to every
 *  row. */
export function StreamingLines({
  text,
  stalled,
  showCursor = true,
}: {
  text: string;
  stalled: boolean;
  showCursor?: boolean;
}) {
  const segments = useMemo(() => parseActive(text), [text]);
  return (
    <div className="stream-text-settle">
      {segments.map((seg, i) => (
        <RenderSegment key={i} segment={seg} stalled={stalled} showCursor={showCursor} />
      ))}
      {/* The reveal frontier sits at the end of the active block — an empty
          tail (a just-closed paragraph, or a blank line) keeps a breathing
          cursor so typing continues visibly there. */}
      {(text === "" || text.endsWith("\n")) && showCursor && (
        <ActiveLine text="" stalled={stalled} />
      )}
    </div>
  );
}

/**
 * Completed block — rendered once with full MarkdownRenderer, never re-parsed.
 * `stream-text-enter` softens the text→markdown transition: when a block
 * crosses the \n\n boundary it switches from row streaming to the parsed
 * renderer; the 0.2s fade-in hides that re-render pop.
 */
export const CompletedBlock = memo(
  function CompletedBlock({ text }: { text: string }) {
    return (
      <div className="stream-text-enter">
        <MarkdownRenderer content={text} interactiveFiles />
      </div>
    );
  },
  (prev, next) => prev.text === next.text,
);

export interface StreamingMarkdownProps {
  content: string;
  /** False once the stream has ended — the final block switches from row
   *  streaming to a full MarkdownRenderer pass; completed blocks are left
   *  untouched (same key + memo → DOM survives, zero-pop). */
  isStreaming?: boolean;
}
