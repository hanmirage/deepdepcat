/**
 * StreamingMarkdown — block-level + row-level incremental markdown renderer.
 *
 * Splits streaming content into top-level blocks at `\n\n` boundaries
 * (respecting fenced code blocks). Completed blocks are rendered once via
 * memoized `MarkdownRenderer` and never re-parsed — their DOM is final and
 * survives turn_end untouched (zero-pop). The last incomplete block
 * ("active block") is rendered at ROW granularity:
 *
 * - completed lines → `InlineMarkdown` (memoized, unchanged lines never
 *   re-render)
 * - the active line → true-content-reveal typewriter (lib/typewriter)
 *   whose revealed prefix is HEALED (lib/markdownHeal) before inline
 *   parsing — `**加粗` renders bold AS IT TYPES, no plain-text asterisks
 *   waiting for the closing pair
 * - a fenced code block in progress → a code container + reveal +
 *   lightweight syntax highlighting (lib/codeHighlight)
 *
 * TURN_END: the component keeps its split structure — completed blocks
 * stay `CompletedBlock` (same key, same text → React leaves the DOM
 * alone), only the final block switches from row-level streaming to a
 * single `MarkdownRenderer` pass. There is no wholesale re-render of the
 * full content, so nothing "pops". (Tradeoff: blocks render standalone
 * rather than as one document — reference-style link definitions that
 * live in a separate block are not resolved. Rare in model output.)
 */

import { memo, useMemo, useEffect, useState, useRef } from "react";
import { StreamingLines, CompletedBlock, type StreamingMarkdownProps } from "@/components/chat/StreamingMarkdownParts";
import { useReveal } from "@/lib/typewriter";
import { useSettingsStore } from "@/stores/settingsStore";

/** Renderable token — either a worker token (inline color) or a
 *  lightweight-tokenizer token (CSS class). */
export interface RenderToken {
  text: string;
  className: string | null;
  color?: string;
}

/** Minimum length before splitting kicks in (short content = single block). */
const MIN_SPLIT_LENGTH = 200;

/** Active-block size cap (chars). Beyond this the block is force-completed
 *  at the last line break outside a fence, so row-level parsing and the
 *  worker highlight never re-scan an unbounded tail (a long code dump with
 *  no blank lines would otherwise never split). */
const MAX_ACTIVE_CHARS = 6000;

/** Final blocks at or below this size flip to MarkdownRenderer immediately
 *  on turn_end (the parse is a few ms — no jank worth deferring). */
const FINALIZE_INLINE_THRESHOLD = 1500;
/** Hard cap on the deferred finalize (ms) — fallback when idle callbacks
 *  are starved (background tab, busy worker). */
const FINALIZE_MAX_DELAY_MS = 250;

export interface Block {
  /** Raw markdown text for this block. */
  text: string;
  /** Absolute source offset where this block starts — the stable React key.
   *  A completed block's offset never changes, so it reconciles instead of
   *  remounting when later blocks complete around it. Absent on the active
   *  tail (which renders under the fixed "active" key). */
  offset?: number;
}


/**
 * Split streaming markdown into completed blocks and one active block.
 */
export function splitBlocks(content: string): [Block[], Block] {
  if (content.length < MIN_SPLIT_LENGTH) {
    return [[], { text: content }];
  }

  const blocks: Block[] = [];
  let inFence = false;
  let fenceMarker = "";
  let lastSplitPos = 0;
  // Position after the most recent line break OUTSIDE a fence — the cap's
  // completion point (a safe place to end a block mid-stream).
  let lastSafeBreak = -1;
  // Start position of the currently-open fence — when the tail is one fence
  // run with no outside-fence breaks, the cap completes the block BEFORE
  // the fence (clean markdown) instead of mid-code. Reset on close/cut.
  let fenceLineStart = -1;

  const lines = content.split("\n");
  let pos = 0;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const trimmed = line.trimStart();

    if (trimmed.startsWith("```") || trimmed.startsWith("~~~")) {
      const marker = trimmed.slice(0, 3);
      if (!inFence) {
        inFence = true;
        fenceMarker = marker;
        fenceLineStart = pos;
      } else if (marker === fenceMarker) {
        inFence = false;
        fenceMarker = "";
        fenceLineStart = -1;
      }
    }

    pos += line.length + 1;
    if (line === "" && !inFence) {
      const blockText = content.slice(lastSplitPos, pos - 1).trimEnd();
      if (blockText) {
        blocks.push({ text: blockText, offset: lastSplitPos });
      }
      lastSplitPos = pos;
      lastSafeBreak = pos;
    } else if (!inFence) {
      lastSafeBreak = pos;
    }

    // Size cap: the tail (no \n\n outside fences left) may still grow huge.
    // Complete it at the last safe line break so parseActive stays bounded.
    if (content.length - lastSplitPos > MAX_ACTIVE_CHARS && lastSafeBreak > lastSplitPos) {
      const blockText = content.slice(lastSplitPos, lastSafeBreak).trimEnd();
      if (blockText) {
        blocks.push({ text: blockText, offset: lastSplitPos });
      }
      lastSplitPos = lastSafeBreak;
      fenceLineStart = -1;
    } else if (content.length - lastSplitPos > MAX_ACTIVE_CHARS && fenceLineStart > lastSplitPos) {
      // Tail is one fence-opened code run with no outside-fence breaks
      // (the content STARTED inside a fence — e.g. a pure code reply).
      // Complete the block before the fence opener: the completed block
      // stays clean markdown and the active block keeps the fence, so it
      // still parses as a code segment.
      const blockText = content.slice(lastSplitPos, fenceLineStart).trimEnd();
      if (blockText) {
        blocks.push({ text: blockText, offset: lastSplitPos });
      }
      lastSplitPos = fenceLineStart;
      fenceLineStart = -1;
    } else if (content.length - lastSplitPos > MAX_ACTIVE_CHARS) {
      // No line break at all in the tail (a single unbounded line — a giant
      // minified JSON dump, a model streaming one enormous in-fence line).
      // Hard-split at a character boundary so parseActive/reveal/highlight
      // stay bounded; the finalize pass at turn_end re-parses the FULL text,
      // so the mid-line cut is only a streaming-time artifact.
      const cut = lastSplitPos + MAX_ACTIVE_CHARS;
      blocks.push({ text: content.slice(lastSplitPos, cut), offset: lastSplitPos });
      lastSplitPos = cut;
      fenceLineStart = -1;
    }
  }

  const remaining = content.slice(lastSplitPos);

  if (blocks.length === 0) {
    return [[], { text: remaining }];
  }

  return [blocks, { text: remaining }];
}

/**
 * Incremental markdown splitter — the streaming-time equivalent of
 * `splitBlocks` that only rescans the newly appended tail instead of the
 * whole text every frame (a long reply rescanned at 15fps is the hottest
 * render cost in the streaming path).
 *
 * State (fence tracking, split positions, completed blocks) persists
 * across feeds; a content REPLACEMENT (shorter than before, or not a
 * prefix — a new turn) resets to a full scan. The previous final line was
 * already scanned (its fence state applied), so the resume pass skips its
 * fence judgment UNLESS the line grew (a fence marker arriving in pieces
 * must still be recognized once it completes).
 */
export class IncrementalSplitter {
  private blocks: Block[] = [];
  private lastSplitPos = 0;
  private inFence = false;
  private fenceMarker = "";
  private lastSafeBreak = -1;
  private fenceLineStart = -1;
  private prevText = "";
  /** Whether splitting has begun (content crossed MIN_SPLIT_LENGTH). Short
   *  content renders whole as the active block, exactly like `splitBlocks` —
   *  the paragraph boundaries below the threshold are never cut. */
  private started = false;

  /** Feed the full current content; returns [completedBlocks, activeBlock]. */
  feed(content: string): [Block[], Block] {
    if (content.length < this.prevText.length || !content.startsWith(this.prevText)) {
      // Content replaced (new turn / shorter) — full rescan from scratch.
      this.reset();
      this.started = false;
      this.prevText = content;
      return this.startOrScan(content);
    }
    if (!this.started) {
      // Still below the split threshold: the whole text is the active
      // block. Crossing the threshold needs a FULL scan (earlier paragraph
      // boundaries were never cut).
      this.prevText = content;
      return this.startOrScan(content);
    }
    if (content.length === this.prevText.length) {
      return [this.blocks, this.currentActive(content)];
    }
    // The previous scan may have advanced the split positions past the
    // string end for the PHANTOM trailing element of split("\n") ("abc\n"
    // → ["abc", ""]; its split advances +1 past the end). That advance is
    // meaningless once the content grows — undo it so the resume pass cuts
    // from the real line boundary instead of one character into the first
    // new line.
    if (this.lastSplitPos > this.prevText.length) {
      this.lastSplitPos = this.prevText.length;
      this.lastSafeBreak = Math.min(this.lastSafeBreak, this.prevText.length);
      if (this.fenceLineStart > this.prevText.length) {
        this.fenceLineStart = -1;
      }
    }
    // Incremental: resume just past the previous final newline. The line
    // starting there was the incomplete final line of the previous feed —
    // rescan it WITHOUT re-applying its fence judgment (already applied),
    // unless the append completed/changed it.
    const resumeAt = this.prevText.lastIndexOf("\n") + 1;
    if (resumeAt === 0) {
      // No newline ever seen — a single unbounded line; the cap-split
      // state is not resumable, fall back to a full scan.
      this.reset();
      this.scan(content, 0, false);
      this.prevText = content;
      return [this.blocks, this.currentActive(content)];
    }
    const prevLastLine = this.prevText.slice(resumeAt);
    const nextLineEnd = content.indexOf("\n", resumeAt);
    const nowLastLine = content.slice(
      resumeAt,
      nextLineEnd === -1 ? content.length : nextLineEnd,
    );
    const rescanFence = prevLastLine !== nowLastLine;
    this.scan(content, resumeAt, !rescanFence);
    this.prevText = content;
    return [this.blocks, this.currentActive(content)];
  }

  /** First split pass: below the threshold render whole, at/above it scan
   *  everything from scratch. */
  private startOrScan(content: string): [Block[], Block] {
    if (content.length < MIN_SPLIT_LENGTH) {
      return [[], { text: content }];
    }
    this.started = true;
    this.scan(content, 0, false);
    return [this.blocks, this.currentActive(content)];
  }

  /** The tail after the last completed split — always present (possibly
   *  empty) so the active block stays mounted and the cursor breathes. */
  private currentActive(content: string): Block {
    return { text: content.slice(this.lastSplitPos) };
  }

  private reset() {
    this.blocks = [];
    this.lastSplitPos = 0;
    this.inFence = false;
    this.fenceMarker = "";
    this.lastSafeBreak = -1;
    this.fenceLineStart = -1;
  }

  /** Scan lines starting at `from` (absolute position), continuing the
   *  persisted state. `skipFirstLineFence` avoids double-applying a fence
   *  judgment on the previously-scanned incomplete final line. */
  private scan(content: string, from: number, skipFirstLineFence: boolean) {
    const lines = content.slice(from).split("\n");
    let pos = from;
    for (let i = 0; i < lines.length; i++) {
      const line = lines[i];
      const trimmed = line.trimStart();
      const applyFence = !(skipFirstLineFence && i === 0);

      if (applyFence && (trimmed.startsWith("```") || trimmed.startsWith("~~~"))) {
        const marker = trimmed.slice(0, 3);
        if (!this.inFence) {
          this.inFence = true;
          this.fenceMarker = marker;
          this.fenceLineStart = pos;
        } else if (marker === this.fenceMarker) {
          this.inFence = false;
          this.fenceMarker = "";
          this.fenceLineStart = -1;
        }
      }

      pos += line.length + 1;
      if (line === "" && !this.inFence) {
        const blockText = content.slice(this.lastSplitPos, pos - 1).trimEnd();
        if (blockText) {
          this.blocks.push({ text: blockText, offset: this.lastSplitPos });
        }
        this.lastSplitPos = pos;
        this.lastSafeBreak = pos;
      } else if (!this.inFence) {
        this.lastSafeBreak = pos;
      }

      // Size cap: identical rules to `splitBlocks` — complete the tail at
      // the last safe line break (or hard-cut an unbounded line).
      if (content.length - this.lastSplitPos > MAX_ACTIVE_CHARS && this.lastSafeBreak > this.lastSplitPos) {
        const blockText = content.slice(this.lastSplitPos, this.lastSafeBreak).trimEnd();
        if (blockText) {
          this.blocks.push({ text: blockText, offset: this.lastSplitPos });
        }
        this.lastSplitPos = this.lastSafeBreak;
        this.fenceLineStart = -1;
      } else if (content.length - this.lastSplitPos > MAX_ACTIVE_CHARS && this.fenceLineStart > this.lastSplitPos) {
        const blockText = content.slice(this.lastSplitPos, this.fenceLineStart).trimEnd();
        if (blockText) {
          this.blocks.push({ text: blockText, offset: this.lastSplitPos });
        }
        this.lastSplitPos = this.fenceLineStart;
        this.fenceLineStart = -1;
      } else if (content.length - this.lastSplitPos > MAX_ACTIVE_CHARS) {
        const cut = this.lastSplitPos + MAX_ACTIVE_CHARS;
        this.blocks.push({ text: content.slice(this.lastSplitPos, cut), offset: this.lastSplitPos });
        this.lastSplitPos = cut;
        this.fenceLineStart = -1;
      }
    }
  }
}

// ── Active-block row segments ──────────────────────────────────

interface MdLine {
  text: string;
}

export type StreamSegment =
  | { kind: "md"; lines: MdLine[] }
  | { kind: "md-active"; line: MdLine }
  | { kind: "code"; text: string; lang: string; active: boolean };

/**
 * Parse the active block into renderable row segments. A row is "md" when
 * its line ended with a newline; the final line without a trailing newline
 * is the active (still typing) row. Fenced code runs become code segments —
 * the fence markers themselves are dropped (the container + language label
 * stand in for them).
 */
// ── Row renderers ──────────────────────────────────────────────

/** Completed row — rendered once per row text; unchanged rows skip. */function StreamingMarkdownImpl({ content, isStreaming = true }: StreamingMarkdownProps) {
  const instant = useSettingsStore((s) => s.general.streamingSpeed === "instant");
  // The typewriter paces the WHOLE stream here (not just the last paragraph):
  // the splitter only ever sees the REVEALED prefix, so a paragraph migrates
  // to the completed renderer only after its text has actually been typed —
  // a DeepSeek burst can no longer snap a \n\n-complete paragraph in whole.
  const { shown, caughtUp, stalled } = useReveal(content, instant, true);

  // Incremental splitter: only the appended tail is rescanned per frame
  // (a long reply at 15fps used to rescan the whole text every render).
  const splitterRef = useRef<IncrementalSplitter | null>(null);
  const [completed, active] = useMemo(() => {
    if (!splitterRef.current) splitterRef.current = new IncrementalSplitter();
    return splitterRef.current.feed(shown);
  }, [shown]);

  // Deferred finalize: when a turn ends on a LARGE final block, the switch
  // to MarkdownRenderer (full parse + highlight, on the main thread) can
  // jank for 100-300ms — right when the stream "should" feel done. Small
  // blocks flip inline; large ones wait for an idle callback (bounded by
  // FINALIZE_MAX_DELAY_MS) while the row renderer keeps showing the
  // revealed text, so the stream settles instead of stuttering. Also waits
  // for the reveal to catch up, so the final block is never cut short.
  const [finalized, setFinalized] = useState(!isStreaming);
  const activeLen = active.text.length;

  useEffect(() => {
    if (isStreaming) {
      setFinalized(false);
      return;
    }
    if (!caughtUp) return;
    if (activeLen < FINALIZE_INLINE_THRESHOLD) {
      setFinalized(true);
      return;
    }
    let done = false;
    const finish = () => {
      if (!done) {
        done = true;
        setFinalized(true);
      }
    };
    const timer = setTimeout(finish, FINALIZE_MAX_DELAY_MS);
    const idleId =
      typeof window !== "undefined" && typeof window.requestIdleCallback === "function"
        ? window.requestIdleCallback(finish, { timeout: FINALIZE_MAX_DELAY_MS })
        : null;
    return () => {
      done = true;
      clearTimeout(timer);
      if (idleId !== null && typeof window.cancelIdleCallback === "function") {
        window.cancelIdleCallback(idleId);
      }
    };
  }, [isStreaming, activeLen, caughtUp]);

  return (
    <>
      {completed.map((block, idx) => (
        <CompletedBlock key={block.offset ?? idx} text={block.text} />
      ))}
      {isStreaming || !finalized ? (
        <StreamingLines
          key="active"
          text={active.text}
          stalled={stalled}
          // Turn-end: once the reveal catches up the cursor disappears
          // immediately — no dead breathing cursor during the finalize wait.
          showCursor={!instant && (isStreaming || !caughtUp)}
        />
      ) : active.text !== "" ? (
        <CompletedBlock key="final" text={active.text} />
      ) : null}
    </>
  );
}

export const StreamingMarkdown = memo(StreamingMarkdownImpl);
