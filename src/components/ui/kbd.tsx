/**
 * Kbd — unified keyboard-hint chip for action buttons.
 *
 * One visual language for the Enter / Shift+Enter / Esc hints across the
 * decision cards (PermissionDialog, AskUserDialog, ElicitationDialog,
 * PlanApprovalPanel) — previously each card hand-rolled its own <kbd>.
 */

import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

interface KbdProps {
  children: ReactNode;
  className?: string;
}

export function Kbd({ children, className }: KbdProps) {
  return (
    <kbd
      className={cn(
        "ml-1 rounded border border-current/25 bg-current/10 px-1 py-px text-[9px] font-normal tabular-nums leading-none",
        className,
      )}
    >
      {children}
    </kbd>
  );
}
