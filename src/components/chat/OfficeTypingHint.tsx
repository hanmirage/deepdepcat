/**
 * OfficeTypingHint — small floating hint while the agent is typing into an
 * open WPS/Word window via the persistent office host.
 *
 * The backend emits `office-typing` events: `{active: true, ...}` when a
 * write starts (with chunk progress for live_doc_write), `{active: false}`
 * when it finishes. While active, a one-line hint appears above the input
 * telling the user to look at the office window; it fades out on its own.
 */

import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useTauriEvent } from "@/hooks/useTauriEvent";
import { cn } from "@/lib/utils";

interface OfficeTypingPayload {
  active: boolean;
  chunk?: number;
  total?: number;
  chars?: number;
  target?: string;
}

export interface OfficeTypingHintProps {
  className?: string;
}

export function OfficeTypingHint({ className }: OfficeTypingHintProps) {
  const { t } = useTranslation();
  const [typing, setTyping] = useState<OfficeTypingPayload | null>(null);
  const [leaving, setLeaving] = useState(false);
  const hiddenRef = useRef<boolean>(false);

  useTauriEvent<OfficeTypingPayload>("office-typing", (payload) => {
    if (payload.active) {
      hiddenRef.current = false;
      setLeaving(false);
      setTyping(payload);
    } else {
      // Let the last frame linger for a moment, then fade out.
      hiddenRef.current = true;
      setLeaving(true);
      window.setTimeout(() => {
        if (hiddenRef.current) {
          setLeaving(false);
          setTyping(null);
        }
      }, 700);
    }
  });

  if (!typing) return null;

  const progress =
    typing.total && typing.total > 0
      ? t("chat.typingSegments", {
          chunk: typing.chunk ?? 0,
          total: typing.total,
          defaultValue: " · {{chunk}}/{{total}} 段",
        })
      : "";
  const target = typing.target ? ` · ${typing.target}` : "";
  const chars =
    typeof typing.chars === "number" && typing.chars > 0
      ? t("chat.typingChars", { chars: typing.chars, defaultValue: " · {{chars}} 字" })
      : "";

  return (
    <div
      className={cn(
        "flex items-center gap-2 rounded-full border border-border/70 bg-card/95 px-3.5 py-1.5 text-[11px] text-muted-foreground shadow-paper-sm backdrop-blur",
        "animate-in fade-in slide-in-from-bottom duration-200",
        leaving && "animate-out fade-out duration-300",
        className,
      )}
    >
      <span className="relative flex h-1.5 w-1.5">
        <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-emerald-400 opacity-60" />
        <span className="relative inline-flex h-1.5 w-1.5 rounded-full bg-emerald-500" />
      </span>
      <span className="truncate">
        {t("officeTyping.hint", { defaultValue: "正在向 WPS 窗口打字" })}
        {progress}
        {chars}
        {target}
        {t("officeTyping.watchWindow", { defaultValue: "——请看打开的文档窗口" })}
      </span>
    </div>
  );
}
