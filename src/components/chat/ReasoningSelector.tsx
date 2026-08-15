/**
 * ReasoningSelector — DeepSeek reasoning effort switcher with an energy slider.
 *
 * The "max" stop plays a WebGL energy-particle burst (ported from Claude
 * Desktop's effort slider). Auto/DeepSeek 自动优化 lives in Settings → 常规.
 *
 * Controlled component: the parent (ChatInput, code mode) supplies value + onChange.
 * Only rendered in Code mode — Depwork has no reasoning setting.
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Brain, ChevronDown } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { EnergySlider } from "@/components/chat/EnergySlider/EnergySlider";
import { cn } from "@/lib/utils";

export type ReasoningMode = "auto" | "low" | "high" | "max";

const EFFORT_STOPS: Array<{ value: number; label: ReasoningMode; accent?: boolean }> = [
  { value: 0, label: "auto" },
  { value: 1, label: "low" },
  { value: 2, label: "high" },
  { value: 3, label: "max", accent: true },
];

export interface ReasoningSelectorProps {
  className?: string;
  value: ReasoningMode;
  onChange: (mode: ReasoningMode) => void;
}

export function ReasoningSelector({ className, value, onChange }: ReasoningSelectorProps) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);

  // Map ReasoningMode → stop index.
  const currentIdx = value === "auto" ? 0 : value === "max" ? 3 : value === "high" ? 2 : 1;

  const handleStopChange = (idx: number) => {
    const next: ReasoningMode =
      idx <= 0 ? "auto" : idx >= 3 ? "max" : idx === 2 ? "high" : "low";
    if (next !== value) onChange(next);
  };

  const currentLabel =
    value === "auto"
      ? t("reasoning.auto")
      : value === "max"
        ? t("reasoning.max")
        : value === "high"
          ? t("reasoning.high")
          : t("reasoning.low");

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className={cn(
            "h-7 shrink-0 gap-1 rounded-full border border-border/60 bg-muted/20 px-2 text-xs hover:bg-muted/40 hover:text-foreground",
            className,
          )}
          aria-label={t("reasoning.label")}
        >
          <Brain className="h-3.5 w-3.5 text-muted-foreground" />
          <span>{currentLabel}</span>
          <ChevronDown className="h-3 w-3 text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-52">
        <DropdownMenuLabel>{t("reasoning.label")}</DropdownMenuLabel>
        <DropdownMenuSeparator />
        <div className="p-2.5">
          <EnergySlider
            stops={EFFORT_STOPS.map((s) => ({
              value: s.value,
              label: t(s.label),
              desc: t(`${s.label}Desc`),
              accent: s.accent,
            }))}
            value={currentIdx}
            onChange={handleStopChange}
          />
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
