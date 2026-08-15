/**
 * SettingRow — reusable settings row component.
 *
 * Mirrors a modern settings layout:
 * ┌──────────────────────────────────────────────────────┐
 * │ Label                                                │
 * │ Description text (small, muted)                      │
 * │ [Control: switch / select / input]                   │
 * └──────────────────────────────────────────────────────┘
 *
 * Props:
 * - label: setting name
 * - description: what it does (shown as muted helper text)
 * - children: the control element (Switch, Select, Input, etc.)
 */

import { type ReactNode } from "react";
import { cn } from "@/lib/utils";

export interface SettingRowProps {
  label: string;
  description?: string;
  children: ReactNode;
  className?: string;
  /** i18n key used by settings search to locate & highlight this row. */
  searchKey?: string;
}

export function SettingRow({
  label,
  description,
  children,
  className,
  searchKey,
}: SettingRowProps) {
  return (
    <div className={cn("py-3", className)} data-search-key={searchKey}>
      {/* The whole row is an implicit <label>: clicking anywhere toggles the
          switch / focuses the input (large hit target), and screen readers
          get the control↔label association for free. */}
      <label className="flex cursor-pointer items-start justify-between gap-4">
        <span className="flex-1 min-w-0">
          <span className="block text-xs font-medium text-foreground">{label}</span>
          {description && (
            <span className="mt-0.5 block text-[11px] leading-relaxed text-muted-foreground">
              {description}
            </span>
          )}
        </span>

        <span className="flex items-center gap-2">
          {children}
        </span>
      </label>
    </div>
  );
}
