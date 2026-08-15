/**
 * typewriter — smooth reveal pacing for streaming text.
 *
 * The backend forwards every LLM delta as it arrives (no pacing); the
 * frontend reducer accumulates them by seq and patches state per frame.
 * This module gives the *renderer* a smooth reveal so bursty arrivals don't
 * appear as block jumps:
 *
 *  - small deltas (≤ REVEAL_IMMEDIATE) render instantly
 *  - larger backlogs reveal progressively on a tick and race to catch up
 *  - truncation (StreamingMarkdown migrates a completed block out of the
 *    active line) syncs immediately
 *
 * Stall sense: once the renderer has CAUGHT UP with the stream and no new
 * text has arrived for STALL_MS, `stalled` flips (the cursor breathes
 * slowly — "still alive, just quiet"). Any append snaps it back.
 */

import { useEffect, useRef, useState } from "react";

/** Backlog (chars) up to which content renders immediately — tiny deltas
 *  land as they arrive; anything larger reveals on a tick so fast models
 *  (DeepSeek bursts ~40-200 chars per wire delta) get a smooth typewriter
 *  instead of whole-batch jumps. */
export const REVEAL_IMMEDIATE = 24;
/** Tick interval for paced reveals (ms). */
export const REVEAL_PACE_MS = 24;
/** Look-ahead (chars) when snapping a step forward to a word boundary. */
const SNAP_LOOKAHEAD = 8;
/** Characters that terminate a "word" for the boundary snap — ASCII space/
 *  punctuation plus the CJK sentence/clause terminators and full-width forms,
 *  so Chinese text lands on natural boundaries too (。、！？；：…「」…). */
const SNAP_CHARS = /[\s.,!?;:)\]，。、！？；：…「」『』（）【】《》〈〉～·]/;
/** Silence (ms) before the cursor switches to the stall breathing. */
export const STALL_MS = 1500;

/**
 * How many chars to reveal in one tick for a given remaining backlog.
 * Monotonic bounded curve: the tail settles at a readable pace while large
 * backlogs type at a fixed ceiling (~1300 c/s) — still a visible typewriter,
 * never a fast-forward, yet fast enough to keep pace with a hot model.
 */
export function stepFor(remaining: number): number {
  if (remaining <= 12) return 3;
  if (remaining <= 48) return 6;
  if (remaining <= 96) return 12;
  if (remaining <= 384) return 20;
  return 32;
}

/**
 * End position for a reveal starting at `start`: step forward from `start`,
 * then snap to the next word boundary (up to SNAP_LOOKAHEAD chars beyond).
 * Guaranteed to advance when `start < text.length` — a reveal can never
 * deadlock on a zero-progress tick.
 */
export function nextEnd(text: string, start: number): number {
  const step = stepFor(text.length - start);
  const end = Math.max(
    Math.min(text.length, start + step),
    start < text.length ? start + 1 : start,
  );
  const max = Math.min(text.length, end + SNAP_LOOKAHEAD);
  for (let i = end; i < max; i++) {
    if (SNAP_CHARS.test(text[i])) return i + 1;
  }
  return end;
}

export interface RevealResult {
  /** Content revealed so far (a prefix of `content`). */
  shown: string;
  /** True when `shown` has caught up with `content` (renderer idle). */
  caughtUp: boolean;
  /** True when caught up AND the stream has been quiet for >STALL_MS. */
  stalled: boolean;
}

/**
 * Smooth reveal for streaming content — deltas land as they arrive; the
 * reveal keeps pace without racing and catches up when the stream bursts.
 *
 * `revealOnMount` starts from a small prefix so the FIRST chunk types out
 * too (a fresh component would otherwise paint its opening burst instantly).
 */
export function useReveal(
  content: string,
  instant = false,
  revealOnMount = false,
): RevealResult {
  // Mount seed: with revealOnMount, always start from a small prefix so the
  // FIRST chunk types out too — a fresh component would otherwise paint its
  // opening burst instantly. Size is deliberately NOT a factor: a live stream
  // can open with a large delta, and that opening must type, never snap.
  const mountShown =
    revealOnMount && content.length > REVEAL_IMMEDIATE
      ? content.slice(0, REVEAL_IMMEDIATE)
      : content;
  const mountCaughtUp = !revealOnMount || content.length <= REVEAL_IMMEDIATE;

  const [shown, setShown] = useState<string>(mountShown);
  const [caughtUp, setCaughtUp] = useState<boolean>(mountCaughtUp);
  const [stalled, setStalled] = useState<boolean>(false);

  const contentRef = useRef(mountShown);
  const shownRef = useRef(mountShown);
  const caughtUpRef = useRef(mountCaughtUp);
  // Mounted with text counts as an "arrival" — a stream that goes quiet
  // from the start still breathes the stall cursor.
  const lastAppendAtRef = useRef(content.length > 0 ? Date.now() : 0);
  const stalledRef = useRef(false);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const stallTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Self-arming stall timer: fires exactly STALL_MS after the last append /
  // catch-up instead of a coarse 500ms poll, and only exists while a reveal
  // instance is mounted (no forever-running intervals). NO static catch-up:
  // a live turn's big burst must keep typing through tool pauses — jumping
  // the reveal to the end at the quiet window was the "一次性出来" bug.
  const armStall = () => {
    if (stallTimerRef.current !== null) clearTimeout(stallTimerRef.current);
    stallTimerRef.current = setTimeout(() => {
      stallTimerRef.current = null;
      const stalledNow =
        lastAppendAtRef.current > 0 &&
        Date.now() - lastAppendAtRef.current >= STALL_MS &&
        caughtUpRef.current;
      if (stalledNow !== stalledRef.current) {
        stalledRef.current = stalledNow;
        setStalled(stalledNow);
      }
    }, STALL_MS);
  };

  useEffect(() => {
    if (instant) {
      // Instant mode — reveal the full content immediately, no typewriter
      // ticks, no cursor stall state.
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      if (stallTimerRef.current !== null) {
        clearTimeout(stallTimerRef.current);
        stallTimerRef.current = null;
      }
      shownRef.current = content;
      contentRef.current = content;
      setShown(content);
      if (!caughtUpRef.current) {
        caughtUpRef.current = true;
        setCaughtUp(true);
      }
      if (stalledRef.current) {
        stalledRef.current = false;
        setStalled(false);
      }
      return;
    }

    const sync = (text: string) => {
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
      shownRef.current = text;
      contentRef.current = text;
      setShown(text);
      if (!caughtUpRef.current) {
        caughtUpRef.current = true;
        setCaughtUp(true);
        armStall();
      }
    };

    const tick = () => {
      timerRef.current = null;
      const text = contentRef.current;
      const start = shownRef.current.length;
      const end = nextEnd(text, start);
      shownRef.current = text.slice(0, end);
      setShown(shownRef.current);
      if (end < text.length) {
        timerRef.current = setTimeout(tick, REVEAL_PACE_MS);
      } else if (!caughtUpRef.current) {
        caughtUpRef.current = true;
        setCaughtUp(true);
        armStall();
      }
    };

    const prev = contentRef.current;
    contentRef.current = content;

    if (content.length < shownRef.current.length) {
      // Truncation (block migration) — sync instantly.
      sync(content);
      return;
    }

    // Every arrival — the initial content on mount included — restarts the
    // stall countdown (the mount path falls through here before the
    // `content === prev` early return, so a quiet stream still breathes).
    if (content.length > 0) {
      lastAppendAtRef.current = Date.now();
      if (stalledRef.current) {
        stalledRef.current = false;
        setStalled(false);
      }
      armStall();
    }

    if (content === prev) return;

    const remaining = content.length - shownRef.current.length;
    if (remaining <= REVEAL_IMMEDIATE) {
      sync(content);
      return;
    }

    if (caughtUpRef.current) {
      caughtUpRef.current = false;
      setCaughtUp(false);
    }
    if (timerRef.current === null) {
      timerRef.current = setTimeout(tick, REVEAL_PACE_MS);
    }
  }, [content, instant]);

  // Clear any pending reveal/stall timers on unmount.
  useEffect(() => {
    return () => {
      if (timerRef.current !== null) clearTimeout(timerRef.current);
      if (stallTimerRef.current !== null) clearTimeout(stallTimerRef.current);
    };
  }, []);

  return { shown, caughtUp, stalled };
}
