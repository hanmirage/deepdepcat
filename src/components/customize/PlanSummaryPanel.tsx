/**
 * PlanSummaryPanel — the right-panel "plan" pane.
 *
 * Shows plan-mode status + pending interactions, and — from plan approval
 * through execution — the FULL approved plan Markdown (retained in
 * planStore.currentPlan until the run ends and the backend archives it).
 */

import { useTranslation } from "react-i18next";
import { ClipboardList, Clock3, FileText, Inbox } from "lucide-react";
import {
  useCurrentPlan,
  useCurrentPlanApproval,
  useIsSessionInPlanMode,
  usePendingInteractions,
} from "@/stores/planStore";
import { MarkdownRenderer } from "@/components/chat/MarkdownRenderer";
import { cn } from "@/lib/utils";

export function PlanSummaryPanel({
  sessionId,
}: {
  sessionId: string | null | undefined;
}) {
  const { t } = useTranslation();
  const inPlanMode = useIsSessionInPlanMode(sessionId);
  const interactions = usePendingInteractions(sessionId);
  const approval = useCurrentPlanApproval();
  const currentPlan = useCurrentPlan();
  const livePlan =
    currentPlan && currentPlan.sessionId === sessionId ? currentPlan.plan : null;
  // Gate the parked approval by session too — a code session's request must
  // never render inside depwork's plan pane (and vice versa).
  const liveApproval =
    approval && approval.session_id === sessionId ? approval : null;

  return (
    <div className="space-y-3">
      <div
        className={cn(
          "rounded-md border px-3 py-2.5",
          inPlanMode
            ? "border-amber-500/30 bg-amber-500/5"
            : "border-border/60 bg-muted/20",
        )}
      >
        <div className="flex items-center gap-1.5 text-xs font-medium">
          <ClipboardList className="h-3.5 w-3.5 shrink-0" />
          {t("rightPanel.planModeActive")}
        </div>
        <p className="mt-1 text-[11px] text-muted-foreground">
          {inPlanMode ? t("rightPanel.planModeActiveDesc") : t("rightPanel.planModeIdle")}
        </p>
      </div>

      {livePlan && (
        <div className="rounded-md border border-border/60 bg-background/60">
          <div className="flex items-center gap-1.5 border-b border-border/60 px-3 py-1.5">
            <FileText className="h-3 w-3 shrink-0 text-muted-foreground" />
            <span className="text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
              {t("rightPanel.planView")}
            </span>
          </div>
          <div className="max-h-[min(60vh,480px)] overflow-auto p-2.5">
            <MarkdownRenderer content={livePlan} />
          </div>
        </div>
      )}

      {liveApproval && (
        <p className="rounded-md border border-primary/20 bg-primary/5 px-3 py-2 text-[11px] text-foreground/80">
          {t("rightPanel.planWaitingApproval")}
        </p>
      )}

      {interactions.length > 0 && (
        <div>
          <h4 className="mb-1 flex items-center gap-1 text-[10px] font-semibold uppercase tracking-wide text-muted-foreground">
            <Clock3 className="h-3 w-3" />
            {t("rightPanel.planInteractions")}
          </h4>
          <ul className="space-y-1">
            {interactions.map((it) => (
              <li
                key={it.request_id}
                className="flex items-center gap-1.5 rounded-md border border-border/60 px-2 py-1.5 text-[11px]"
              >
                <span className="min-w-0 flex-1 truncate">{it.summary}</span>
              </li>
            ))}
          </ul>
        </div>
      )}

      {!inPlanMode && !liveApproval && interactions.length === 0 && (
        <p className="flex items-center gap-1.5 rounded-md border border-border/60 bg-muted/20 px-3 py-4 text-center text-[11px] text-muted-foreground">
          <Inbox className="h-3.5 w-3.5 shrink-0" />
          {t("rightPanel.planEmpty")}
        </p>
      )}
    </div>
  );
}
