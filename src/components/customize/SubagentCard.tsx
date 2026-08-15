/**
 * SubagentCard — one dispatched subagent inside the single "子代理" pane.
 *
 * Event-driven (chatStore.subagents): type badge + specialist mark, task,
 * and live turn progress + last message while RUNNING. A done/failed card
 * auto-collapses to a one-line header with an expand toggle for the full
 * result (user-confirmed behavior). Reuses toolNarrative label/summary
 * helpers and the activity card's elapsed formatter.
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Bot,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  ClipboardList,
  Loader2,
  Search,
  XCircle,
} from "lucide-react";
import type { SubagentUIRecord } from "@/types";
import {
  agentTypeLabelKey,
  isCustomSpecialist,
  summarizeAgentTask,
} from "@/config/toolNarrative";
import { formatElapsed } from "./AgentActivityCard";

const AGENT_ICON: Record<string, typeof Bot> = {
  explore: Search,
  plan: ClipboardList,
  general: Bot,
};

export function SubagentCard({ subagent }: { subagent: SubagentUIRecord }) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const Icon = AGENT_ICON[subagent.agent_type] ?? Bot;
  const running = subagent.status === "running";
  const failed = subagent.status === "failed";
  const typeLabel =
    subagent.agent_type === "general"
      ? null
      : t(agentTypeLabelKey(subagent.agent_type));
  const hasResult = Boolean(subagent.result);
  // Done/failed cards collapse to the header line; running ones show live
  // progress; the result only reveals when explicitly expanded.
  const showBody = running || expanded;

  return (
    <div className="rounded-md border border-border/60 bg-background/60">
      <div className="flex items-center gap-1.5 px-2 py-1.5">
        <span className="shrink-0">
          {running ? (
            <Loader2 className="h-3 w-3 animate-spin text-primary" />
          ) : failed ? (
            <XCircle className="h-3 w-3 text-destructive" />
          ) : (
            <CheckCircle2 className="h-3 w-3 text-green-500" />
          )}
        </span>
        <Icon className="h-3 w-3 shrink-0 text-muted-foreground/60" />
        {typeLabel && (
          <span className="shrink-0 rounded bg-muted px-1 py-px text-[9px] font-medium text-muted-foreground/80">
            {typeLabel}
          </span>
        )}
        {isCustomSpecialist(subagent.agent_type) && (
          <span className="shrink-0 rounded bg-primary/10 px-1 py-px text-[9px] font-medium text-primary">
            {t("chat.specialistBadge", { defaultValue: "专家" })}
          </span>
        )}
        <span className="min-w-0 flex-1 truncate text-[11px] font-medium">
          {subagent.task
            ? summarizeAgentTask(subagent.task, 60)
            : t("activity.untitledTask")}
        </span>
        <span className="shrink-0 text-[10px]">
          {running
            ? t("subagents.running")
            : failed
              ? t("subagents.failed")
              : t("subagents.done")}
        </span>
        <span className="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground">
          {formatElapsed(subagent.startedAt)}
        </span>
        {!running && hasResult && (
          <button
            type="button"
            onClick={() => setExpanded((v) => !v)}
            className="shrink-0 rounded p-0.5 text-muted-foreground/60 transition-colors hover:bg-muted hover:text-foreground"
            title={expanded ? t("subagents.hideResult") : t("subagents.viewResult")}
            aria-label={expanded ? t("subagents.hideResult") : t("subagents.viewResult")}
          >
            {expanded ? (
              <ChevronDown className="h-3 w-3" />
            ) : (
              <ChevronRight className="h-3 w-3" />
            )}
          </button>
        )}
      </div>

      {showBody && (
        <div className="space-y-0.5 px-2 pb-1.5 pl-6">
          {running ? (
            <>
              {subagent.total_turns > 0 && (
                <span className="text-[10px] text-muted-foreground/70">
                  {t("subagents.turn", { turn: subagent.turn, total: subagent.total_turns })}
                </span>
              )}
              {subagent.lastMessage && (
                <p className="truncate text-[10px] text-muted-foreground">
                  {subagent.lastMessage}
                </p>
              )}
            </>
          ) : (
            expanded &&
            hasResult && (
              <pre className="whitespace-pre-wrap rounded bg-muted/40 p-1.5 text-[10px] leading-relaxed text-foreground/80">
                {subagent.result}
              </pre>
            )
          )}
        </div>
      )}
    </div>
  );
}
