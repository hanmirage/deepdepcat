/**
 * PlanApprovalPanel — floating panel that rises above the chat input when
 * the agent submits a plan via `exit_plan_mode`.
 *
 * Shows:
 * - The plan text (verbatim, scrollable)
 * - The workspace git-change summary collected at submission time
 * - Actions: Approve & start coding / Request changes (with feedback)
 *
 * Approving exits plan mode and the agent continues implementing; rejecting
 * with feedback keeps the agent in plan mode to revise and re-submit.
 */

import { useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import {
  CheckCircle2,
  MessageSquareWarning,
  ClipboardList,
  FileText,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { MarkdownRenderer } from "@/components/chat/MarkdownRenderer";
import { useCurrentPlanApproval, usePlanStore } from "@/stores/planStore";
import { useFocusTrap } from "@/hooks/useFocusTrap";
import { isEditableKeyEvent } from "@/lib/utils";

export function PlanApprovalPanel() {
  const { t } = useTranslation();
  const approval = useCurrentPlanApproval();
  const respond = usePlanStore((s) => s.respond);
  const [rejecting, setRejecting] = useState(false);
  const [feedback, setFeedback] = useState("");

  // Reset local state when a new plan arrives.
  useEffect(() => {
    setRejecting(false);
    setFeedback("");
  }, [approval?.request_id]);

  // Focus trap + restore for the modal panel (aria-modal but a plain div).
  const dialogRef = useFocusTrap<HTMLDivElement>(!!approval);

  // Keyboard: Enter approves (when not rejecting), Escape cancels the panel.
  // Guarded — Enter typed inside the feedback textarea (or during IME
  // composition) is typing, not an approval. Enter pressed while a button
  // has focus is the button's own activation (native), so it must never be
  // hijacked into an approval — "Request changes" with focus would otherwise
  // approve the plan instead of clicking the button.
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (!approval) return;
      if (isEditableKeyEvent(e)) return;
      const target = e.target;
      if (target instanceof HTMLElement && target.closest("button, a, [role='button']")) {
        return;
      }
      if (e.key === "Enter" && !e.shiftKey && !rejecting) {
        e.preventDefault();
        void respond("approve");
      } else if (e.key === "Escape") {
        e.preventDefault();
        if (rejecting) {
          // Collapse the feedback row first; a second Esc rejects outright.
          setRejecting(false);
          setFeedback("");
        } else {
          void respond("reject");
        }
      }
    },
    [approval, rejecting, respond],
  );

  useEffect(() => {
    if (!approval) return;
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [approval, handleKeyDown]);

  if (!approval) return null;

  const sendFeedback = () => {
    void respond("reject", feedback.trim() || undefined);
  };

  return (
    <>
      {/* ── Panel — anchored by ChatViewShell directly above the chat input
          (no fixed offsets, so it can never cover or be covered by the
          input as its height changes). ── */}
      <div
        ref={dialogRef}
        className="relative z-40"
        role="dialog"
        aria-modal="true"
        aria-label={t("planApproval.title")}
      >
        <div className="decision-card animate-in slide-in-from-bottom-2 fade-in duration-200">
          {/* ── Header ─────────────────────────────────────────── */}
          <div className="flex items-center gap-2.5 border-b border-border/60 px-4 py-2.5">
            <div className="decision-icon">
              <ClipboardList className="h-4 w-4" />
            </div>
            <span className="text-xs font-semibold">{t("planApproval.title")}</span>
            <span className="ml-auto shrink-0 rounded bg-muted px-1.5 py-0.5 text-[9px] font-medium uppercase tracking-wider text-muted-foreground">
              {t("planApproval.subtitle")}
            </span>
          </div>

          {/* ── Plan content — rendered as markdown so code blocks in the
              plan get full syntax highlighting (was a colorless <pre>). */}
          <div className="space-y-3 px-4 py-3">
            <div className="flex items-start gap-2">
              <FileText className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              <div className="max-h-64 min-h-20 flex-1 overflow-auto rounded-md border border-border bg-muted/40 p-2.5">
                <MarkdownRenderer content={approval.plan} />
              </div>
            </div>

            {/* ── Workspace changes (diff summary) ─────────────── */}
            {approval.changed_files && approval.changed_files.length > 0 && (
              <div>
                <p className="mb-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                  {t("planApproval.changedFiles")}
                </p>
                <div className="flex flex-wrap gap-1">
                  {approval.changed_files.slice(0, 24).map((f) => (
                    <span
                      key={f}
                      className="rounded border border-border bg-muted/40 px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground"
                    >
                      {f}
                    </span>
                  ))}
                </div>
              </div>
            )}

            {/* ── Reject feedback ──────────────────────────────── */}
            {rejecting && (
              <div className="space-y-2">
                <textarea
                  autoFocus
                  value={feedback}
                  onChange={(e) => setFeedback(e.target.value)}
                  placeholder={t("planApproval.feedbackPlaceholder")}
                  rows={3}
                  className="w-full resize-none rounded-md border border-border bg-background/60 p-2 text-xs text-foreground placeholder:text-muted-foreground focus:border-primary focus:outline-none"
                />
                <div className="flex justify-end gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    className="gap-1.5 text-xs"
                    onClick={() => setRejecting(false)}
                  >
                    <X className="h-3.5 w-3.5" />
                    {t("common.cancel")}
                  </Button>
                  <Button
                    size="sm"
                    className="gap-1.5 text-xs"
                    onClick={sendFeedback}
                  >
                    <MessageSquareWarning className="h-3.5 w-3.5" />
                    {t("planApproval.sendFeedback")}
                  </Button>
                </div>
              </div>
            )}
          </div>

          {/* ── Actions ────────────────────────────────────────── */}
          <div className="flex items-center gap-2 border-t border-border/60 px-4 py-2.5">
            {!rejecting && (
              <Button
                variant="outline"
                size="sm"
                className="gap-1.5 text-xs text-amber-600 hover:bg-amber-500/10 hover:text-amber-600"
                onClick={() => setRejecting(true)}
              >
                <MessageSquareWarning className="h-3.5 w-3.5" />
                {t("planApproval.reject")}
              </Button>
            )}
            <div className="flex-1" />
            <Button
              size="sm"
              className="gap-1.5 text-xs"
              onClick={() => void respond("approve")}
              autoFocus={
                !rejecting &&
                (!document.activeElement || document.activeElement === document.body)
              }
            >
              <CheckCircle2 className="h-3.5 w-3.5" />
              {t("planApproval.approve")}
              {!rejecting && (
                <Kbd className="border-primary-foreground/30 text-primary-foreground/70">↵</Kbd>
              )}
            </Button>
          </div>
        </div>
      </div>
    </>
  );
}
