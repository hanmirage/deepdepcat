/**
 * VerbSwap — opencode-style state-word cross-fade (读取中 → 已读取).
 *
 * The active and done words share one inline-grid cell; the visible word
 * fades in with a blur-clear + 0.03em rise while the outgoing one blurs
 * out (CSS transitions). The cell WIDTH is measured on swap and animated
 * to the new word's width, so the target text next to it never jumps.
 * The active word carries the text shimmer while running.
 *
 * Shared by ToolCallCard (tool verbs) and ReadGroup (aggregate read verbs).
 */

import { useState, useEffect, useRef } from "react";
import { cn } from "@/lib/utils";

export function VerbSwap({
  activeText,
  doneText,
  active,
  className,
}: {
  activeText: string;
  doneText: string;
  active: boolean;
  className?: string;
}) {
  const [shown, setShown] = useState(active);
  const [width, setWidth] = useState<string | undefined>(undefined);
  const activeRef = useRef<HTMLSpanElement>(null);
  const doneRef = useRef<HTMLSpanElement>(null);
  const frameRef = useRef<number | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (active === shown) return;
    // Measure the CURRENTLY visible word's width before the swap commits.
    const curRef = shown ? activeRef : doneRef;
    const first = curRef.current?.getBoundingClientRect().width;
    setShown(active);
    if (!first) return;
    // Pin the cell at the old width, then transition to the new word's width.
    setWidth(`${Math.ceil(first)}px`);
    frameRef.current = requestAnimationFrame(() => {
      frameRef.current = null;
      const nextRef = active ? activeRef : doneRef;
      const last = nextRef.current?.getBoundingClientRect().width;
      if (last && Math.ceil(last) !== Math.ceil(first)) {
        setWidth(`${Math.ceil(last)}px`);
      }
      timerRef.current = setTimeout(() => setWidth(undefined), 600);
    });
    return () => {
      if (frameRef.current !== null) cancelAnimationFrame(frameRef.current);
      if (timerRef.current !== null) clearTimeout(timerRef.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active]);

  return (
    <span
      className={cn("verb-swap", className)}
      data-active={shown ? "true" : "false"}
      style={width ? { width } : undefined}
    >
      <span
        ref={activeRef}
        className={cn(
          "verb-swap-word verb-swap-active",
          // Last: tailwind-merge would otherwise drop `text-shimmer` as a
          // conflict with the text-* color classes.
          shown && "text-shimmer",
        )}
      >
        {activeText}
      </span>
      <span ref={doneRef} className="verb-swap-word verb-swap-done">
        {doneText}
      </span>
    </span>
  );
}
