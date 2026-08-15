/**
 * ProductMenu — title-bar Code / Depwork surface switcher.
 *
 * Clicking the window title (the product identity) opens the two-surface
 * menu; the sidebar keeps only its collapse toggle. A pulsing dot marks
 * the surface with a live streaming session.
 */

import { useTranslation } from "react-i18next";
import { Check, ChevronDown, Code2, FileText } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useAppStore } from "@/stores/appStore";
import { cn } from "@/lib/utils";
import type { AppMode } from "@/config/constants";

interface ProductDef {
  mode: AppMode;
  icon: typeof Code2;
  labelKey: string;
  descKey: string;
  iconClass: string;
}

const PRODUCTS: ProductDef[] = [
  {
    mode: "code",
    icon: Code2,
    labelKey: "layout.productCode",
    descKey: "layout.productCodeDesc",
    iconClass: "text-sky-600 dark:text-sky-400",
  },
  {
    mode: "depwork",
    icon: FileText,
    labelKey: "layout.productDepwork",
    descKey: "layout.productDepworkDesc",
    iconClass: "text-violet-600 dark:text-violet-400",
  },
];

export interface ProductMenuProps {
  /** True while the Code surface has a live streaming session. */
  codeStreaming?: boolean;
  /** True while the Depwork surface has a live streaming session. */
  depworkStreaming?: boolean;
}

export function ProductMenu({
  codeStreaming = false,
  depworkStreaming = false,
}: ProductMenuProps) {
  const { t } = useTranslation();
  const mode = useAppStore((s) => s.mode);
  const setMode = useAppStore((s) => s.setMode);

  const currentLabel =
    mode === "code" ? t("layout.codeMode") : t("layout.depworkMode");

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          data-no-drag
          className="flex h-7 items-center gap-1 rounded-md px-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted/80 hover:text-foreground"
          aria-label={currentLabel}
        >
          {currentLabel}
          <ChevronDown className="h-3 w-3 text-muted-foreground/60" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-56">
        {PRODUCTS.map((p) => {
          const Icon = p.icon;
          const active = mode === p.mode;
          const streaming = p.mode === "code" ? codeStreaming : depworkStreaming;
          return (
            <DropdownMenuItem
              key={p.mode}
              onClick={() => setMode(p.mode)}
              className="flex items-center gap-2 py-1.5"
              data-mode={p.mode}
              data-streaming={streaming ? "true" : "false"}
            >
              <Icon className={cn("h-3.5 w-3.5 shrink-0", p.iconClass)} />
              <span className="flex-1">
                <span className="block text-xs font-medium">{t(p.labelKey)}</span>
                <span className="block text-[10px] text-muted-foreground">
                  {t(p.descKey)}
                </span>
              </span>
              {streaming && (
                <span
                  className="relative flex h-1.5 w-1.5 shrink-0"
                  title={t("layout.productStreaming")}
                >
                  <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-primary opacity-60" />
                  <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-primary" />
                </span>
              )}
              {active && <Check className="h-3 w-3 shrink-0 text-primary" />}
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
