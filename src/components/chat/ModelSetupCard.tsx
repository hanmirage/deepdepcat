/**
 * ModelSetupCard — hero empty-state for the "no model yet" gap.
 *
 * Status-aware: inspects the live provider config and tells the user exactly
 * what's missing (no provider / no API key / models not fetched), instead of
 * a generic "configure a model" hint. Where the gap can be closed in one
 * click (provider ready but model list empty), the card calls the provider's
 * /models endpoint directly and reports the result inline.
 *
 * Theme-driven: uses primary / destructive tokens only — no hardcoded hues.
 */

import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Zap,
  Settings2,
  RefreshCw,
  CheckCircle2,
  XCircle,
  ChevronRight,
  KeyRound,
  ListTree,
  MessageCircle,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { useSettingsStore, getModelSetupStatus, type ModelSetupStatus } from "@/stores/settingsStore";
import { useAppStore } from "@/stores/appStore";
import { cn } from "@/lib/utils";
import type { LucideIcon } from "lucide-react";

const STEPS: { icon: LucideIcon; labelKey: string }[] = [
  { icon: KeyRound, labelKey: "chat.setup.steps.key" },
  { icon: ListTree, labelKey: "chat.setup.steps.models" },
  { icon: MessageCircle, labelKey: "chat.setup.steps.start" },
];

export function ModelSetupCard({ className }: { className?: string }) {
  const { t } = useTranslation();
  const providers = useSettingsStore((s) => s.providers);
  const fetchModels = useSettingsStore((s) => s.fetchModels);
  const openSettings = useAppStore((s) => s.openSettings);

  const [fetching, setFetching] = useState(false);
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  const status = useMemo<ModelSetupStatus>(
    () => getModelSetupStatus(providers),
    [providers],
  );

  // The provider that will move the user forward: prefer one that only needs
  // a model fetch; otherwise the first enabled one.
  const targetProvider = useMemo(() => {
    if (status === "no-models") {
      return providers.find((p) => p.enabled && p.apiKey && p.baseUrl) ?? null;
    }
    return providers.find((p) => p.enabled) ?? null;
  }, [providers, status]);

  if (status === "ready") return null;
  const providerName = targetProvider?.name ?? "DeepSeek";

  const title =
    status === "no-provider"
      ? t("chat.setup.noProvider.title")
      : status === "missing-key"
        ? t("chat.setup.missingKey.title", { name: providerName })
        : t("chat.setup.noModels.title", { name: providerName });

  const desc =
    status === "no-provider"
      ? t("chat.setup.noProvider.desc")
      : status === "missing-key"
        ? t("chat.setup.missingKey.desc", { name: providerName })
        : t("chat.setup.noModels.desc", { name: providerName });

  // Which setup step the user is currently on (for the step pills).
  const activeStep = status === "no-models" ? 1 : 0;

  const handleFetch = async () => {
    if (!targetProvider || fetching) return;
    setFetching(true);
    setFeedback(null);
    const result = await fetchModels(targetProvider.id);
    setFetching(false);
    if (result.success) {
      setFeedback({ ok: true, text: t("chat.setup.fetchOk", { count: result.count }) });
    } else {
      setFeedback({ ok: false, text: t("chat.setup.fetchFail", { error: result.error ?? "unknown" }) });
    }
  };

  return (
    <div className={cn("w-full max-w-2xl", className)}>
      <div className="rounded-2xl bg-gradient-to-b from-primary/50 via-primary/20 to-primary/5 p-px shadow-[0_8px_32px_-12px_hsl(var(--primary)/0.45)]">
        <div className="flex flex-col items-center gap-3 rounded-[calc(1rem-1px)] bg-card/90 px-5 py-4 backdrop-blur-xl">
          {/* ── Glowing icon ── */}
          <div className="relative">
            <div className="absolute inset-0 rounded-full bg-primary/35 blur-lg" />
            <div className="relative flex h-9 w-9 items-center justify-center rounded-full bg-gradient-to-br from-primary to-primary/60 text-primary-foreground shadow-md shadow-primary/40">
              <Zap className="h-4 w-4" />
            </div>
          </div>

          {/* ── Title / description ── */}
          <div className="text-center">
            <h3 className="text-sm font-semibold text-foreground">{title}</h3>
            <p className="mt-0.5 max-w-sm text-[11px] leading-relaxed text-muted-foreground">{desc}</p>
          </div>

          {/* ── Setup steps ── */}
          <div className="flex items-center gap-1.5">
            {STEPS.map((step, i) => {
              const Icon = step.icon;
              const done = i < activeStep;
              const active = i === activeStep;
              return (
                <div
                  key={step.labelKey}
                  className={cn(
                    "flex items-center gap-1 rounded-full border px-2 py-0.5 text-[10px] font-medium transition-colors",
                    active
                      ? "border-primary/40 bg-primary/10 text-primary"
                      : done
                        ? "border-primary/20 bg-primary/5 text-primary/80"
                        : "border-border text-muted-foreground",
                  )}
                >
                  <Icon className={cn("h-3 w-3", active ? "text-primary" : "text-muted-foreground")} />
                  {t(step.labelKey)}
                  {i < STEPS.length - 1 && (
                    <ChevronRight className="h-2.5 w-2.5 text-muted-foreground/50" />
                  )}
                </div>
              );
            })}
          </div>

          {/* ── Inline fetch feedback ── */}
          {feedback && (
            <p
              className={cn(
                "flex items-center gap-1.5 rounded-full border px-3 py-1 text-[11px]",
                feedback.ok
                  ? "border-primary/30 bg-primary/10 text-primary"
                  : "border-destructive/30 bg-destructive/10 text-destructive",
              )}
            >
              {feedback.ok ? (
                <CheckCircle2 className="h-3 w-3 shrink-0" />
              ) : (
                <XCircle className="h-3 w-3 shrink-0" />
              )}
              <span className="truncate">{feedback.text}</span>
            </p>
          )}

          {/* ── CTA ── */}
          <div className="flex items-center gap-2">
            {status === "no-models" ? (
              <>
                <Button
                  size="sm"
                  onClick={handleFetch}
                  disabled={fetching}
                  className="gap-1.5 bg-primary text-primary-foreground shadow-md shadow-primary/25 transition-all hover:scale-105"
                >
                  <RefreshCw className={cn("h-3.5 w-3.5", fetching && "animate-spin")} />
                  {fetching ? t("chat.setup.fetching") : t("chat.setup.fetchModels")}
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => openSettings("models")}
                  className="gap-1.5"
                >
                  <Settings2 className="h-3.5 w-3.5" />
                  {t("chat.setup.openSettings")}
                </Button>
              </>
            ) : (
              <Button
                size="sm"
                onClick={() => openSettings("models")}
                className="gap-1.5 bg-primary text-primary-foreground shadow-md shadow-primary/25 transition-all hover:scale-105"
              >
                <Settings2 className="h-3.5 w-3.5" />
                {t("chat.setup.configure")}
                <ChevronRight className="h-3.5 w-3.5" />
              </Button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
