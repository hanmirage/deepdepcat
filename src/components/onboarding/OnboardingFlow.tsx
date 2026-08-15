/**
 * OnboardingFlow — first-run full-screen onboarding experience.
 *
 * Steps:
 *   1. BrandSplash  — logo + tagline + "开始"
 *   2. GuideSteps   — 3 feature cards (Code / Depwork / Customize)
 *   3. InitConfig   — pick model / workspace / theme (+ optional sign-in)
 *
 * The current step is persisted: closing the app mid-onboarding resumes at
 * the same step instead of restarting from the splash.
 */

import { useState, useCallback } from "react";
import { BrandSplash } from "./BrandSplash";
import { GuideSteps } from "./GuideSteps";
import { InitConfig } from "./InitConfig";
import { useOnboardingStore } from "@/stores/onboardingStore";

type Step = "splash" | "guide" | "config";

const STEP_KEY = "deepdepcat.onboardingStep";

function loadStep(): Step {
  try {
    const s = localStorage.getItem(STEP_KEY);
    if (s === "guide" || s === "config") return s;
  } catch {
    // localStorage unavailable — start from scratch
  }
  return "splash";
}

export function OnboardingFlow() {
  const [step, setStep] = useState<Step>(loadStep);
  const setCompleted = useOnboardingStore((s) => s.setCompleted);

  const changeStep = useCallback((next: Step) => {
    setStep(next);
    try {
      if (next === "splash") localStorage.removeItem(STEP_KEY);
      else localStorage.setItem(STEP_KEY, next);
    } catch {
      // Persistence is best-effort — the flow still works in-memory.
    }
  }, []);

  const handleFinish = useCallback(() => {
    try {
      localStorage.removeItem(STEP_KEY);
    } catch {
      // Ignore
    }
    setCompleted();
  }, [setCompleted]);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-background">
      {step === "splash" && <BrandSplash onNext={() => changeStep("guide")} />}
      {step === "guide" && (
        <GuideSteps onNext={() => changeStep("config")} onSkip={handleFinish} />
      )}
      {step === "config" && (
        <InitConfig onComplete={handleFinish} onBack={() => changeStep("guide")} />
      )}
    </div>
  );
}
