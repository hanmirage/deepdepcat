/**
 * BrowserLivePane — the "browser" pane: a LIVE mirror of the agent's real
 * Chromium browser (`browser_control`), streamed as screencast JPEG frames.
 *
 * Distinct from the "preview" pane (HtmlPreviewPane, which renders generated
 * HTML artifacts): this shows the real page the agent is driving — logins,
 * clicks, forms — as they happen. The user can watch (or, via the agent's
 * `handoff`, take over the real window). Frames stream only while this pane
 * is mounted; the pane is opened automatically by useAgentBrowserPane when
 * the agent starts a browser for the current session.
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Compass, Loader2 } from "lucide-react";
import type { AppMode } from "@/config/constants";
import { useChatStore } from "@/stores/chatStore";
import { useDepworkChatStore } from "@/stores/depworkChatStore";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import {
  browserApi,
  sessionBrowserProfile,
  BROWSER_SCREENCAST_FRAME_EVENT,
  type BrowserScreencastFrame,
  type BrowserStatus,
} from "@/lib/tauri";
import { cn } from "@/lib/utils";

/** Poll cadence for the URL/title header while the browser is live. */
const STATUS_POLL_MS = 2000;

interface BrowserLivePaneProps {
  mode: AppMode;
}

/** Header row — URL/title + live status dot (+ headless tag). */
function BrowserHeader({
  title,
  url,
  running,
  headless,
}: {
  title: string;
  url?: string | null;
  running: boolean;
  headless?: boolean;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex shrink-0 items-center gap-2 border-b border-border/60 bg-muted/30 px-3 py-1.5">
      <Compass className="h-3.5 w-3.5 shrink-0 text-muted-foreground/60" />
      <span
        className={cn(
          "h-2 w-2 shrink-0 rounded-full",
          running ? "bg-emerald-500" : "bg-muted-foreground/40",
        )}
      />
      <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-foreground/80" title={url ?? undefined}>
        {title}
      </span>
      {headless && (
        <span className="shrink-0 rounded bg-muted px-1 py-0.5 text-[9px] uppercase tracking-wide text-muted-foreground/60">
          {t("preview.headless", { defaultValue: "无头" })}
        </span>
      )}
    </div>
  );
}

export function BrowserLivePane({ mode }: BrowserLivePaneProps) {
  const { t } = useTranslation();
  const codeSessionId = useChatStore((s) => s.currentSessionId);
  const depworkSessionId = useDepworkChatStore((s) => s.currentSessionId);
  const sessionId = mode === "depwork" ? depworkSessionId : codeSessionId;
  const profile = sessionBrowserProfile(sessionId);

  const [frame, setFrame] = useState<BrowserScreencastFrame | null>(null);
  const [status, setStatus] = useState<BrowserStatus | null>(null);
  const [takeover, setTakeover] = useState(false);

  // Stream frames + poll status while mounted; stop on unmount.
  useEffect(() => {
    if (!profile) return;
    void browserApi.screencastStart(profile);
    let alive = true;
    const poll = () => {
      void browserApi.status(profile).then((s) => {
        if (alive) setStatus(s);
      }).catch(() => {});
    };
    poll();
    const iv = setInterval(poll, STATUS_POLL_MS);
    return () => {
      alive = false;
      clearInterval(iv);
      void browserApi.screencastStop(profile);
    };
  }, [profile]);

  useTauriEvent<BrowserScreencastFrame>(BROWSER_SCREENCAST_FRAME_EVENT, (e) => {
    if (!profile || e.profile !== profile) return;
    setFrame(e);
  });
  useTauriEvent<{ reason: string; profile?: string }>(
    "browser-takeover-requested",
    (e) => {
      if (!profile || (e.profile && e.profile !== profile)) return;
      setTakeover(true);
    },
  );
  useTauriEvent("browser-takeover-resumed", () => setTakeover(false));

  const running = status?.running ?? false;
  const title = status?.title || status?.url || t("preview.browserUntitled", { defaultValue: "未加载页面" });

  return (
    <div className="flex h-full min-h-0 flex-col">
      <BrowserHeader
        title={title}
        url={status?.url}
        running={running}
        headless={status?.headless}
      />

      {/* The live page — latest screencast frame */}
      <div className="relative min-h-0 flex-1 bg-white">
        {frame && running ? (
          <img
            src={`data:image/jpeg;base64,${frame.jpeg}`}
            alt={t("preview.browserLive", { defaultValue: "代理浏览器实时画面" })}
            className="h-full w-full object-contain"
          />
        ) : (
          <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
            <p className="text-[11px] text-muted-foreground">
              {running
                ? t("preview.browserWaitingFrame", { defaultValue: "等待浏览器画面…" })
                : t("preview.browserNotRunning", { defaultValue: "代理浏览器未运行" })}
            </p>
          </div>
        )}

        {/* Human-in-the-loop takeover banner */}
        {takeover && (
          <div className="absolute inset-x-0 top-0 z-10 flex items-center gap-2 bg-amber-500/90 px-3 py-2 text-[11px] font-medium text-amber-950">
            {t("takeover.title", { defaultValue: "代理浏览器需要你" })}
          </div>
        )}
      </div>
    </div>
  );
}
