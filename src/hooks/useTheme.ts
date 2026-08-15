/**
 * Theme hook — initializes and toggles dark/light mode + UI accent hue.
 *
 * `theme` flips the `.dark` class; `accent` sets `data-accent` on <html>,
 * which drives the accent CSS overrides in index.css. Both are applied here
 * so a refresh/restart restores the persisted choices.
 */

import { useEffect } from "react";
import { useAppStore } from "@/stores/appStore";

export function useTheme() {
  const theme = useAppStore((s) => s.theme);
  const accent = useAppStore((s) => s.accent);
  const toggleTheme = useAppStore((s) => s.toggleTheme);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", theme === "dark");
  }, [theme]);

  useEffect(() => {
    document.documentElement.dataset.accent = accent;
  }, [accent]);

  return { theme, toggleTheme };
}
