/**
 * Debug events hook — subscribes to backend debug events.
 *
 * When `debugMode` is enabled in appStore, this hook listens for
 * "debug-event" Tauri events and feeds them into `debugStore`.
 * When `debugMode` is off, the listener is torn down.
 */

import { useAppStore } from "@/stores/appStore";
import { useDebugStore } from "@/stores/debugStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { DebugEvent } from "@/types";

export function useDebugEvents() {
  const debugMode = useAppStore((s) => s.debugMode);
  const addEvent = useDebugStore((s) => s.addEvent);

  useTauriEvent<DebugEvent>("debug-trace", (event) => {
    addEvent(event);
  }, debugMode);
}
