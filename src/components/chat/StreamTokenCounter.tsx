/**
 * StreamTokenCounter — Claude-style live token counter during streaming.
 *
 * DeepSeek's OpenAI-format stream carries no mid-stream usage, so output
 * tokens are ESTIMATED from text growth; the exact usage arrives via the
 * `usage` event and lives on the message. The estimate weights by character
 * class — CJK chars run ~0.7/token (~1.4 tokens per hanzi), Latin ~4/token —
 * instead of a flat /3, which under-counts Chinese, the app's primary UI.
 * The rate is a rolling 3-second window.
 *
 * Shows only while streaming, right-aligned next to the status line on the
 * last assistant message; disappears at turn end (Claude behavior). Fully
 * self-contained: subscribes to the last message's text length and computes
 * deltas — zero store changes.
 */

import { useRef, useState, useEffect } from "react";
import type { UIMessage } from "@/types";

/** Rolling rate window (ms). */
const RATE_WINDOW_MS = 3000;

/** Estimated tokens for one string — CJK ~0.7 chars/token, Latin ~4. */
export function estimateTokens(s: string): number {
  let cjk = 0;
  let other = 0;
  for (let i = 0; i < s.length; i++) {
    const code = s.charCodeAt(i);
    if (
      (code >= 0x4e00 && code <= 0x9fff) || // CJK unified ideographs
      (code >= 0x3000 && code <= 0x30ff) || // CJK punctuation + kana
      (code >= 0xff00 && code <= 0xffef) || // fullwidth forms
      (code >= 0xac00 && code <= 0xd7af)    // hangul
    ) {
      cjk++;
    } else {
      other++;
    }
  }
  // CJK ~0.7 chars/token (~1.4 tokens per hanzi) — DeepSeek's BPE splits
  // common Chinese into 1–2 tokens per char; the earlier 1.5 chars/token
  // under-counted real usage (a DeepSeek turn routinely lands 10–30k tokens).
  // Latin stays ~4 chars/token.
  return cjk / 0.7 + other / 4;
}

/** Estimated total tokens across the message's text + reasoning blocks. */
function textTokens(m: UIMessage): number {
  let n = 0;
  for (const b of m.blocks) {
    if (b.type === "text" || b.type === "reasoning") n += estimateTokens(b.content);
  }
  return n;
}

export function StreamTokenCounter({ message }: { message: UIMessage }) {
  const [estimatedTokens, setEstimatedTokens] = useState(0);
  const [rate, setRate] = useState(0);
  const prevLenRef = useRef(textTokens(message));
  const totalRef = useRef(0);
  // Rate window samples: [timestamp, tokensSeenAtTimestamp].
  const windowRef = useRef<{ t: number; tokens: number }[]>([]);

  useEffect(() => {
    const len = textTokens(message);
    const delta = len - prevLenRef.current;
    prevLenRef.current = len;
    if (delta > 0) {
      totalRef.current += delta;
      setEstimatedTokens(Math.round(totalRef.current));
      const now = performance.now();
      windowRef.current.push({ t: now, tokens: totalRef.current });
      windowRef.current = windowRef.current.filter(
        (s) => now - s.t < RATE_WINDOW_MS,
      );
      const oldest = windowRef.current[0];
      if (oldest && now > oldest.t) {
        const dt = (now - oldest.t) / 1000;
        const dc = totalRef.current - oldest.tokens;
        setRate(dt > 0.5 ? Math.round(dc / dt) : 0);
      }
    }
  }, [message]);

  if (estimatedTokens === 0) return null;

  return (
    <span className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground/60">
      {estimatedTokens.toLocaleString()} tok
      {rate > 0 && <span> · {rate}/s</span>}
    </span>
  );
}
