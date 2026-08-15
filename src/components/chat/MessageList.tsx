/**
 * MessageList — renders the conversation messages with modern layout.
 *
 * Layout:
 * - User messages: right-aligned bubble with a light primary tint
 *   (user's product decision — the user message bubble sits on the RIGHT)
 * - Assistant messages: left-aligned narrative card with actions
 *
 * Virtualization:
 * - Short conversations (< VIRTUALIZE_THRESHOLD messages): plain full
 *   render with the classic follow-output logic (ResizeObserver + poll).
 * - Long conversations: react-virtuoso virtual list — only visible items
 *   are in the DOM. Streaming output is followed ONLY while the user is at
 *   the bottom; once the user scrolls up, auto-follow stops until they
 *   return to the bottom (ScrollToBottom button handles the jump back).
 */

import { useRef, useState, useEffect, useCallback, useMemo, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";
import { ScrollArea } from "@/components/ui/scroll-area";
import { PenLine } from "lucide-react";
import { Button } from "@/components/ui/button";
import { UserMessage } from "@/components/chat/UserMessage";
import { AssistantMessage } from "@/components/chat/AssistantMessage";
import { ScrollToBottom } from "@/components/chat/ScrollToBottom";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { cn, dayGroupLabel } from "@/lib/utils";
import { buildRefineDraft } from "@/lib/refineSelection";
import type { UIMessage } from "@/types";
import type { CompactionRecord } from "@/stores/chatStore/types";

/** Above this count the list switches to virtualization. */
const VIRTUALIZE_THRESHOLD = 200;

/** Distance from the bottom (px) treated as "at the bottom".
 *  Below this the list auto-follows streaming output; above it the
 *  back-to-bottom button appears. */
const BOTTOM_STICK_DISTANCE = 120;

/** Once the user scrolls away, they must return within this distance of
 *  the bottom before auto-follow re-engages (hysteresis — prevents the
 *  stream from yanking the viewport back while the user reads history). */
const BOTTOM_RE_STICK_DISTANCE = 20;

/** Update the stick flag with hysteresis: engage below the stick distance,
 *  but only re-engage once the user is near the bottom again. */
function updateStickState(stickRef: { current: boolean }, dist: number) {
  if (stickRef.current) {
    stickRef.current = dist < BOTTOM_STICK_DISTANCE;
  } else {
    stickRef.current = dist < BOTTOM_RE_STICK_DISTANCE;
  }
}

/** Height change (px) above which the follow scrolls smoothly instead of
 *  snapping — structural jumps (card expand/collapse, panel toggle). */
const SMOOTH_SCROLL_DELTA = 120;

/** Compaction happened during this window after a user message — the
 *  backend compacts the context when a new turn starts, so the record may
 *  land a few seconds after the message's own timestamp. */
const COMPACTION_WINDOW_MS = 10_000;

/** The compaction record that belongs to the turn starting at `msgTs`
 *  (latest record first); null when none. */
export function compactionForMessage(
  compactions: CompactionRecord[],
  prevTimestamp: number | undefined,
  msgTs: number,
): CompactionRecord | null {
  for (const record of compactions) {
    if (record.at >= (prevTimestamp ?? 0) && record.at <= msgTs + COMPACTION_WINDOW_MS) {
      return record;
    }
  }
  return null;
}

interface RefineAnchor {
  x: number;
  y: number;
  text: string;
}

/** Thin "today / yesterday / earlier" separator between conversation days. */
function DayDivider({ label }: { label: string }) {
  return (
    <div className="flex items-center gap-3 py-1.5">
      <div className="h-px flex-1 bg-border/60" />
      <span className="text-[10px] font-medium uppercase tracking-widest text-muted-foreground/50">
        {label}
      </span>
      <div className="h-px flex-1 bg-border/60" />
    </div>
  );
}

/** Thin "context compacted" separator — above the user message that
 *  started the turn which compressed the context. */
function CompactionDivider({ tokens }: { tokens: number }) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-3 py-1.5">
      <div className="h-px flex-1 bg-border/60" />
      <span className="text-[10px] font-medium text-muted-foreground/50">
        {t("chat.compactionDivider", {
          defaultValue: "上下文已压缩 · 节省 {{tokens}} tokens",
          tokens: tokens.toLocaleString(),
        })}
      </span>
      <div className="h-px flex-1 bg-border/60" />
    </div>
  );
}

export interface MessageListProps {
  messages: UIMessage[];
  /** Recent compaction records (context compressed at a turn start). */
  compactions?: CompactionRecord[];
  /** 定点修改：用户选中助手消息内容后，把引用草稿交回调用方（写入输入框）。 */
  onRefineSelection?: (draft: string) => void;
}

export function MessageList({ messages, compactions = [], onRefineSelection }: MessageListProps) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const virtuosoRef = useRef<VirtuosoHandle>(null);
  const refineButtonRef = useRef<HTMLDivElement>(null);
  const [showScrollBtn, setShowScrollBtn] = useState(false);
  const [refineAnchor, setRefineAnchor] = useState<RefineAnchor | null>(null);
  const stickToBottom = useRef(true);

  const uiMessages = messages;

  // Session restores replace the whole array — its last message must not
  // replay the enter animation. Track the first id: a change means the
  // conversation identity switched, so suppress animations this render.
  const firstIdRef = useRef<string | null>(null);
  const sessionRestored =
    firstIdRef.current !== null &&
    uiMessages[0]?.id !== firstIdRef.current;
  firstIdRef.current = uiMessages[0]?.id ?? null;

  // Id of the last assistant message — the only one that renders the live
  // turn-phase status line (one subscription, not one per message).
  const lastAssistantId = useMemo(() => {
    for (let i = uiMessages.length - 1; i >= 0; i--) {
      if (uiMessages[i].role === "assistant") return uiMessages[i].id;
    }
    return null;
  }, [uiMessages]);

  // ── 定点修改：选中助手消息内容 → 浮动按钮 ─────────────────────────
  useEffect(() => {
    const onMouseUp = (e: MouseEvent) => {
      if (refineButtonRef.current?.contains(e.target as Node)) return;
      const sel = window.getSelection();
      if (!sel || sel.isCollapsed || sel.rangeCount === 0) {
        setRefineAnchor(null);
        return;
      }
      const text = sel.toString().trim();
      if (text.length === 0) {
        setRefineAnchor(null);
        return;
      }
      const node = sel.anchorNode;
      const el = node instanceof Element ? node : node?.parentElement ?? null;
      const holder = el?.closest?.("[data-message-id]");
      if (!holder || holder.getAttribute("data-message-role") !== "assistant") {
        setRefineAnchor(null);
        return;
      }
      const rect = sel.getRangeAt(0).cloneRange().getBoundingClientRect();
      setRefineAnchor({
        x: rect.left + rect.width / 2,
        y: rect.bottom + 8,
        text,
      });
    };
    const dismiss = () => setRefineAnchor(null);
    document.addEventListener("mouseup", onMouseUp);
    document.addEventListener("scroll", dismiss, true);
    window.addEventListener("resize", dismiss);
    return () => {
      document.removeEventListener("mouseup", onMouseUp);
      document.removeEventListener("scroll", dismiss, true);
      window.removeEventListener("resize", dismiss);
    };
  }, []);

  const handleRefine = useCallback(() => {
    if (!refineAnchor || !onRefineSelection) return;
    onRefineSelection(buildRefineDraft(refineAnchor.text));
    setRefineAnchor(null);
    window.getSelection()?.removeAllRanges();
  }, [refineAnchor, onRefineSelection]);

  const refineButton =
    refineAnchor && onRefineSelection ? (
      <div
        ref={refineButtonRef}
        className="fixed z-50 -translate-x-1/2"
        style={{ left: refineAnchor.x, top: refineAnchor.y }}
      >
        <Button
          size="sm"
          variant="secondary"
          className="h-7 gap-1 border px-2 text-xs shadow-md"
          onClick={handleRefine}
          aria-label={t("chat.refineSelection")}
        >
          <PenLine className="h-3.5 w-3.5" />
          {t("chat.refineSelection")}
        </Button>
      </div>
    ) : null;

  const scrollToBottom = useCallback(() => {
    stickToBottom.current = true;
    setShowScrollBtn(false);
    if (virtuosoRef.current) {
      virtuosoRef.current.scrollToIndex({ index: messages.length - 1, behavior: "smooth" });
    } else if (scrollRef.current) {
      scrollRef.current.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" });
    }
  }, [messages.length]);

  const virtualized = uiMessages.length >= VIRTUALIZE_THRESHOLD;

  // ── Plain path (short conversations) ─────────────────────────
  // Three signals drive bottom-following:
  //  1. TOOL-BLOCK EVENTS — a new tool card entering the flow (the message
  //     list's structural change, not per-delta growth) smooth-scrolls so
  //     the card visibly "rolls in" (Claude-style dynamics). Historical
  //     messages are append-only — their tool counts never change — so the
  //     full-scan baseline is recomputed ONLY when a new message enters;
  //     per-delta work is just counting the last (streaming) message's
  //     blocks (O(blocks) instead of O(all blocks × messages)).
  //  2. ResizeObserver on the CONTENT container — fires when streaming
  //     appends text (content height grows). Small increments (per-delta
  //     text) snap instantly; big jumps (card expand/collapse, panel open)
  //     smooth-scroll. (Observing the scroll viewport would never fire: its
  //     box size doesn't change when content grows.)
  //  3. Native scroll events on the viewport (the ScrollArea ref points at
  //     the real scrolling element; scroll doesn't bubble, so it must be
  //     attached directly) — drive the stick flag + back-to-bottom button.
  //     Replaces the old 200ms poll: zero idle cost, instant response.
  const toolBaselineRef = useRef(0);
  const lastMsgIdRef = useRef<string | null>(null);
  const toolTotalRef = useRef(0);
  useEffect(() => {
    if (virtualized) return;
    const last = uiMessages[uiMessages.length - 1];
    if (!last) return;
    if (last.id !== lastMsgIdRef.current) {
      // New message entered the flow (send / session switch / clear) —
      // recompute the historical baseline once; cheap at this frequency.
      lastMsgIdRef.current = last.id;
      let baseline = 0;
      for (let i = 0; i < uiMessages.length - 1; i++) {
        const blocks = uiMessages[i].blocks;
        for (let j = 0; j < blocks.length; j++) {
          if (blocks[j].type === "tool_call") baseline++;
        }
      }
      toolBaselineRef.current = baseline;
    }
// Streaming: count only the last (streaming) message's blocks.
    let lastCount = 0;
    const blocks = last.blocks;
    for (let i = 0; i < blocks.length; i++) {
      if (blocks[i].type === "tool_call") lastCount++;
    }
    const total = toolBaselineRef.current + lastCount;
    if (total !== toolTotalRef.current) {
      toolTotalRef.current = total;
      if (stickToBottom.current && scrollRef.current) {
        scrollRef.current.scrollTo({
          top: scrollRef.current.scrollHeight,
          behavior: "smooth",
        });
      }
    }
  }, [uiMessages, virtualized]);

  // Stream-follow + stick-state updates from content growth.
  useEffect(() => {
    if (virtualized) return;
    const viewport = scrollRef.current;
    const content = contentRef.current;
    if (!viewport || !content) return;
    let lastHeight = content.scrollHeight;
    const updateStick = () => {
      const dist = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight;
      updateStickState(stickToBottom, dist);
      setShowScrollBtn(dist > BOTTOM_STICK_DISTANCE);
    };
    const ro = new ResizeObserver(() => {
      if (!stickToBottom.current) return;
      const height = content.scrollHeight;
      const delta = height - lastHeight;
      lastHeight = height;
      if (delta > SMOOTH_SCROLL_DELTA) {
        // Big structural jump (card expand/collapse, panel toggle) — roll.
        viewport.scrollTo({ top: viewport.scrollHeight, behavior: "smooth" });
      } else {
        // Streaming increments — snap (invisible: content grows at the bottom).
        viewport.scrollTop = viewport.scrollHeight;
      }
      updateStick();
    });
    // Observe the content box so height growth (streaming) triggers follow.
    ro.observe(content);
    return () => ro.disconnect();
  }, [virtualized]);

  // User-scroll detector — native scroll on the viewport (event-driven,
  // replaces the legacy 200ms poll: no idle wakeups, no up-to-200ms lag).
  useEffect(() => {
    if (virtualized) return;
    const el = scrollRef.current;
    if (!el) return;
    const update = () => {
      const dist = el.scrollHeight - el.scrollTop - el.clientHeight;
      updateStickState(stickToBottom, dist);
      setShowScrollBtn(dist > BOTTOM_STICK_DISTANCE);
    };
    el.addEventListener("scroll", update, { passive: true });
    // Initial alignment — the poll's first round used to do this on mount.
    update();
    return () => el.removeEventListener("scroll", update);
  }, [virtualized]);

  if (virtualized) {
    return (
      <div className="relative min-h-0 flex-1">
        <Virtuoso
          ref={virtuosoRef}
          className="h-full"
          data={uiMessages}
          computeItemKey={(_, msg) => msg.id}
          // Start at the latest message.
          initialTopMostItemIndex={Math.max(0, uiMessages.length - 1)}
          // Follow streaming output ONLY while the user is at the bottom.
          // "auto" = instant snap-follow: each content patch jumps the list
          // to the bottom WITHOUT an animation. "smooth" used to get
          // interrupted every frame by the next delta — visible jitter on
          // fast streams. The ScrollToBottom button keeps its own smooth
          // jump (user-driven, not stream-driven).
          followOutput={(isAtBottom) => (isAtBottom ? "auto" : false)}
          atBottomStateChange={(atBottom) => {
            stickToBottom.current = atBottom;
            setShowScrollBtn(!atBottom);
          }}
          itemContent={(index, msg) => {
            const isNewest = msg.id === uiMessages[uiMessages.length - 1]?.id;
            const newDay =
              index === 0 ||
              dayGroupLabel(uiMessages[index - 1].timestamp) !== dayGroupLabel(msg.timestamp);
            const compacted =
              msg.role === "user"
                ? compactionForMessage(
                    compactions,
                    index > 0 ? uiMessages[index - 1]?.timestamp : undefined,
                    msg.timestamp,
                  )
                : null;
            return (
              <div
                data-message-id={msg.id}
                data-message-role={msg.role}
                className={cn(
                  "mx-auto max-w-[840px] px-4 pb-2",
                  index > 0 && uiMessages[index - 1].role !== msg.role ? "pt-6" : "pt-2",
                  isNewest && !sessionRestored && "message-enter",
                )}
              >
                {newDay && (
                  <DayDivider
                    label={
                      dayGroupLabel(msg.timestamp) === "today"
                        ? t("chat.today")
                        : dayGroupLabel(msg.timestamp) === "yesterday"
                          ? t("chat.yesterday")
                          : t("chat.earlier")
                    }
                  />
                )}
                {compacted && <CompactionDivider tokens={compacted.tokens} />}
                <ErrorBoundary resetKey={msg.id}>
                  {msg.role === "user" ? (
                    <div className="flex justify-end">
                      <UserMessage messageId={msg.id} />
                    </div>
                  ) : (
                    <div className="flex justify-start">
                      <AssistantMessage
                        messageId={msg.id}
                        showStreamStatus={msg.id === lastAssistantId}
                      />
                    </div>
                  )}
                </ErrorBoundary>
              </div>
            );
          }}
          components={{
            // Match the plain path's py-4: 16px air above the first item
            // and below the last one (space-y-6 gives the 24px gaps).
            Header: () => <div className="h-4" />,
            Footer: () => <div className="h-4" />,
          }}
        />
        {refineButton}
        <ScrollToBottom visible={showScrollBtn} onClick={scrollToBottom} />
      </div>
    );
  }

  return (
    <div className="relative min-h-0 flex-1">
      <ScrollArea
        ref={scrollRef as RefObject<HTMLDivElement>}
        className="h-full"
      >
        <div className="mx-auto max-w-[840px] px-4 py-4" ref={contentRef}>
          {uiMessages.map((msg, i) => {
            const newDay =
              i === 0 ||
              dayGroupLabel(uiMessages[i - 1].timestamp) !== dayGroupLabel(msg.timestamp);
            const compacted =
              msg.role === "user"
                ? compactionForMessage(
                    compactions,
                    i > 0 ? uiMessages[i - 1]?.timestamp : undefined,
                    msg.timestamp,
                  )
                : null;
            return (
              <div key={msg.id}>
                {newDay && (
                  <DayDivider
                    label={
                      dayGroupLabel(msg.timestamp) === "today"
                        ? t("chat.today")
                        : dayGroupLabel(msg.timestamp) === "yesterday"
                          ? t("chat.yesterday")
                          : t("chat.earlier")
                    }
                  />
                )}
                {compacted && <CompactionDivider tokens={compacted.tokens} />}
                <div
                  data-message-id={msg.id}
                  data-message-role={msg.role}
                  // Only the LATEST message animates in — history loads
                  // (session switch / restore) enter silently instead of
                  // replaying a batch slide-in, and streamed appends keep a
                  // single focal entry point. Existing DOM nodes keep their
                  // key, so streaming updates never replay the animation.
                  className={cn(
                    i === uiMessages.length - 1 && !sessionRestored && "message-enter",
                    msg.role === "user" ? "flex justify-end" : "flex justify-start",
                    // Turn-boundary rhythm: a role switch gets a big gap, a
                    // same-role continuation (e.g. a second assistant block)
                    // stays tight so one reply reads as one unit.
                    i > 0 && uiMessages[i - 1].role !== msg.role ? "mt-6" : "mt-2",
                    // Long-history scroll perf: skip off-screen rendering for
                    // anything but the tail (which is what streaming touches).
                    i < uiMessages.length - 3 && "message-virtualized",
                  )}
                >
                  <ErrorBoundary resetKey={msg.id}>
                    {msg.role === "user" ? (
                      <UserMessage messageId={msg.id} />
                    ) : (
                      <AssistantMessage
                        messageId={msg.id}
                        showStreamStatus={msg.id === lastAssistantId}
                      />
                    )}
                  </ErrorBoundary>
                </div>
              </div>
            );
          })}
        </div>
      </ScrollArea>

      {refineButton}

      {/* Back-to-bottom — sits just above the input bar; appears only when
          the user has scrolled away from the latest message. */}
      <ScrollToBottom visible={showScrollBtn} onClick={scrollToBottom} />
    </div>
  );
}


