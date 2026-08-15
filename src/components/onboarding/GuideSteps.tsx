/**
 * GuideSteps — Step 2: Three large guide cards introducing the product's core capabilities.
 *
 * Each card fills the screen with an icon, title, and description.
 *
 * Keyboard: ←/→ to flip cards, Enter to advance. Enter is only handled when
 * the focus is NOT on a button — otherwise pressing Enter on the focused
 * "Skip"/"Next" button would fire BOTH the native click and the global
 * handler (double advance / skip).
 */

import { useMemo, useState, useEffect, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, ChevronLeft } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import codeIcon from "/icon-code.png";
import depworkIcon from "/icon-depwork.png";
import coffeeIcon from "/icon-idle.png";

interface GuideStep {
  img: string;
  /** i18n keys — the guide must follow the app language. */
  titleKey: string;
  subtitleKey: string;
  descKey: string;
  bgAccent: string;
}

const STEPS: GuideStep[] = [
  {
    img: codeIcon,
    titleKey: "onboarding.guideTitle1",
    subtitleKey: "onboarding.guideSubtitle1",
    descKey: "onboarding.guideDesc1",
    bgAccent: "from-blue-500/10 to-cyan-500/10",
  },
  {
    img: depworkIcon,
    titleKey: "onboarding.guideTitle2",
    subtitleKey: "onboarding.guideSubtitle2",
    descKey: "onboarding.guideDesc2",
    bgAccent: "from-violet-500/10 to-purple-500/10",
  },
  {
    img: coffeeIcon,
    titleKey: "onboarding.guideTitle3",
    subtitleKey: "onboarding.guideSubtitle3",
    descKey: "onboarding.guideDesc3",
    bgAccent: "from-emerald-500/10 to-teal-500/10",
  },
];

export interface GuideStepsProps {
  onNext: () => void;
  onSkip: () => void;
}

export function GuideSteps({ onNext, onSkip }: GuideStepsProps) {
  const { t } = useTranslation();
  const [current, setCurrent] = useState(0);
  const isLast = current === STEPS.length - 1;

  const step = useMemo(() => STEPS[current], [current]);

  const goNext = useCallback(() => {
    if (isLast) {
      onNext();
    } else {
      setCurrent((c) => c + 1);
    }
  }, [isLast, onNext]);

  const goPrev = useCallback(() => {
    setCurrent((c) => Math.max(0, c - 1));
  }, []);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // Never hijack Enter/Space while a button has focus — the native
      // activation must be the ONLY trigger (no double advance).
      if (e.target instanceof HTMLElement && e.target.closest("button, a, [role='button']")) {
        return;
      }
      if (e.key === "ArrowRight" || e.key === "Enter") goNext();
      if (e.key === "ArrowLeft") goPrev();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [goNext, goPrev]);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden">
      {/* Top progress bar */}
      <div className="flex h-1 w-full">
        {STEPS.map((_, i) => (
          <div
            key={i}
            className={cn(
              "h-full flex-1 transition-all duration-500",
              i <= current ? "bg-primary" : "bg-primary/20",
            )}
          />
        ))}
      </div>

      {/* Card area */}
      <div className="flex flex-1 items-center justify-center px-8">
        <div
          className={cn(
            "w-full max-w-2xl animate-in fade-in zoom-in-95 duration-500",
          )}
          key={current}
        >
          <div className="text-center">
            {/* Step number */}
            <p className="text-sm font-medium text-muted-foreground">
              {current + 1} / {STEPS.length}
            </p>

            {/* Icon card */}
            <div className={cn(
              "mt-4 inline-flex flex-col items-center gap-6 rounded-3xl bg-gradient-to-br p-8",
              step.bgAccent,
            )}>
              <div className="flex h-24 w-24 items-center justify-center overflow-hidden rounded-2xl bg-primary/10">
                <img src={step.img} alt={t(step.titleKey)} className="h-20 w-20 rounded-xl" />
              </div>
              <div>
                <h2 className="text-2xl font-bold text-foreground sm:text-3xl">
                  {t(step.titleKey)}
                </h2>
                <p className="mt-1 text-sm text-primary">{t(step.subtitleKey)}</p>
              </div>
            </div>

            {/* Description */}
            <p className="mt-6 text-base text-muted-foreground">
              {t(step.descKey)}
            </p>
          </div>
        </div>
      </div>

      {/* Bottom navigation */}
      <div className="flex items-center justify-between px-8 pb-8">
        <div className="flex items-center gap-2">
          {STEPS.map((_, i) => (
            <button
              key={i}
              onClick={() => setCurrent(i)}
              aria-label={t("onboarding.stepLabel", { current: i + 1, total: STEPS.length })}
              aria-current={i === current ? "step" : undefined}
              className={cn(
                "h-2 rounded-full transition-all duration-300",
                i === current ? "w-8 bg-primary" : "w-2 bg-primary/30 hover:bg-primary/50",
              )}
            />
          ))}
        </div>

        <div className="flex items-center gap-3">
          <Button
            variant="ghost"
            onClick={onSkip}
            className="text-muted-foreground"
          >
            {t("onboarding.skipGuide")}
          </Button>

          {current > 0 && (
            <Button variant="outline" size="icon" onClick={goPrev} aria-label={t("onboarding.backToGuide")}>
              <ChevronLeft className="h-4 w-4" />
            </Button>
          )}

          <Button
            className="gap-2"
            onClick={goNext}
          >
            {isLast ? t("onboarding.guideContinue") : t("onboarding.guideNext")}
            <ChevronRight className="h-4 w-4" />
          </Button>
        </div>
      </div>
    </div>
  );
}
