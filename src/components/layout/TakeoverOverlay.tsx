/**
 * TakeoverOverlay — the agent-browser human-in-the-loop banner, decoupled
 * from the (now Claude-Preview-style) preview pane.
 *
 * When the agent's real browser hits a wall it can't cross (captcha / login),
 * the backend pauses it and emits `browser-takeover-requested`. This floating
 * banner tells the user the real browser window is waiting; clicking resume
 * tells the backend to continue. It is deliberately a tiny overlay, NOT a
 * pane — the preview pane stays a clean canvas.
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Bot } from "lucide-react";
import { browserApi, type BrowserTakeoverRequest } from "@/lib/tauri";
import { useTauriEvent } from "@/hooks/useTauriEvent";

export function TakeoverOverlay() {
  const { t } = useTranslation();
  const [active, setActive] = useState<BrowserTakeoverRequest | null>(null);

  useTauriEvent<BrowserTakeoverRequest>("browser-takeover-requested", (e) => {
    setActive(e);
  });
  useTauriEvent("browser-takeover-resumed", () => {
    setActive(null);
  });

  if (!active) return null;

  const resume = () => {
    void browserApi.resume().then(() => setActive(null));
  };

  return (
    <div className="fixed bottom-4 right-4 z-50 flex items-center gap-3 rounded-lg border border-border bg-card p-3 shadow-lg">
      <Bot className="h-4 w-4 shrink-0 text-primary" />
      <div className="min-w-0">
        <p className="text-[11px] font-medium">
          {t("takeover.title", { defaultValue: "代理浏览器需要你" })}
        </p>
        <p className="truncate text-[10px] text-muted-foreground">{active.reason}</p>
      </div>
      <button
        onClick={resume}
        className="shrink-0 rounded-md bg-primary px-3 py-1.5 text-[11px] font-medium text-primary-foreground"
      >
        {t("takeover.resume", { defaultValue: "我已接管完成，继续" })}
      </button>
    </div>
  );
}
