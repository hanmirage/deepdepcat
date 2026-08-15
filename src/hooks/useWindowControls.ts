/**
 * Window controls hook — manages minimize/maximize/close for the
 * custom (frameless) title bar.
 *
 * Uses Tauri's onResized event instead of polling.
 */

import { useCallback, useEffect } from "react";
import { useAppStore } from "@/stores/appStore";
import { windowApi, isTauri } from "@/lib/tauri";
import { useTauriEvent } from "@/hooks/useTauriEvent";

export function useWindowControls() {
  const isMaximized = useAppStore((s) => s.isMaximized);
  const setIsMaximized = useAppStore((s) => s.setIsMaximized);

  useEffect(() => {
    if (!isTauri) return;

    let cancelled = false;

    // Sync initial state
    windowApi.isMaximized()
      .then((m) => { if (!cancelled) setIsMaximized(m); })
      .catch(() => {});

    return () => {
      cancelled = true;
    };
  }, [setIsMaximized]);

  // Listen for window resize events instead of polling.
  const subscribeResized = useCallback(
    (handler: () => void) => windowApi.onResized(handler),
    [],
  );
  useTauriEvent<void>(subscribeResized, () => {
    void windowApi
      .isMaximized()
      .then((maximized) => setIsMaximized(maximized))
      .catch(() => {
        // ignore
      });
  });

  const minimize = () => windowApi.minimize();
  const toggleMaximize = () => windowApi.toggleMaximize();
  const close = () => windowApi.close();

  return { isMaximized, minimize, toggleMaximize, close };
}
