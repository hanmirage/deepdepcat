/**
 * StreamingText — smooth streaming text with a cursor at the reveal frontier.
 *
 * The backend forwards every LLM delta as it arrives; this component reveals
 * content progressively (see lib/typewriter) so bursty arrivals don't jump
 * in blocks. The cursor sits at the reveal frontier: when the renderer has
 * caught up and no text has arrived for a while (model thinking, a tool
 * running, network hiccup) it switches to slow-breathing — "still alive,
 * just quiet". Any append snaps it back.
 *
 * Accessibility: prefers-reduced-motion disables the cursor animation.
 */

import { useReveal } from "@/lib/typewriter";
import { useSettingsStore } from "@/stores/settingsStore";

interface StreamingTextProps {
  content: string;
  className?: string;
}

/** The live output cursor — pulsing block at the reveal frontier. */
export function StreamingCursor({ stalled }: { stalled: boolean }) {
  return (
    <span
      className={stalled ? "streaming-cursor streaming-cursor-stalled" : "streaming-cursor"}
      aria-hidden="true"
    />
  );
}

export function StreamingText({ content, className }: StreamingTextProps) {
  const instant = useSettingsStore((s) => s.general.streamingSpeed === "instant");
  const { shown, stalled } = useReveal(content, instant);

  return (
    <span className={className ?? "whitespace-pre-wrap text-sm leading-relaxed"}>
      <span>{shown}</span>
      {!instant && <StreamingCursor stalled={stalled} />}
    </span>
  );
}
