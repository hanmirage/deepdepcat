/**
 * TaskCompletedBanner — floating notification shown when a background bash
 * task finishes while the agent is idle (auto-wake signal).
 *
 * The backend emits `task-completed` when a background task exits. If the
 * event belongs to the current session and the agent is NOT streaming, this
 * banner appears above the input with:
 * - "Continue" → fills the input with a synthesized follow-up prompt (user
 *   can edit before sending) and sends it, turning a finished long task
 *   into an immediate next agent turn.
 * - Dismiss → closes the banner (the task result is still available via
 *   the task panel / wait_tasks).
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { CheckCircle2, XCircle, RefreshCw, X } from "lucide-react";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useStore } from "zustand";
import { cn } from "@/lib/utils";

interface TaskCompletedPayload {
  task_id: string;
  session_id: string;
  command: string;
  exit_code: number | null;
  status: string;
}

export interface TaskCompletedBannerProps {
  /** Which chat store this banner binds to. "code" → chatStore. */
  mode?: "code" | "depwork";
  className?: string;
}

export function TaskCompletedBanner({
  mode = "code",
  className,
}: TaskCompletedBannerProps) {
  const { t } = useTranslation();
  const isDepwork = mode === "depwork";
  const [completed, setCompleted] = useState<TaskCompletedPayload | null>(null);
  const lastTaskRef = useRef<string>("");

  const chatStore = isDepwork ? useDepworkChatStore : useChatStore;
  const sessionId = useStore(chatStore, (s) => s.currentSessionId);
  const isStreaming = useStore(chatStore, (s) => s.isStreaming);

  useTauriEvent<TaskCompletedPayload>("task-completed", (payload) => {
    // Only surface tasks from the current session, and deduplicate
    // re-delivered events for the same task.
    if (payload.session_id !== sessionId) return;
    if (payload.task_id === lastTaskRef.current) return;
    lastTaskRef.current = payload.task_id;
    setCompleted(payload);
  });

  const dismiss = useCallback(() => setCompleted(null), []);

  // Auto-hide after 30s of inaction — the banner must not squat on the
  // input area forever while the user reads the task results.
  useEffect(() => {
    if (!completed) return;
    const timer = setTimeout(dismiss, 30_000);
    return () => clearTimeout(timer);
  }, [completed, dismiss]);

  const continueTask = useCallback(() => {
    if (!completed) return;
    const command = completed.command.length > 120
      ? `${completed.command.slice(0, 120)}…`
      : completed.command;
    const synthesized = t("chat.taskContinuePrompt", {
      defaultValue: "[Background task completed] {command} exited with code {code}. Inspect the results and continue the work.",
      command,
      code: completed.exit_code ?? "?",
    });
    // Fill the input ONLY — the user reviews the synthesized prompt and
    // presses Enter. Auto-sending would fire an unread prompt (possibly in
    // the wrong language) with zero review.
    if (isDepwork) {
      useDepworkChatStore.getState().setInputText(synthesized);
    } else {
      useChatStore.getState().setInputText(synthesized);
    }
    setCompleted(null);
  }, [completed, isDepwork, t]);

  // Auto-hide while a new turn is streaming.
  const visible = completed !== null && !isStreaming;
  if (!visible || !completed) return null;

  const succeeded = completed.status === "completed";

  return (
    <div className={cn("flex items-center gap-2.5 rounded-lg border border-border/70 bg-card px-3 py-2 text-xs shadow-paper-md", className)}>
      {succeeded ? (
        <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-emerald-500/10">
          <CheckCircle2 className="h-3.5 w-3.5 text-emerald-500" />
        </span>
      ) : (
        <span className="flex h-5 w-5 shrink-0 items-center justify-center rounded-full bg-amber-500/10">
          <XCircle className="h-3.5 w-3.5 text-amber-500" />
        </span>
      )}
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium">
          {succeeded
            ? t("chat.taskCompleted", { defaultValue: "Background task completed" })
            : t("chat.taskFailed", { defaultValue: "Background task failed (code {{code}})", code: completed.exit_code ?? "?" })}
        </p>
        <p className="truncate font-mono text-[10px] text-muted-foreground">
          {completed.command}
        </p>
      </div>
      <button
        className="flex items-center gap-1 rounded-md bg-primary px-2 py-1 text-[10px] font-medium text-primary-foreground hover:bg-primary/90"
        onClick={continueTask}
        title={t("chat.taskContinueHint", { defaultValue: "填入输入框，确认后发送" })}
      >
        <RefreshCw className="h-3 w-3" />
        {t("chat.taskFillInput", { defaultValue: "继续" })}
      </button>
      <button
        className="rounded-md p-1 text-muted-foreground hover:bg-secondary/60 hover:text-foreground"
        onClick={dismiss}
        aria-label={t("common.dismiss", { defaultValue: "Dismiss" })}
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}
