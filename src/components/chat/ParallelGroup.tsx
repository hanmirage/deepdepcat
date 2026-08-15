/**
 * ParallelGroup — consecutive non-read tools that ran in the SAME concurrent
 * batch collapse into one row (Claude-style "N 个工具并行执行"). The header
 * carries an aggregate status; expanding reveals each member tool as its own
 * bare ToolCallCard line (each keeps its own details / result / elapsed).
 *
 * Grouping happens in segments.ts: only adjacent tool rows sharing a
 * `parallelBatch` id (assigned by the workspace reducer when their runs
 * overlap) fold together — a lone tool renders as its own plain row.
 */

import { useState } from "react";
import * as Collapsible from "@radix-ui/react-collapsible";
import { Loader2, XCircle, Check, Layers } from "lucide-react";
import { useTranslation } from "react-i18next";
import type { ToolCallState } from "@/types";
import { cn } from "@/lib/utils";
import { ToolCallCard } from "@/components/chat/ToolCallCard";

export function ParallelGroup({ tools }: { tools: ToolCallState[] }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  const isRunning = tools.some((tool) => tool.status === "running");
  const errorCount = tools.filter((tool) => tool.status === "error").length;
  const count = tools.length;

  return (
    <Collapsible.Root open={open} onOpenChange={setOpen} className="w-full">
      <Collapsible.Trigger asChild>
        <button
          className="group flex w-full items-center gap-2 py-0.5 text-left transition-colors hover:bg-muted/20"
          aria-expanded={open}
          aria-label={t("toolCall.parallelAriaLabel", { count })}
        >
          {/* Status — running spins with a breathing glow; done is a hairline
              check (matches the bare ToolCallCard state language). */}
          <span className={cn("flex w-3.5 shrink-0 justify-center", isRunning && "animate-glow-pulse")}>
            {isRunning ? (
              <Loader2 className="h-3 w-3 animate-spin text-primary" />
            ) : errorCount > 0 ? (
              <XCircle className="h-3 w-3 text-destructive" />
            ) : (
              <Check className="h-2.5 w-2.5 text-muted-foreground/40" strokeWidth={2.5} />
            )}
          </span>

          <Layers className="h-3 w-3 shrink-0 text-muted-foreground/60" />

          {/* Aggregate verb — running / done / error with counts. */}
          <span className="min-w-0 flex-1 truncate text-[11px] font-medium text-foreground/75">
            {isRunning
              ? t("toolCall.parallelRunning", { count })
              : errorCount > 0
                ? t("toolCall.parallelError", { count, errors: errorCount })
                : t("toolCall.parallelDone", { count })}
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
