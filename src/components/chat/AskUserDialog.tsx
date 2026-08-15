/**
 * AskUserDialog — floating dialog for ask_user tool requests.
 *
 * The agent calls ask_user when it needs a human decision and then
 * blocks waiting for a reply. Without this dialog the user has no way
 * to respond, and the agent times out after 5 minutes — appearing hung.
 *
 * Shows:
 * - The agent's question
 * - One button per predefined option (click to reply instantly)
 * - A free-text input for a custom answer (Enter to send)
 *
 * Positioned above the chat input, matching PermissionDialog styling.
 */

import { useState, useEffect, useRef, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { HelpCircle, Send, Loader2, Check, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Kbd } from "@/components/ui/kbd";
import { cn } from "@/lib/utils";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useAppStore } from "@/stores/appStore";
import { useFocusTrap } from "@/hooks/useFocusTrap";

export function AskUserDialog() {
  const { t } = useTranslation();
  // Read both stores unconditionally (hooks must not be conditional); the
  // active one is chosen by the current app surface. Depwork sessions answer
  // their own requests — the ask-user event is routed by session_id in
  // useAskUserEvents.
  const codePending = useChatStore((s) => s.pendingAskUser);
  const depworkPending = useDepworkChatStore((s) => s.pendingAskUser);
  const codeRespond = useChatStore((s) => s.respondAskUser);
  const depworkRespond = useDepworkChatStore((s) => s.respondAskUser);
  const mode = useAppStore((s) => s.mode);
  const pending = mode === "depwork" ? depworkPending : codePending;
  const respondAskUser = mode === "depwork" ? depworkRespond : codeRespond;
  const [customAnswer, setCustomAnswer] = useState("");
  const [sending, setSending] = useState(false);
  // The option currently being sent — shows the selection feedback while
  // the reply is in flight (the dialog closes when the backend acks).
  const [selected, setSelected] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const dialogRef = useFocusTrap<HTMLDivElement>(!!pending);
  // 取消时回给 agent 的固定文案——让等待的回合立即继续，而不是干等 5 分钟。
  const CANCEL_TEXT = t("chat.askUserCancelText", "（用户已关闭弹窗，未作答）");

  // Focus the custom input when a new request arrives
  useEffect(() => {
    if (pending) {
      setCustomAnswer("");
      setSending(false);
      setSelected(null);
      setTimeout(() => textareaRef.current?.focus(), 50);
    }
  }, [pending]);

  const handleRespond = useCallback(
    async (answer: string) => {
      if (sending) return;
      setSending(true);
      setSelected(answer);
      try {
        await respondAskUser(answer);
      } catch {
        // invoke failure — keep the dialog open so the user can retry
        setSelected(null);
        setSending(false);
      }
    },
    [respondAskUser, sending],
  );

  const handleCancel = useCallback(() => {
    if (sending) return;
    void handleRespond(CANCEL_TEXT);
  }, [handleRespond, sending, CANCEL_TEXT]);

  // Esc = 暂不回答（与弹窗家族的 Esc 语义一致）。
  useEffect(() => {
    if (!pending) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !(e.target instanceof HTMLTextAreaElement)) {
        e.preventDefault();
        handleCancel();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pending, handleCancel]);

  if (!pending) return null;

  const handleKeyDown = (e: React.KeyboardEvent) => {
    // IME guard — Enter used to confirm a Chinese/Japanese composition
    // selection must not send a half-typed reply.
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      if (customAnswer.trim()) void handleRespond(customAnswer.trim());
    }
  };

  return (
    <>
      {/* ── Ask-user strip — anchored by ChatViewShell directly above the
          chat input (width matches the message column, centered). */}
      <div
        ref={dialogRef}
        className="relative z-40"
        role="dialog"
        aria-modal="true"
        aria-label="Ask user"
      >
        <div className="decision-card animate-in slide-in-from-bottom-3 fade-in duration-200">
          <div className="space-y-2.5 px-4 py-3">
            {/* ── Header ─────────────────────────────────────────── */}
            <div className="flex items-center gap-2.5">
              <div className="decision-icon">
                <HelpCircle className="h-4 w-4" />
              </div>
              <span className="text-xs font-semibold">{t("chat.askUserTitle", "需要你确认")}</span>
              {sending ? (
                <Loader2 className="ml-auto h-3.5 w-3.5 animate-spin text-muted-foreground" />
              ) : (
                <span className="ml-auto flex items-center gap-1.5 text-[10px] text-muted-foreground/70">
                  <span className="h-1.5 w-1.5 animate-pulse rounded-full bg-primary" />
                  {t("chat.askUserWaiting", "等待回复")}
                </span>
              )}
            </div>

            {/* ── Question — the card's focal content ────────────── */}
            <p className="text-[15px] leading-relaxed text-foreground">{pending.question}</p>

            {/* ── Options — full-width card rows (match AskUserCard's
                style); clicking sends instantly with a selected-state
                feedback while the reply is in flight. */}
            {pending.options.length > 0 && (
              <div className="space-y-1.5">
                {pending.options.map((opt, idx) => {
                  const isSelected = selected === opt;
                  return (
                    <button
                      key={idx}
                      disabled={sending && !isSelected}
                      onClick={() => void handleRespond(opt)}
                      className={cn(
                        "group flex w-full items-center gap-2.5 rounded-lg border px-3 py-2 text-left text-sm transition-colors",
                        isSelected
                          ? "border-primary/60 bg-primary/10 text-foreground"
                          : "border-border/70 bg-background/50 text-foreground/85 hover:border-primary/40 hover:bg-primary/5",
                      )}
                    >
                      <span
                        className={cn(
                          "flex h-5 w-5 shrink-0 items-center justify-center rounded-full text-[10px] font-medium transition-colors",
                          isSelected
                            ? "bg-primary text-primary-foreground"
                            : "bg-muted text-muted-foreground",
                        )}
                      >
                        {isSelected ? <Check className="h-3 w-3" /> : idx + 1}
                      </span>
                      <span className="min-w-0 flex-1">{opt}</span>
                      {isSelected && (
                        <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-primary" />
                      )}
                    </button>
                  );
                })}
              </div>
            )}

            {/* ── Custom answer input ──────────────────────────── */}
            <div className="flex items-end gap-2">
              <Textarea
                ref={textareaRef}
                value={customAnswer}
                onChange={(e) => setCustomAnswer(e.target.value)}
                onKeyDown={handleKeyDown}
                placeholder={t("chat.askUserPlaceholder", "输入你的回复，Enter 发送")}
                rows={1}
                disabled={sending}
                className="min-h-[36px] max-h-[120px] flex-1 resize-none border-border/70 text-sm focus-visible:ring-primary/30"
              />
              <Button
                size="icon"
                disabled={sending || !customAnswer.trim()}
                className="h-9 w-9 shrink-0 rounded-full"
                onClick={() => void handleRespond(customAnswer.trim())}
                aria-label="Send reply"
              >
                <Send className="h-3.5 w-3.5" />
              </Button>
            </div>
          </div>

          {/* ── Keyboard hint strip ───────────────────────────── */}
          <div className="decision-card-footer">
            <Button
              variant="ghost"
              size="sm"
              className="gap-1.5 text-[11px] text-muted-foreground hover:text-foreground"
              onClick={handleCancel}
              disabled={sending}
            >
              <X className="h-3 w-3" />
              {t("chat.askUserCancel", "暂不回答")}
            </Button>
            <div className="flex-1" />
            <span className="flex items-center gap-1 text-[10px] text-muted-foreground/60">
              {t("chat.askUserKbdHint", "Enter 发送")}
              <Kbd className="border-border/60 text-muted-foreground/60">↵</Kbd>
            </span>
          </div>
        </div>
      </div>
    </>
  );
}
