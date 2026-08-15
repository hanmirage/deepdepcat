/**
 * AssistantMessage — narrative text flows in execution order, with each tool
 * call rendered as its own bare terminal-log line right where it happens.
 *
 * Blocks render in their original order — text streams, tool calls appear
 * as one thin line each (running: verb shimmer; done: check + verb + target),
 * then more text, and so on:
 *
 *   好的，先看一下工作目录…
 *   ● 读取中 C:\workspace           ← bare tool line in the flow
 *   ✓ 已读取 2 个文件
 *   ✓ 修改完成，验证结果如下…        ← summary text follows the tools
 *
 * ask_user stays inline (it needs the user's attention).
 */

import { useState, memo, useMemo } from "react";
import { useTranslation } from "react-i18next";
import {
  Copy,
  Check,
  ThumbsUp,
  ThumbsDown,
  Undo2,
  Brain,
  AlertTriangle,
  CheckCircle2,
  Play,
  RotateCcw,
} from "lucide-react";
import { cn } from "@/lib/utils";
import type { MessageBlock, ToolCallState, TurnOutcome } from "@/types";
import { useSettingsStore } from "@/stores/settingsStore";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useStore } from "zustand";
import { useAppStore } from "@/stores/appStore";
import { useAuthStore } from "@/stores/authStore";
import { cloudApi } from "@/lib/tauri";
import { MarkdownRenderer } from "@/components/chat/MarkdownRenderer";
import { StreamingMarkdown } from "@/components/chat/StreamingMarkdown";
import { ToolCallCard } from "@/components/chat/ToolCallCard";
import { ParallelGroup } from "@/components/chat/ParallelGroup";
import { ChangesSummaryCard } from "@/components/chat/ChangesSummaryCard";
import { ReadGroup } from "@/components/chat/ReadGroup";
import { AskUserCard } from "@/components/chat/AskUserCard";
import { ReasoningBlock } from "@/components/chat/ReasoningBlock";
import { ErrorBlock } from "@/components/chat/ErrorBlock";
import { ArtifactCard } from "@/components/chat/ArtifactCard";
import { StreamStatusLine } from "@/components/chat/StreamStatusLine";
import { StreamTokenCounter } from "@/components/chat/StreamTokenCounter";
import { stripToolCallMarkup } from "@/types/chat";
import { buildSegments, type Segment } from "@/components/chat/segments";

/** Stable empty block list for a missing message (hooks must stay stable). */
const EMPTY_BLOCKS: MessageBlock[] = [];
import { formatTime } from "@/lib/utils";

/** Non-done terminal statuses → i18n key. `done` is the accepted terminal
 *  state and renders with its own checkmark (never a warning). */
const TURN_STATUS_I18N: Partial<Record<TurnOutcome, string>> = {
  limit: "chat.turnStatusLimit",
  cancelled: "chat.turnStatusCancelled",
  failed: "chat.turnStatusFailed",
  denied: "chat.turnStatusDenied",
  needs_input: "chat.turnStatusNeedsInput",
};

interface AssistantMessageProps {
  /** Message id — the component subscribes to its store object directly.
   *  During streaming flushes only the streamed message changes reference,
   *  so every OTHER message skips re-rendering (zustand Object.is). */
  messageId: string;
  /** True for the LAST assistant message — the only one that may show the
   *  live turn-phase status line (connecting / thinking). */
  showStreamStatus?: boolean;
}

type ReasoningBlock = Extract<MessageBlock, { type: "reasoning" }>;
type TextBlock = Extract<MessageBlock, { type: "text" }>;

// ── Streaming "still working" indicator ──

function StreamingDots() {
  return (
    <div className="flex items-center gap-1.5 text-sm text-muted-foreground">
      <span className="flex gap-0.5">
        <span className="animate-thinking-dot h-1.5 w-1.5 rounded-full bg-muted-foreground" />
        <span className="animate-thinking-dot h-1.5 w-1.5 rounded-full bg-muted-foreground" />
        <span className="animate-thinking-dot h-1.5 w-1.5 rounded-full bg-muted-foreground" />
      </span>
    </div>
  );
}

// ── Render a single narrative block inline ──

function RenderBlock({
  block,
  isStreaming,
  isLastTextBlock,
}: {
  block: MessageBlock;
  isStreaming: boolean;
  /** true when this text block is the one actively receiving deltas */
  isLastTextBlock: boolean;
}) {
  const isStreamingActive = isStreaming && isLastTextBlock;

  if (block.type === "text") {
    // Display-side strip: the backend sanitizes tool-call protocol markup
    // before persisting; this covers history written by older builds.
    const content = stripToolCallMarkup(block.content);
    return isStreamingActive ? (
      <StreamingMarkdown content={content} isStreaming />
    ) : (
      <MarkdownRenderer content={content} interactiveFiles />
    );
  }

  if (block.type === "error") {
    return <ErrorBlock content={block.content} />;
  }

  if (block.type === "artifact") {
    return <ArtifactCard artifact={block} />;
  }

  if (block.type === "tool_call" && block.tool.name === "ask_user") {
    return <AskUserCard tool={block.tool} />;
  }

  return null;
}

// ── Main component ──

export const AssistantMessage = memo(function AssistantMessage({
  messageId,
  showStreamStatus = false,
}: AssistantMessageProps) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const [showTimestamp, setShowTimestamp] = useState(false);
  const [feedback, setFeedback] = useState<"like" | "dislike" | null>(null);

  // Cloud feedback: like → praise (5), dislike → general (1). Uploaded to
  // the website's public /api/feedback via the Rust side (no CORS there).
  // Best-effort — a failed upload never blocks or bothers the user.
  const serverUrl = useAuthStore((s) => s.serverUrl);
  const toggleFeedback = (kind: "like" | "dislike") => {
    const next = feedback === kind ? null : kind;
    setFeedback(next);
    if (next && message) {
      const excerpt =
        message.blocks
          .filter((b) => b.type === "text")
          .map((b) => b.content)
          .join("\n")
          .slice(0, 200) || "(no text)";
      void cloudApi
        .submitFeedback(
          serverUrl,
          next === "like" ? 5 : 1,
          `[${message.model ?? "unknown"}] ${excerpt}`,
          next === "like" ? "praise" : "general",
        )
        .catch(() => {
          /* feedback is fire-and-forget */
        });
    }
  };

  const showThinking = useSettingsStore((s) => s.general.showThinking);
  const thinkingMode = showThinking ? "collapsed" : "hidden";
  const deleteMessage = useChatStore((s) => s.deleteMessage);
  const depworkDeleteMessage = useDepworkChatStore((s) => s.deleteMessage);
  const appMode = useAppStore((s) => s.mode);
  // Message-level subscription — the active mode's array drives the render.
  // Unchanged message objects keep their reference, so zustand's Object.is
  // check skips this component entirely on unrelated flushes (the O(blocks)
  // streamingSignature hash is gone — long sessions now cost nothing). The
  // #79 factory unified both stores on one message type (UIMessage), so no
  // cast is needed anymore.
  const chatStore = appMode === "depwork" ? useDepworkChatStore : useChatStore;
  // Message-level subscription — the active mode's array drives the render.
  const message = useStore(chatStore, (s) => s.messages.find((m) => m.id === messageId));
  // Memory auto-injected into this turn — the "已引用记忆" marker.
  const memoryRef = useStore(chatStore, (s) => s.memoryRef);
  const streamPhase = useStore(chatStore, (s) => s.streamPhase);
  // Latency from turn_start to the first streamed token (set on first delta).
  const firstTokenLatencyMs = useStore(chatStore, (s) => s.firstTokenLatencyMs);
  // The recall action must hit the mode's OWN store or it silently no-ops.
  const recallMessage = appMode === "depwork" ? depworkDeleteMessage : deleteMessage;

  // ALL hooks must run before any early return — a message that vanishes
  // mid-mount (session switch/clear) would otherwise change the hook count
  // between renders and crash the whole tree.
  const isStreaming = message?.isStreaming ?? false;
  const blocks = message?.blocks ?? EMPTY_BLOCKS;

  // Reasoning blocks render only when the thinking panel is enabled —
  // stripped here (per-message) instead of by MessageList's filtered copy.
  const reasoningBlocks = useMemo(
    () =>
      showThinking
        ? blocks.filter((b): b is ReasoningBlock => b.type === "reasoning")
        : [],
    [showThinking, blocks],
  );

  // Build ordered renderable segments (pure — see segments.ts).
  const segments = useMemo(
    () => buildSegments(blocks),
    [blocks],
  );

  // The text block currently receiving deltas — the last text block overall.
  const lastTextBlock = useMemo(() => {
    for (let i = segments.length - 1; i >= 0; i--) {
      const seg = segments[i];
      if (seg.kind === "block" && seg.block.type === "text") return seg.block;
    }
    return null;
  }, [segments]);

  // True when this turn actually executed tools — the closing text block is
  // then the post-work summary (Claude-style: more breathing room above it,
  // no decoration). Pure Q&A (no tools) keeps every paragraph uniform.
  const hasToolActivity = useMemo(
    () =>
      segments.some(
        (s) => s.kind === "tool" || s.kind === "readGroup" || s.kind === "parallelGroup",
      ),
    [segments],
  );

  // End-of-turn change summary (appended by the store on turn_end) — the
  // single review surface for what this turn changed.
  const changesSummary = useMemo(() => {
    const block = blocks.find((b) => b.type === "changes_summary");
    return block?.type === "changes_summary" ? block.changes : null;
  }, [blocks]);

  // Full text for copy (all text blocks joined)
  const fullText = useMemo(
    () =>
      segments
        .filter((s): s is Extract<Segment, { kind: "block" }> => s.kind === "block" && s.block.type === "text")
        .map((s) => (s.block as TextBlock).content)
        .join("\n"),
    [segments],
  );

  // A turn that ended with an error block offers "重试" (re-send the last
  // user message from a clean truncation point); a healthy last turn offers
  // "继续" (a bare continuation prompt). Both only appear on the LAST
  // assistant message (showStreamStatus marks it).
  const hasErrorBlock = useMemo(
    () => blocks.some((b) => b.type === "error"),
    [blocks],
  );

  // A visible status line already explains what the agent is doing — never
  // stack the bare streaming dots on top of it (one indicator at a time).
  const statusLineVisible =
    isStreaming &&
    (streamPhase === "connecting" ||
      streamPhase === "verifying" ||
      (streamPhase === "thinking" && !showThinking));

  if (!message) return null;

  const handleCopy = async () => {
    if (!fullText) return;
    await navigator.clipboard.writeText(fullText);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  const continueOrRetry = async () => {
    const store = appMode === "depwork" ? useDepworkChatStore : useChatStore;
    const st = store.getState();
    if (hasErrorBlock) {
      const lastUser = [...st.messages].reverse().find((m) => m.role === "user");
      if (!lastUser) return;
      const text = lastUser.blocks
        .filter((b) => b.type === "text")
        .map((b) => b.content)
        .join("\n");
      if (!text) return;
      // Truncate at the user message so the retry starts from a clean
      // history (the failed turn disappears from backend + UI alike).
      await recallMessage(lastUser.id);
      st.setInputText(text);
    } else {
      st.setInputText(t("chat.continuePrompt", { defaultValue: "继续" }));
    }
    void st.sendMessage();
  };

  return (
    <div
      className="group flex gap-3"
      onMouseEnter={() => setShowTimestamp(true)}
      onMouseLeave={() => setShowTimestamp(false)}
    >
      {/* 消息内容区 */}
      <div className="min-w-0 flex-1 space-y-3">
        {/* 收紧 markdown 段落间距——assistant 文本里相邻段落不再隔一条空白。
            只作用于聊天文本（chat-prose），不影响 depwork 预览等其他 prose。
            aria-live: while streaming the content mutates ~15×/sec — silence
            it for assistive tech (same approach as other agents); once the
            turn ends the reply is announced once. */}
        <div
          className="chat-prose space-y-2"
          aria-live={isStreaming ? "off" : "polite"}
        >
          {/* 0. Live turn phase — only on the last message, only for
              phases without other visual feedback */}
          {showStreamStatus && isStreaming && <StreamStatusLine />}

          {/* 1. Reasoning — collapsible, always at top */}
          {reasoningBlocks.length > 0 && thinkingMode !== "hidden" && (
            <ReasoningBlock
              key="reasoning"
              content={reasoningBlocks.map((b) => b.content).join("")}
              isStreaming={isStreaming}
              mode="collapsed"
            />
          )}

          {/* 2. Blocks in original order — text streams; each tool call is
              its own bare line at its own spot. */}
          {segments.map((seg, idx) => {
            if (seg.kind === "tool") {
              return (
                <ToolCallCard
                  key={seg.tool.id}
                  tool={seg.tool}
                />
              );
            }
            if (seg.kind === "parallelGroup") {
              return (
                <ParallelGroup
                  key={`pg:${seg.tools[0].id}`}
                  tools={seg.tools}
                />
              );
            }
            if (seg.kind === "readGroup") {
              return (
                <ReadGroup
                  key={`rg:${seg.tools[0].id}`}
                  tools={seg.tools}
                />
              );
            }
            const block = seg.block;
            const isText = block.type === "text";
            const isLastText = isText && lastTextBlock === block;
            // Claude-style post-work summary: only the closing text block of
            // a turn that ran tools gets the extra breathing room above it —
            // pure rhythm, no box/line/label.
            const isSummary = isText && isLastText && hasToolActivity;
            const key = `${idx}:${isText ? "t" : "a"}`;
            return (
              <div key={key} className={isSummary ? "final-summary" : undefined}>
                <RenderBlock
                  block={block}
                  isStreaming={isStreaming}
                  isLastTextBlock={isLastText}
                />
              </div>
            );
          })}

          {/* Streaming dots when nothing rendered yet AND no status line is
              already carrying the state. */}
          {segments.length === 0 && isStreaming && !statusLineVisible && (
            <StreamingDots />
          )}

          {/* 3. End-of-turn change summary — one card, file list + diffs */}
          {!isStreaming && changesSummary && changesSummary.length > 0 && (
            <ChangesSummaryCard changes={changesSummary} />
          )}

          {/* 4. Terminal status — `done` is the accepted end state; anything
              else is an explicit non-normal outcome the user should see
              (limit / cancelled / denied / failed / needs_input). */}
          {!isStreaming && message.turnOutcome && (
            <div className="flex items-center gap-1.5 pt-1 text-[11px] text-muted-foreground/80">
              {message.turnOutcome === "done" ? (
                <>
                  <CheckCircle2 className="h-3.5 w-3.5 text-emerald-500/80" />
                  <span>{t("chat.turnStatusDone")}</span>
                </>
              ) : (
                <>
                  <AlertTriangle className="h-3.5 w-3.5 text-amber-500/80" />
                  <span>
                    {t(
                      TURN_STATUS_I18N[message.turnOutcome] ?? "chat.turnStatusDone",
                    )}
                  </span>
                </>
              )}
            </div>
          )}
        </div>

        {/* ── Live token counter + memory reference — at the BOTTOM of the
            reply, only while streaming (disappears at end). Copy stays
            available mid-stream — the user may want to grab generated code
            before the turn ends. */}
        {showStreamStatus && isStreaming && (
          <div className="flex items-center justify-end gap-2">
            {memoryRef && (
              <span
                className="flex min-w-0 items-center gap-1 text-[10px] text-muted-foreground/70"
                title={t("chat.memoryRefTitle")}
              >
                <Brain className="h-2.5 w-2.5 shrink-0" />
                <span className="truncate">
                  {t("chat.memoryRef")} · {memoryRef.snippet}
                </span>
              </span>
            )}
            {firstTokenLatencyMs != null && (
              <span
                className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground/60"
                title={t("chat.firstTokenTitle", { defaultValue: "首 token 延迟" })}
              >
                {t("chat.firstToken", {
                  defaultValue: "首 token {{s}}s",
                  s: (firstTokenLatencyMs / 1000).toFixed(1),
                })}
              </span>
            )}
            <StreamTokenCounter message={message} />
            {fullText && (
              <button
                onClick={handleCopy}
                className={cn(
                  "flex h-6 items-center gap-1 rounded px-1.5 text-[11px] transition-colors",
                  copied
                    ? "text-green-600"
                    : "text-muted-foreground hover:bg-muted hover:text-foreground",
                )}
                title={copied ? t("chat.copied") : t("chat.copy")}
              >
                {copied ? (
                  <Check className="h-3 w-3" />
                ) : (
                  <Copy className="h-3 w-3" />
                )}
                {copied ? t("chat.copied") : t("chat.copy")}
              </button>
            )}
          </div>
        )}

        {/* ── Action bar ────────────────────────────────────── */}
        {!isStreaming && (
          <div className="flex items-center gap-1">
            {/* Copy stays visible — the highest-frequency action; the rest
                fades in on hover or keyboard focus. */}
            <button
              onClick={handleCopy}
              className={cn(
                "flex h-6 items-center gap-1 rounded px-1.5 text-[11px] transition-colors",
                copied
                  ? "text-green-600"
                  : "text-muted-foreground hover:bg-muted hover:text-foreground",
              )}
              title={copied ? t("chat.copied") : t("chat.copy")}
              disabled={!fullText}
            >
              {copied ? (
                <Check className="h-3 w-3" />
              ) : (
                <Copy className="h-3 w-3" />
              )}
              {copied ? t("chat.copied") : t("chat.copy")}
            </button>

            <span className="flex items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
              <div className="mx-1 h-3 w-px bg-border" />

              <button
                onClick={() => toggleFeedback("like")}
                className={cn(
                  "flex h-6 items-center gap-1 rounded px-1.5 text-[11px] transition-colors",
                  feedback === "like"
                    ? "text-primary"
                    : "text-muted-foreground hover:bg-muted hover:text-foreground",
                )}
                title={t("chat.helpful", { defaultValue: "有帮助" })}
                aria-pressed={feedback === "like"}
              >
                <ThumbsUp className={cn("h-3 w-3", feedback === "like" && "fill-current")} />
              </button>

              <button
                onClick={() => toggleFeedback("dislike")}
                className={cn(
                  "flex h-6 items-center gap-1 rounded px-1.5 text-[11px] transition-colors",
                  feedback === "dislike"
                    ? "text-destructive"
                    : "text-muted-foreground hover:bg-muted hover:text-foreground",
                )}
                title={t("chat.notHelpful", { defaultValue: "无帮助" })}
                aria-pressed={feedback === "dislike"}
              >
                <ThumbsDown className={cn("h-3 w-3", feedback === "dislike" && "fill-current")} />
              </button>

              <div className="mx-1 h-3 w-px bg-border" />

              <button
                onClick={() => void recallMessage(message.id)}
                className="flex h-6 items-center gap-1 rounded px-1.5 text-[11px] text-muted-foreground transition-colors hover:bg-muted hover:text-destructive"
                title={t("chat.recall", { defaultValue: "撤回" })}
              >
                <Undo2 className="h-3 w-3" />
                {t("chat.recall", { defaultValue: "撤回" })}
              </button>

              {showStreamStatus && (
                <button
                  onClick={() => void continueOrRetry()}
                  className={cn(
                    "flex h-6 items-center gap-1 rounded px-1.5 text-[11px] transition-colors",
                    hasErrorBlock
                      ? "text-destructive/80 hover:bg-destructive/10 hover:text-destructive"
                      : "text-primary hover:bg-primary/10",
                  )}
                  title={
                    hasErrorBlock
                      ? t("common.retry")
                      : t("chat.continue", { defaultValue: "继续" })
                  }
                >
                  {hasErrorBlock ? (
                    <RotateCcw className="h-3 w-3" />
                  ) : (
                    <Play className="h-3 w-3" />
                  )}
                  {hasErrorBlock
                    ? t("common.retry")
                    : t("chat.continue", { defaultValue: "继续" })}
                </button>
              )}
            </span>

            {showTimestamp && (
              <span className="ml-auto flex items-center gap-1.5 text-[10px] text-muted-foreground">
                {message.model && (
                  <span className="max-w-32 truncate font-mono" title={message.model}>
                    {message.model}
                  </span>
                )}
                <span>{formatTime(message.timestamp)}</span>
              </span>
            )}
          </div>
        )}
      </div>
    </div>
  );
}, (prev, next) => {
  // The component subscribes to its store object by id, so re-renders are
  // already driven by reference changes — the memo only guards against
  // parent re-renders with identical props (O(1), no block hashing).
  return (
    prev.messageId === next.messageId &&
    prev.showStreamStatus === next.showStreamStatus
  );
});
