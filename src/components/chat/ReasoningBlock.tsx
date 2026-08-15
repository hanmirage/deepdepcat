/**
 * ReasoningBlock — collapsible thinking section with three display modes.
 *
 * Modes (controlled by settingsStore.general.showThinking):
 * - "hidden": Not rendered at all (default)
 * - "collapsed": Shows header with preview, click to expand
 * - "expanded": Always expanded, shows full thinking content
 *
 * Visual design:
 * - Subtle border and background
 * - Animated thinking dots during streaming
 * - Smooth expand/collapse transitions
 */

import { useMemo, useState } from "react";
import { ChevronDown, ChevronUp, Brain } from "lucide-react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import { StreamingText } from "@/components/chat/StreamingText";
import { reasoningHeading } from "@/lib/reasoningHeading";

export type ThinkingDisplayMode = "hidden" | "collapsed" | "expanded";

interface ReasoningBlockProps {
  content: string;
  isStreaming?: boolean;
  mode?: ThinkingDisplayMode;
}

export function ReasoningBlock({
  content,
  isStreaming = false,
  mode = "collapsed",
}: ReasoningBlockProps) {
  const [userExpanded, setUserExpanded] = useState(false);
  const { t } = useTranslation();
  const heading = useMemo(() => reasoningHeading(content), [content]);

  // "hidden" mode: don't render anything
  if (mode === "hidden") {
    return null;
  }

  // Determine if content is visible
  const isExpanded = mode === "expanded" || userExpanded;
  // While streaming, the preview shows the reasoning heading ("思考中 ·
  // 分析需求") when one exists — the user knows WHAT the model is thinking
  // about without expanding; otherwise the live tail. Idle falls back to
  // the heading or the static head slice.
  const preview = isStreaming
    ? heading
      ? `${t("chat.streamThinking")} · ${heading.slice(0, 60)}`
      : `${t("chat.streamThinking")} ${content.slice(-48)}`
    : heading
      ? heading.slice(0, 80)
      : content.slice(0, 80) + (content.length > 80 ? "..." : "");

  return (
    <div
      className={cn(
        "overflow-hidden rounded-lg border border-border/50",
        "bg-muted/20 transition-colors duration-200"
      )}
    >
      {/* Header — always visible */}
      <button
        onClick={() => mode === "collapsed" && setUserExpanded(!userExpanded)}
        className={cn(
          "flex w-full items-center justify-between px-3 py-2",
          "text-left transition-colors",
          mode === "collapsed" && "hover:bg-muted/30 cursor-pointer",
          mode === "expanded" && "cursor-default"
        )}
      >
        <div className="flex items-center gap-2">
          <Brain className="h-3.5 w-3.5 text-muted-foreground/60" />
          <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground/70">
            {t("reasoning.label")}
          </span>
          {isStreaming && (
            <span className="flex items-center gap-0.5">
              <span className="reasoning-dot" />
              <span className="reasoning-dot" />
              <span className="reasoning-dot" />
            </span>
          )}
        </div>

        {mode === "collapsed" && (
          <div className="flex items-center gap-1.5">
            {!isExpanded && content.length > 0 && (
              <span
                className={cn(
                  "max-w-[200px] truncate text-[11px] italic",
                  isStreaming
                    ? "text-shimmer text-muted-foreground/70"
                    : "text-muted-foreground/40",
                )}
              >
                {preview}
              </span>
            )}
            {isExpanded ? (
              <ChevronUp className="h-3.5 w-3.5 text-muted-foreground/50" />
            ) : (
              <ChevronDown className="h-3.5 w-3.5 text-muted-foreground/50" />
            )}
          </div>
        )}
      </button>

      {/* Content — smooth expand/collapse via grid-template-rows (0fr→1fr),
          a pure-CSS height animation that needs no measured pixel values. */}
      <div
        className={cn(
          "reasoning-expand",
          isExpanded && "reasoning-expand-open",
        )}
        data-open={isExpanded}
      >
        <div className="reasoning-expand-inner">
          {isExpanded && (
            <div className="stream-text-settle border-t border-border/30 px-3 py-2">
              {isStreaming ? (
                <StreamingText
                  content={content}
                  className="text-xs whitespace-pre-wrap leading-relaxed text-muted-foreground/70"
                />
              ) : (
                <p className="text-xs whitespace-pre-wrap leading-relaxed text-muted-foreground/70">
                  {content}
                </p>
              )}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
