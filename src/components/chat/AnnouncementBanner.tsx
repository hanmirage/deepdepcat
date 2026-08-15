/**
 * AnnouncementBanner — 官网公告遥控的客户端展示端。
 *
 * 拉取 /api/site-config 的 announcement（经 Rust 侧原生 HTTP，无 CORS 问题），
 * 在 Code 聊天区顶部显示可关闭横幅；每 5 分钟自动刷新、回到前台时立即刷新，
 * 新公告（ID/标题/内容任一变化）额外发系统通知。
 * 同一公告 ID 关闭后不再重复打扰（localStorage 记忆，ID 变化即重新展示）。
 */

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertOctagon, AlertTriangle, Info, X } from "lucide-react";
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { cloudApi, type AnnouncementConfig } from "@/lib/tauri";
import { isTauri } from "@/lib/tauri";
import { useAuthStore } from "@/stores/authStore";
import { cn } from "@/lib/utils";

const DISMISS_KEY = "deepdepcat.announcement.dismissed";
const REFRESH_MS = 5 * 60 * 1000;

let permissionPromise: Promise<boolean> | null = null;

function ensurePermission(): Promise<boolean> {
  if (!permissionPromise) {
    permissionPromise = (async (): Promise<boolean> => {
      try {
        if (await isPermissionGranted()) return true;
        return (await requestPermission()) === "granted";
      } catch {
        return false;
      }
    })();
  }
  return permissionPromise;
}

function isDismissed(id: string): boolean {
  try {
    const list = JSON.parse(localStorage.getItem(DISMISS_KEY) ?? "[]");
    return Array.isArray(list) && list.includes(id);
  } catch {
    return false;
  }
}

function dismissId(id: string): void {
  try {
    const list = JSON.parse(localStorage.getItem(DISMISS_KEY) ?? "[]");
    if (!Array.isArray(list)) return;
    localStorage.setItem(DISMISS_KEY, JSON.stringify([...new Set([...list, id])]));
  } catch {
    // ignore — banner close is best-effort
  }
}

const LEVEL_STYLES: Record<
  AnnouncementConfig["level"],
  { box: string; icon: React.ReactNode; label: string }
> = {
  info: {
    box: "border-blue-500/30 bg-blue-500/10 text-blue-200",
    icon: <Info size={14} className="shrink-0" />,
    label: "chat.announcementInfo",
  },
  warning: {
    box: "border-amber-500/30 bg-amber-500/10 text-amber-200",
    icon: <AlertTriangle size={14} className="shrink-0" />,
    label: "chat.announcementWarning",
  },
  critical: {
    box: "border-red-500/30 bg-red-500/10 text-red-200",
    icon: <AlertOctagon size={14} className="shrink-0" />,
    label: "chat.announcementCritical",
  },
};

export function AnnouncementBanner() {
  const { t } = useTranslation();
  const serverUrl = useAuthStore((s) => s.serverUrl);
  const [announcement, setAnnouncement] = useState<AnnouncementConfig | null>(null);
  /** 已「见过」的公告指纹（id:title:message），用于新公告判定与通知去重 */
  const seenKey = useRef<string | null>(null);

  const fetchAnnouncement = useCallback(
    async (notify: boolean) => {
    if (!serverUrl) return;
      try {
        const cfg = await cloudApi.fetchSiteConfig(serverUrl);
        const a = cfg?.announcement;
        if (!a || !a.enabled || !a.id || isDismissed(a.id)) {
          setAnnouncement(null);
          return;
        }
        const key = `${a.id}:${a.title}:${a.message}`;
        const isNew = seenKey.current !== key;
        if (isNew) {
          seenKey.current = key;
          if (notify) void notifyAnnouncement(a);
        }
        setAnnouncement(a);
      } catch {
        // best-effort — 拉取失败保持现状
      }
    },
    [serverUrl],
  );

  useEffect(() => {
    void fetchAnnouncement(false);
    const timer = setInterval(() => void fetchAnnouncement(true), REFRESH_MS);
    const onVisible = () => {
      if (!document.hidden) void fetchAnnouncement(true);
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [fetchAnnouncement]);

  if (!announcement) return null;
  const style = LEVEL_STYLES[announcement.level] ?? LEVEL_STYLES.info;

  return (
    <div
      role="status"
      className={cn(
        "mx-3 mt-3 flex items-start gap-2 rounded-lg border px-3 py-2 text-xs",
        style.box,
      )}
    >
      {style.icon}
      <div className="min-w-0 flex-1">
        <p className="font-medium">
          {t(style.label)}
          {announcement.title ? `：${announcement.title}` : ""}
        </p>
        {announcement.message && (
          <p className="mt-0.5 whitespace-pre-wrap break-words opacity-90">{announcement.message}</p>
        )}
      </div>
      <button
        type="button"
        aria-label={t("chat.announcementDismiss")}
        onClick={() => {
          dismissId(announcement.id);
          setAnnouncement(null);
        }}
        className="shrink-0 rounded p-0.5 opacity-60 transition-opacity hover:opacity-100"
      >
        <X size={12} />
      </button>
    </div>
  );
}

/** 新公告 → 系统通知（仅桌面端；浏览器模式静默跳过）。 */
function notifyAnnouncement(a: AnnouncementConfig): void {
  if (!isTauri) return;
  void ensurePermission().then((granted) => {
    if (!granted) return;
    sendNotification({
      title: a.title ? `DeepDepCat 公告：${a.title}` : "DeepDepCat 公告",
      body: a.message.slice(0, 200),
    });
  });
}
