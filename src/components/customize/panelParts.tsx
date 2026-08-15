/**
 * panelParts — shared building blocks for right-panel pages.
 */

import type { ReactNode } from "react";
import type { LucideIcon } from "lucide-react";

/** One row of page chrome: icon + label + optional count + trailing action. */
export function SectionHeader({
  icon: Icon,
  label,
  count,
  action,
}: {
  icon: LucideIcon;
  label: string;
  count?: number;
  action?: ReactNode;
}) {
  return (
    <div className="flex items-center gap-1.5">
      <Icon className="h-3.5 w-3.5 shrink-0 text-muted-foreground/70" />
      <span className="text-[10px] font-semibold uppercase tracking-widest text-muted-foreground/60">
        {label}
      </span>
      {typeof count === "number" && count > 0 && (
        <span className="rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-muted-foreground/70">
          {count}
        </span>
      )}
      <span className="flex-1" />
      {action}
    </div>
  );
}

/** Dashed empty-state box with a primary line and an optional hint line. */
export function EmptyHint({ text, sub }: { text: string; sub?: string }) {
  return (
    <div className="rounded-lg border border-dashed border-border/70 bg-muted/10 px-4 py-5 text-center">
      <p className="text-[11px] text-muted-foreground/70">{text}</p>
      {sub && <p className="mt-1 text-[10px] text-muted-foreground/50">{sub}</p>}
    </div>
  );
}
