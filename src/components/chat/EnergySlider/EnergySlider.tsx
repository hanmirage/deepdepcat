/**
 * EnergySlider — the effort slider with a WebGL energy-particle canvas.
 *
 * Ported from Claude Desktop's effort picker. A horizontal bar of rounded
 * cells sits under the slider track; dragging fires bursts of energy that
 * race across the cells, strongest at the top stop. The bar is invisible at
 * rest — energy IS visibility.
 *
 * Render loop: animation-frame driven, but ONLY while there is energy to
 * show. When the field fully cools down the loop stops, so idle bars burn no
 * CPU and — critically — no zombie loop can clear a sibling canvas.
 */

import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { createEnergyBar, type EnergyBarHandle } from "./energyShader";
import { cn } from "@/lib/utils";

// Per-canvas bar handle. React StrictMode double-invokes effects (mount →
// cleanup → mount), and Radix popover remounts the content on every open.
// Storing the handle on the canvas itself guarantees every click reaches the
// live one, and only one bar ever binds a given canvas.
const canvasBars = new WeakMap<HTMLCanvasElement, EnergyBarHandle>();

function getBar(canvas: HTMLCanvasElement | null): EnergyBarHandle | null {
  if (!canvas) return null;
  const cached = canvasBars.get(canvas);
  if (cached) return cached;
  const bar = createEnergyBar(canvas);
  if (bar) canvasBars.set(canvas, bar);
  return bar;
}

export interface EffortStop {
  value: number;
  label: string;
  /** Short description of this stop — shown under the current selection. */
  desc?: string;
  /** Accent styling (purple) — the top stop. */
  accent?: boolean;
}

export interface EnergySliderProps {
  stops: EffortStop[];
  /** Current stop index (0-based). */
  value: number;
  onChange: (index: number) => void;
  className?: string;
}

export function EnergySlider({ stops, value, onChange, className }: EnergySliderProps) {
  const { t } = useTranslation();
  const trackRef = useRef<HTMLDivElement>(null);
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const draggingRef = useRef(false);

  const maxIdx = stops.length - 1;
  // Position fraction 0..1 from the current stop.
  const frac = maxIdx > 0 ? value / maxIdx : 1;

  // ── Init the WebGL energy bar ─────────────────────────────
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const bar = getBar(canvas);
    if (bar) bar.setPos(frac);
    return () => {
      // Destroy only when the canvas is really leaving the DOM (popover
      // closes). During StrictMode's mount→cleanup→mount the canvas stays
      // connected, so the live bar is kept and its rAF loop is re-started by
      // the next burst instead of being orphaned.
      if (canvas && !canvas.isConnected) {
        const cached = canvasBars.get(canvas);
        if (cached) {
          cached.destroy();
          canvasBars.delete(canvas);
        }
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Keep the charge surface aligned with the current stop.
  useEffect(() => {
    getBar(canvasRef.current)?.setPos(frac);
  }, [frac]);

  // ── Pointer interaction ───────────────────────────────────
  const posFromClientX = (clientX: number) => {
    const el = trackRef.current;
    if (!el) return null;
    const r = el.getBoundingClientRect();
    if (r.width <= 0) return null;
    return Math.min(Math.max((clientX - r.left) / r.width, 0), 1);
  };

  const idxFromPos = (pos: number) => Math.round(pos * maxIdx);

  const applyDrag = (clientX: number) => {
    const pos = posFromClientX(clientX);
    if (pos == null) return;
    const idx = idxFromPos(pos);
    if (idx !== value) onChange(idx);
    // Gain floor 0.6 — every stop shows visible energy (Claude's bar lights
    // up on any drag, not just at max); the top stop still burns brightest.
    getBar(canvasRef.current)?.burst(pos, 0.6 + 0.4 * (idx / maxIdx));
  };

  const onPointerDown = (e: React.PointerEvent) => {
    e.preventDefault();
    draggingRef.current = true;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    getBar(canvasRef.current)?.press();
    applyDrag(e.clientX);
  };
  const onPointerMove = (e: React.PointerEvent) => {
    if (!draggingRef.current) return;
    applyDrag(e.clientX);
  };
  const endDrag = (e: React.PointerEvent) => {
    if (!draggingRef.current) return;
    draggingRef.current = false;
    getBar(canvasRef.current)?.release();
    const pos = posFromClientX(e.clientX);
    if (pos != null) getBar(canvasRef.current)?.burst(pos, 1);
  };

  return (
    <div className={cn("flex w-full flex-col gap-1.5", className)}>
      <div className="flex items-center justify-between text-[10px] text-muted-foreground">
        <span>{t("reasoning.faster")}</span>
        <span>{t("reasoning.smarter")}</span>
      </div>

      <div
        ref={trackRef}
        role="slider"
        aria-valuemin={0}
        aria-valuemax={maxIdx}
        aria-valuenow={value}
        aria-valuetext={stops[value]?.label}
        tabIndex={0}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={endDrag}
        onPointerCancel={endDrag}
        onKeyDown={(e) => {
          if (e.key === "ArrowRight" || e.key === "ArrowUp") {
            e.preventDefault();
            onChange(Math.min(value + 1, maxIdx));
          } else if (e.key === "ArrowLeft" || e.key === "ArrowDown") {
            e.preventDefault();
            onChange(Math.max(value - 1, 0));
          } else if (e.key === "Home") {
            e.preventDefault();
            onChange(0);
          } else if (e.key === "End") {
            e.preventDefault();
            onChange(maxIdx);
          }
        }}
        className={cn(
          "group relative flex h-5 w-full cursor-ew-resize touch-none select-none items-center outline-none",
          draggingRef.current && "cursor-grabbing",
        )}
      >
        {/* Energy track — a rounded bar. The canvas lives INSIDE it (same
            bounds), so particles flow within the track instead of bleeding
            above/below a thin line. */}
        <div className="relative h-4 w-full overflow-hidden rounded-full">
          {/* Track background — a neutral base dark enough that the purple
              particles read clearly (a near-white base swallows them). */}
          <div className="absolute inset-0 rounded-full bg-muted-foreground/15" />
          {/* Fill — a FIXED saturated purple (not the theme primary, which
              leans blue). #8b5cf6 is a true violet; the earlier #a78bfa was
              lavender with a blue cast, which read as "blue". */}
          <div
            className="absolute inset-y-0 left-0 rounded-full bg-[#8b5cf6]/45 transition-[width] duration-150 ease-out"
            style={{ width: `${frac * 100}%` }}
          />
          {/* Energy canvas — clipped to the track by overflow-hidden. */}
          <canvas
            ref={canvasRef}
            className="pointer-events-none absolute inset-0 h-full w-full"
            style={{ color: "var(--ui-slider-energy-hot)", outlineColor: "var(--ui-slider-energy-cool)" }}
          />
        </div>

        {/* Stop dots */}
        <div className="pointer-events-none absolute inset-0 flex items-center justify-between px-1.5">
          {stops.map((stop, i) => (
            <span
              key={stop.value}
              className={cn(
                "relative flex size-1 items-center justify-center rounded-full transition-opacity duration-300",
                i <= value
                  ? stop.accent
                    ? "bg-[#8b5cf6]"
                    : "bg-primary"
                  : "bg-muted-foreground/40",
              )}
            />
          ))}
        </div>

        {/* Handle — Claude-style round knob: white dot with a primary ring
            and a soft shadow, sitting on the fill line. Clamped to the track
            so the knob never bleeds past either end (left=0%, max=100%). */}
        <div
          className="pointer-events-none absolute top-1/2 h-4 w-4 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-primary bg-background shadow-sm transition-[left,background,box-shadow] duration-150 ease-out group-active:shadow-md"
          style={{ left: `clamp(8px, ${frac * 100}%, calc(100% - 8px))` }}
        />
      </div>

      {/* Labels row */}
      <div className="flex items-center justify-between px-0.5">
        {stops.map((stop, i) => (
          <button
            key={stop.value}
            type="button"
            onClick={() => {
              onChange(i);
              // Fire a burst — and hold the "press" briefly so the rAF loop
              // keeps re-firing bursts for a satisfying multi-pulse burst
              // (matches the feel of dragging the slider to this stop).
              const bar = getBar(canvasRef.current);
              if (!bar) return;
              bar.press();
              bar.burst(i / maxIdx, 0.6 + 0.4 * (i / maxIdx));
              window.setTimeout(() => bar.release(), 400);
            }}
            className={cn(
              "rounded px-1 py-0.5 text-[10px] transition-colors",
              i === value
                ? stop.accent
                  ? "font-semibold text-[#8b5cf6]"
                  : "font-semibold text-foreground"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {stop.label}
          </button>
        ))}
      </div>

      {/* Current stop description — what this level actually does. */}
      {stops[value]?.desc && (
        <p className="text-center text-[10px] leading-snug text-muted-foreground/60">
          {stops[value].desc}
        </p>
      )}
    </div>
  );
}
