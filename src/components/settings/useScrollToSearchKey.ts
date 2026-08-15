import { useEffect } from "react";

/**
 * Scrolls a settings row into view and flashes it after a search result
 * navigates to its category. The row is found by the i18n key rendered in
 * SettingRow's `data-search-key` attribute; lazy pages may mount a frame or
 * two after the category switch, so the lookup retries for up to 90 frames.
 */
export function useScrollToSearchKey(
  focusSearchKey: string | null,
  resetKey: string,
): void {
  useEffect(() => {
    if (!focusSearchKey) return;
    let found: HTMLElement | null = null;
    let frameId = 0;
    let flashTimer = 0;
    let tries = 0;
    const tick = () => {
      if (found) return;
      const el = document.querySelector(
        `[data-search-key="${CSS.escape(focusSearchKey)}"]`,
      );
      if (el instanceof HTMLElement) {
        found = el;
        el.scrollIntoView({ behavior: "smooth", block: "center" });
        el.style.transition = "background-color 0.8s ease";
        el.style.backgroundColor = "hsl(var(--accent) / 0.45)";
        flashTimer = window.setTimeout(() => {
          el.style.backgroundColor = "";
          el.style.transition = "";
        }, 1800);
        return;
      }
      tries += 1;
      if (tries < 90) frameId = requestAnimationFrame(tick);
    };
    frameId = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(frameId);
      window.clearTimeout(flashTimer);
      if (found) {
        found.style.backgroundColor = "";
        found.style.transition = "";
      }
    };
  }, [focusSearchKey, resetKey]);
}
