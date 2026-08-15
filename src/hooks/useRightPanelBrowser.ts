/**
 * useRightPanelBrowser — agent-driven product-preview entry point.
 *
 * When the agent opens a preview (`dev_browser_open` → `dev-browser-open`
 * event), surface the PREVIEW pane (product rendering: generated HTML
 * reports) in the right panel of the CURRENT mode. This is deliberately
 * distinct from the real "browser" pane (agent's live Chromium via
 * `browser_control`) — the preview pane renders artifacts, the browser pane
 * mirrors the agent's real browser. The target is stashed per mode first —
 * the one-shot event may fire before the pane exists — then the pane opens
 * and consumes it on mount.
 */

import { useAppStore } from "@/stores/appStore";
import { useRightPanelStore } from "@/stores/rightPanelStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import type { DevBrowserOpenEvent } from "@/lib/tauri";

export function useRightPanelBrowser() {
  const mode = useAppStore((s) => s.mode);
  const openPane = useRightPanelStore((s) => s.openPane);
  const setPendingPreview = useRightPanelStore((s) => s.setPendingPreview);

  useTauriEvent<DevBrowserOpenEvent>("dev-browser-open", (payload) => {
    setPendingPreview(mode, { url: payload.url ?? null, path: payload.path ?? null });
    openPane(mode, "preview");
  });
}
