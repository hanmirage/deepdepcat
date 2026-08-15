/**
 * ReadGroup — consecutive read-only tool calls collapse into one row
 * (Claude/opencode-style: "✓ 已读取 5 项"). The aggregate verb uses the
 * same VerbSwap cross-fade as tool lines; expanding reveals each member
 * tool as its own bare line (ToolCallCard), which keeps its own details.
 *
 * Grouping happens in AssistantMessage (consecutive read tools only —
 * a write tool or text between reads splits the group).
 */

import { useState } from "react";
import * as Collapsible from "@radix-ui/react-collapsible";
import { Loader2, XCircle, Check, Search } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ToolCallState } from "@/types";
import { cn } from "@/lib/utils";
import { ToolCallCard } from "@/components/chat/ToolCallCard";
import { VerbSwap } from "@/components/chat/VerbSwap";

export function ReadGroup({ tools }: { tools: ToolCallState[] }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const isRunning = tools.some((tool) => tool.status === "running");
  const errorCount = tools.filter((tool) => tool.status === "error").length;
  const count = tools.length;

  const doneVerb =
    errorCount > 0
      ? t("toolCall.readGroupError", { count, errors: errorCount })
      : t("toolCall.readGroupDone", { count });

  return (
    <Collapsible.Root
      open={open}
      // Groups stay expandable while a member runs — each member row keeps
      // its own per-tool details lock, so peeking at which file is being
      // read is safe (unlike tool lines, whose args stream mid-run).
      onOpenChange={setOpen}
      className="w-full"
    >
      <Collapsible.Trigger asChild>
        <button
          className="group flex w-full items-center gap-2 py-0.5 text-left transition-colors hover:bg-muted/20"
          aria-expanded={open}
          aria-label="Read group"
        >
          {/* Status */}
          <span className={cn("flex w-3.5 shrink-0 justify-center", isRunning && "animate-glow-pulse")}>
            {isRunning ? (
              <Loader2 className="h-3 w-3 animate-spin text-primary" />
            ) : errorCount > 0 ? (
              <XCircle className="h-3 w-3 text-destructive" />
            ) : (
              <Check className="h-2.5 w-2.5 text-muted-foreground/40" strokeWidth={2.5} />
            )}
          </span>

          <Search className="h-3 w-3 shrink-0 text-muted-foreground/60" />

          {/* Aggregate verb — cross-fades running → done like tool verbs. */}
          <span className="flex min-w-0 flex-1 items-baseline gap-1.5">
            <VerbSwap
              activeText={t("toolCall.readGroupRunning", { count })}
              doneText={doneVerb}
              active={isRunning}
              className={cn(
                "shrink-0 text-[11px] font-medium",
                errorCount > 0 ? "text-destructive/90" : "text-foreground/75",
              )}
            />
          </span>

          {/* Expand chevron — terminal-prompt `>` matching the tool row. */}
          <span
            className="shrink-0 font-mono text-[11px] font-semibold text-muted-foreground/40 transition-colors group-hover:text-foreground"
            aria-hidden="true"
          >
            <span className={cn("inline-block transition-transform duration-200", open && "rotate-90")}>
              &gt;
            </span>
          </span>
        </button>
      </Collapsible.Trigger>

      {/* Expanded member rows — each keeps its own bare line + details. */}
      <Collapsible.Content>
        <div className="tool-expand-enter mt-1.5 space-y-0.5 rounded-lg border border-border/40 bg-muted/10 p-1.5 pl-2.5">
          {tools.map((tool) => (
            <ToolCallCard key={tool.id} tool={tool} />
          ))}
        </div>
      </Collapsible.Content>
    </Collapsible.Root>
  );
}
