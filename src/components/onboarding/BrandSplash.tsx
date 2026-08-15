/**
 * BrandSplash — Step 1: Full-screen brand page.
 *
 * A calming intro that establishes the DeepDepCat identity before guiding
 * the user through the product.
 *
 * Design: clean paper-white background with deep text — a quiet, premium
 * first impression. A soft monochrome radial glow adds depth without
 * introducing color noise.
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import blink1 from "/blink-1.png";
import blink2 from "/blink-2.png";
import blink3 from "/blink-3.png";

/** Blink animation frames: 1 = open, 2 = half-closed, 3 = closed. */
const BLINK_FRAMES = [blink1, blink2, blink3, blink2, blink1];
const BLINK_FRAME_MS = 90;
const BLINK_IDLE_MS = 3600;
/** Minimum time the splash stays visible before the user can leave — long
 *  enough for the entrance animations (max ~700ms + 500ms delay) to settle,
 *  short enough that it never feels like the app is stuck. */
const MIN_DISPLAY_MS = 1600;

export interface BrandSplashProps {
  onNext: () => void;
}

export function BrandSplash({ onNext }: BrandSplashProps) {
  const { t } = useTranslation();
  // Index into BLINK_FRAMES; 0 = eyes open (the resting frame).
  const [frame, setFrame] = useState(0);
  const [ready, setReady] = useState(false);

  // Keep the splash on screen for at least MIN_DISPLAY_MS.
  useEffect(() => {
    const id = setTimeout(() => setReady(true), MIN_DISPLAY_MS);
    return () => clearTimeout(id);
  }, []);

  const handleNext = useCallback(() => {
    if (ready) onNext();
  }, [ready, onNext]);

  // Loop the blink: wait, then play 1→2→3→2→1, then reset to open.
  // Disabled entirely under prefers-reduced-motion — the eyes stay open.
  useEffect(() => {
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) {
      setFrame(0);
      return;
    }
    const timeouts: ReturnType<typeof setTimeout>[] = [];
    let cancelled = false;

    const schedule = (t: number, fn: () => void) => {
      const id = setTimeout(() => { if (!cancelled) fn(); }, t);
      timeouts.push(id);
    };

    const playBlink = () => {
      BLINK_FRAMES.forEach((_, i) => schedule(i * BLINK_FRAME_MS, () => setFrame(i)));
      schedule(BLINK_FRAMES.length * BLINK_FRAME_MS + BLINK_IDLE_MS, playBlink);
    };

    schedule(BLINK_IDLE_MS, playBlink);
    return () => {
      cancelled = true;
      timeouts.forEach(clearTimeout);
    };
  }, []);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Enter") handleNext();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [ready, onNext, handleNext]);

  return (
    <div className="flex h-screen w-screen flex-col items-center justify-center overflow-hidden bg-[hsl(var(--background))]">
      {/* Soft ambient glow — neutral, keeps the white-premium feel */}
      <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_center,rgba(0,0,0,0.05),transparent_62%)]" />
      <div className="absolute inset-0 bg-[radial-gradient(ellipse_at_80%_-10%,rgba(124,107,232,0.06),transparent_55%)]" />

      {/* Subtle floating particles — dark specks on white */}
      <div className="absolute inset-0 overflow-hidden">
        <div className="absolute top-1/4 left-1/4 h-2 w-2 rounded-full bg-foreground/10 animate-pulse" />
        <div className="absolute top-1/3 right-1/3 h-1.5 w-1.5 rounded-full bg-foreground/10 animate-pulse" style={{ animationDelay: "0.5s" }} />
        <div className="absolute bottom-1/3 left-1/3 h-1 w-1 rounded-full bg-foreground/10 animate-pulse" style={{ animationDelay: "1s" }} />
        <div className="absolute bottom-1/4 right-1/4 h-2 w-2 rounded-full bg-primary/15 animate-pulse" style={{ animationDelay: "1.5s" }} />
      </div>

      <div className="relative z-10 flex flex-col items-center gap-6">
        {/* Logo — the app icon (cat), on a subtle paper chip.
            The cat blinks: opens → half → closed → half → open. */}
        <div className={cn(
          "flex h-32 w-32 items-center justify-center overflow-hidden rounded-3xl",
          "bg-white ring-1 ring-black/10",
          "shadow-[0_12px_40px_rgba(0,0,0,0.08)]",
          "animate-in fade-in zoom-in-75 duration-700",
        )}>
          <img
            src={BLINK_FRAMES[frame]}
            alt="DeepDepCat"
            className="h-28 w-28 rounded-2xl transition-opacity duration-75"
          />
        </div>

        {/* Brand name — deep ink */}
        <h1 className={cn(
          "text-4xl font-bold tracking-tight text-foreground sm:text-5xl",
          "animate-in fade-in slide-in-from-bottom-4 duration-700 delay-150",
        )}>
          DeepDepCat
        </h1>

        {/* Tagline — soft gray */}
        <p className={cn(
          "text-lg font-medium text-muted-foreground",
          "animate-in fade-in slide-in-from-bottom-4 duration-700 delay-300",
        )}>
          {t("onboarding.tagline")}
        </p>

        {/* CTA — ink button on paper */}
        <div className={cn(
          "mt-8",
          "animate-in fade-in slide-in-from-bottom-4 duration-700 delay-500",
        )}>
          <Button
            size="lg"
            disabled={!ready}
            className="bg-foreground text-background hover:bg-foreground/90 shadow-lg px-8"
            onClick={handleNext}
          >
            {ready
              ? t("onboarding.start", { defaultValue: "开始" })
              : t("onboarding.starting", { defaultValue: "即将开始…" })}
          </Button>
        </div>
      </div>

      {/* Version hint — pinned to the very bottom of the page */}
      <p className="absolute bottom-8 left-1/2 -translate-x-1/2 text-xs text-muted-foreground/50">
        v1.0.0
      </p>
    </div>
  );
}
