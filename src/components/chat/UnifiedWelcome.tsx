/**
 * UnifiedWelcome — empty-state hero for both Code and Depwork modes.
 *
 * Shared layout:
 *   logo / heading + subtitle / glass card with input / hint cards grid
 *
 * Mode-specific content injected via `mode` prop:
 *   - "code"    → 编码主题 + 编码提示卡
 *   - "depwork" → 学术主题 + 学术提示卡
 */

import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import {
  Sparkles,
  FolderOpen,
  Wrench,
  FileText,
  Bug,
  PenLine,
  BarChart3,
} from "lucide-react";
import { ChatInput } from "./ChatInput";
import { ModelSetupCard } from "./ModelSetupCard";
import { TimeGreeting } from "./TimeGreeting";
import { DepworkFolderSelector } from "@/components/depwork/DepworkFolderSelector";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useStore } from "zustand";
import type { LucideIcon } from "lucide-react";
import { cn } from "@/lib/utils";
import type { AppMode } from "@/config/constants";
import codeIcon from "/icon-code.png";
import depworkIcon from "/icon-depwork.png";

interface HintCard {
  icon: LucideIcon;
  label: string;
}

const CODE_HINTS: HintCard[] = [
  { icon: FolderOpen, label: "hints.summarize" },
  { icon: Wrench, label: "hints.refactor" },
  { icon: FileText, label: "hints.migrate" },
  { icon: Bug, label: "hints.explain" },
];

const DEPWORK_HINTS: HintCard[] = [
  { icon: FileText, label: "depwork.quickTask1" },
  { icon: PenLine, label: "depwork.quickTask2" },
  { icon: BarChart3, label: "depwork.quickTask3" },
];

export interface UnifiedWelcomeProps {
  mode: AppMode;
  className?: string;
}

export function UnifiedWelcome({ mode, className }: UnifiedWelcomeProps) {
  const { t } = useTranslation();
  const isDepwork = mode === "depwork";

  // Pick the right store based on mode
  const chatStore = isDepwork ? useDepworkChatStore : useChatStore;
  const setInputText = useStore(chatStore, (s) => s.setInputText);
  const selectedModel = useStore(chatStore, (s) => s.selectedModel);

  const hints = useMemo(
    () => (isDepwork ? DEPWORK_HINTS : CODE_HINTS),
    [isDepwork],
  );

  return (
    <div
      className={cn(
        "flex flex-1 flex-col items-center justify-center px-4",
        !isDepwork && "before:absolute before:inset-0 before:-z-10 before:bg-gradient-to-b before:from-primary/5 before:to-transparent",
        className,
      )}
    >
      {/* ── Logo / Heading ──────────────────────────────────── */}
      {isDepwork ? (
        <div className="mb-2 flex items-center gap-2.5">
          <img src={depworkIcon} alt={t("depwork.heading")} className="h-9 w-9 rounded-xl" />
          <h1 className="text-2xl font-bold tracking-tight text-foreground sm:text-3xl">
            {t("depwork.heading")}
          </h1>
        </div>
      ) : (
        <>
          <div className="mb-4 flex h-14 w-14 items-center justify-center overflow-hidden rounded-2xl border border-border bg-card shadow-md">
            <img src={codeIcon} alt={t("layout.codeMode")} className="h-12 w-12" />
          </div>
          <div className="mb-6">
            <TimeGreeting />
          </div>
        </>
      )}

      {isDepwork && (
        <p className="mb-8 text-sm text-muted-foreground">
          {t("depwork.subtitle")}
        </p>
      )}

      {/* ── No-model setup card (before the input card) ──────── */}
      {!selectedModel && <ModelSetupCard className="mb-3" />}

      {/* ── Glass card: input ──────────────────────────────── */}
      <div className="w-full max-w-2xl">
        <div
          className={cn(
            "overflow-hidden rounded-2xl border border-border/60 shadow-2xl",
            "backdrop-blur-xl bg-card/80 dark:bg-card/60",
          )}
        >
          {isDepwork && (
            <div className="flex items-center justify-between border-b border-border/40 px-3 py-2">
              <DepworkFolderSelector />
            </div>
          )}
          <ChatInput compact={false} embedded mode={mode} hideSetupNotice={!selectedModel} />
        </div>
      </div>

      {/* ── Hint / quick-task cards ─────────────────────────── */}
      {isDepwork ? (
        <div className="mt-6 w-full max-w-2xl">
          <div className="mb-2.5 flex items-center gap-2 text-xs font-medium text-muted-foreground">
            <Sparkles className="h-3.5 w-3.5" />
            <span>{t("depwork.quickStart")}</span>
          </div>
          <div className="space-y-2">
            {hints.map((hint) => {
              const Icon = hint.icon;
              return (
                <button
                  key={hint.label}
                  onClick={() => setInputText(t(hint.label))}
                  className={cn(
                    "group flex w-full items-center gap-3 rounded-xl border border-border/60",
                    "bg-card/50 px-4 py-3 text-left",
                    "transition-all duration-200",
                    "hover:border-border hover:bg-card hover:shadow-md",
                    "dark:hover:bg-card/80",
                  )}
                >
                  <span
                    className={cn(
                      "flex h-9 w-9 shrink-0 items-center justify-center rounded-lg",
                      "bg-muted text-muted-foreground",
                      "transition-colors group-hover:bg-primary/10 group-hover:text-primary",
                    )}
                  >
                    <Icon className="h-4 w-4" />
                  </span>
                  <span
                    className={cn(
                      "text-sm font-medium text-muted-foreground",
                      "transition-colors group-hover:text-foreground",
                    )}
                  >
                    {t(hint.label)}
                  </span>
                </button>
              );
            })}
          </div>
        </div>
      ) : (
        <div className="mt-5 grid grid-cols-2 gap-2.5">
          {hints.map((hint) => {
            const Icon = hint.icon;
            return (
              <button
                key={hint.label}
                onClick={() => setInputText(t(hint.label))}
                className={cn(
                  "group flex items-center gap-2.5 rounded-xl border border-border/60 bg-card/50 px-3.5 py-2.5",
                  "backdrop-blur-sm transition-all hover:border-border hover:bg-card hover:shadow-md",
                  "dark:hover:bg-card/80",
                )}
              >
                <span className="flex h-7 w-7 items-center justify-center rounded-lg bg-muted text-muted-foreground transition-colors group-hover:bg-primary/10 group-hover:text-primary">
                  <Icon className="h-3.5 w-3.5" />
                </span>
                <span className="text-xs font-medium text-muted-foreground transition-colors group-hover:text-foreground">
                  {t(hint.label)}
                </span>
              </button>
            );
          })}
        </div>
      )}

    </div>
  );
}
