/**
 * StreamStatusLine — "what is the agent doing right now" line.
 *
 * Rendered only on the LAST assistant message while streaming. Follows the
 * quiet-design rule: only appears for phases that have no other visual
 * feedback —
 * - connecting: the message was received and the backend is spinning up the
 *   model before its first token (the longest silent stretch) — an immediate
 *   "已收到，正在处理你的请求…" ack so the user isn't staring at a silent wait.
 * - thinking: shown only when the thinking panel is hidden (otherwise the
 *   ReasoningBlock's own dots carry the state)
 * - generating / tool_running: the streaming text / tool cards already
 *   communicate progress — stay silent.
 * - verifying: a stop-path gate held the turn (verification pending /
 *   evaluator review) — the streamed text so far is NOT final, so the
 *   "checking…" line prevents reading it as a finished answer.
 *
 * The phase comes from the per-session stream store (never the global
 * agent-status channel — multi-session concurrency would clobber it).
 */

import { useEffect, useRef, useState } from "react";
import { Loader2 } from "lucide-react";
import { useTranslation } from "react-i18next";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useAppStore } from "@/stores/appStore";
import { cn } from "@/lib/utils";

function formatWait(ms: number): string {
  const s = Math.floor(ms / 1000);
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export function StreamStatusLine() {
  const { t } = useTranslation();
  // Subscribe to both stores (hooks can't be conditional); only the active
  // mode's phase drives the line. Phases change a few times per turn, so a
  // redundant re-render on the idle store is negligible.
  const chatPhase = useChatStore((s) => s.streamPhase);
  const depworkPhase = useDepworkChatStore((s) => s.streamPhase);
  const appMode = useAppStore((s) => s.mode);
  const streamPhase = appMode === "depwork" ? depworkPhase : chatPhase;
  const showThinking = useSettingsStore((s) => s.general.showThinking);

  // Live wait counter while connecting (1s cadence, only while visible).
  const [elapsed, setElapsed] = useState(0);
  const startRef = useRef(0);
  useEffect(() => {
    if (streamPhase !== "connecting") return;
    startRef.current = Date.now();
    setElapsed(0);
    const iv = setInterval(() => setElapsed(Date.now() - startRef.current), 1000);
    return () => clearInterval(iv);
  }, [streamPhase]);

  const visible =
    streamPhase === "connecting" ||
    streamPhase === "verifying" ||
    (streamPhase === "thinking" && !showThinking);
  if (!visible) return null;

  // Single derived indicator — one spinner, one line. Connecting keeps the
  // live elapsed counter (a bare spinner gives no sense of how long the
  // model has been silent — cold starts can take tens of seconds).
  return (
    <div className="flex items-center gap-1.5 text-xs text-muted-foreground/80">
      <Loader2
        className={cn(
          "h-3 w-3 animate-spin",
          streamPhase === "verifying" && "text-amber-500/80",
        )}
      />
      <span
        className={cn(
          streamPhase === "verifying" ? "text-amber-500/80" : "text-shimmer",
        )}
      >
        {streamPhase === "connecting"
          ? t("chat.receivedProcessing", { defaultValue: "已收到，正在处理你的请求…" })
          : streamPhase === "verifying"
            ? t("chat.streamVerifying")
            : t("chat.streamThinking")}
      </span>
      {streamPhase === "connecting" && (
        <span className="font-mono tabular-nums text-muted-foreground/60">
          {formatWait(elapsed)}
        </span>
      )}
    </div>
  );
}
