/**
 * AskUserCard — interactive card for ask_user tool calls.
 *
 * When the agent calls ask_user, it's waiting for a human decision.
 * This card highlights that clearly so the user knows:
 * 1. What the question is
 * 2. That they need to reply (not just scroll past)
 * 3. Where to reply (the input box at the bottom)
 *
 * The card auto-expands so the question and options are always visible.
 * While the ask is pending, clicking an option fills the input box with it
 * (the user confirms with Enter) — the reply path stays the input bar,
 * but options no longer need to be typed by hand.
 *
 * Design: part of the decision-card family (see AskUserDialog /
 * PermissionDialog) — neutral card + primary accent, NOT a warning.
 * Amber is reserved for real failures.
 */

import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { AlertCircle, Loader2, CornerDownLeft } from "lucide-react";
import { cn } from "@/lib/utils";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useStore } from "zustand";
import { useAppStore } from "@/stores/appStore";
import type { ToolCallState } from "@/types";

interface AskUserCardProps {
  tool: ToolCallState;
}

/** Parse ask_user arguments. Returns { question, options } or null. */
function parseAskUserArgs(args: string): { question: string; options: string[] } | null {
  try {
    const parsed = JSON.parse(args);
    if (typeof parsed.question !== "string") return null;
    const options = Array.isArray(parsed.options) ? parsed.options : [];
    return { question: parsed.question, options };
  } catch {
    return null;
  }
}

export function AskUserCard({ tool }: AskUserCardProps) {
  const { t } = useTranslation();
  const isRunning = tool.status === "running";
  const isPending = isRunning && !tool.result;
  // The option click fills the ACTIVE mode's input (code / depwork).
  const appMode = useAppStore((s) => s.mode);
  const chatStore = appMode === "depwork" ? useDepworkChatStore : useChatStore;
  const setInputText = useStore(chatStore, (s) => s.setInputText);
  const parsed = useMemo(() => parseAskUserArgs(tool.arguments), [tool.arguments]);

  if (!parsed) {
    // Fallback: just show it as a regular pending tool
    return (
      <div className="flex items-center gap-2 rounded-lg border border-border/70 bg-muted/30 px-3 py-2">
        <AlertCircle className="h-4 w-4 text-primary" />
        <span className="text-sm text-muted-foreground">
          {t("chat.needsConfirmation", { defaultValue: "需要你的确认" })}
        </span>
        {isRunning && <Loader2 className="h-4 w-4 animate-spin text-primary" />}
      </div>
    );
  }

  return (
    <div
      className={cn(
        "rounded-lg border bg-card/80 p-3",
        isPending
          ? "border-primary/40 shadow-paper-md animate-in fade-in"
          : "border-border/70",
      )}
    >
      {/* Header — primary accent, decision not warning */}
      <div className="mb-2 flex items-center gap-2">
        <AlertCircle className="h-4 w-4 text-primary" />
        <span className="text-sm font-medium text-foreground">
          {t("chat.askUserTitle")}
        </span>
        {isRunning && <Loader2 className="h-4 w-4 animate-spin text-primary ml-auto" />}
      </div>

      {/* Question */}
      <p className="mb-3 text-sm leading-relaxed text-foreground">
        {parsed.question}
      </p>

      {/* Options — clickable while pending: fills the input, user sends with
          Enter. Read-only in history (the ask is already answered). */}
      {parsed.options.length > 0 && (
        <div className="mb-2 space-y-1">
          <p className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
            {t("chat.options", { defaultValue: "选项" })}
          </p>
          {parsed.options.map((opt, idx) => (
            <button
              key={idx}
              disabled={!isPending}
              onClick={() => setInputText(opt)}
              title={isPending ? t("chat.optionFillHint", { defaultValue: "点击填入输入框，回车发送" }) : undefined}
              className={cn(
                "flex w-full items-center gap-2 rounded-md border px-3 py-1.5 text-sm transition-colors",
                isPending
                  ? "border-border/60 bg-background/60 text-foreground/80 hover:border-primary/50 hover:bg-primary/5 hover:text-foreground"
                  : "cursor-default border-border/40 bg-background/40 text-muted-foreground",
              )}
            >
              <span className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-primary/10 text-[10px] text-primary">
                {idx + 1}
              </span>
              <span className="flex-1 text-left">{opt}</span>
              {isPending && (
                <CornerDownLeft className="h-3 w-3 shrink-0 text-muted-foreground/40" />
              )}
            </button>
          ))}
        </div>
      )}

      {/* Result — shown after user responds (the pending state needs no
          extra prompt row: the header already says 需要你确认 and the
          options carry their own click affordance). */}
      {tool.result && (
        <div className="mt-2 rounded-md bg-muted/20 px-2.5 py-1.5 text-xs text-muted-foreground">
          <span className="font-medium">{t("chat.yourReply", { defaultValue: "你的回复：" })}</span>
          {tool.result}
        </div>
      )}
    </div>
  );
}
