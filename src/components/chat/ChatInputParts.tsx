/**
 * ChatInput parts — extracted display-only chunks (queued notice,
 * no-model notice, streaming controls) to keep ChatInput readable.
 */

import { useTranslation } from "react-i18next";
import { Clock, Zap, X, Settings2, ArrowUp, Pause, Play, Square } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { cn } from "@/lib/utils";

export function QueuedNotice({
  queuedText,
  onSendNow,
  onClear,
}: {
  queuedText: string;
  onSendNow: () => void;
  onClear: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-1.5 px-3 pt-2">
      <span className="flex max-w-full items-center gap-1.5 rounded-full border border-primary/30 bg-primary/10 px-2 py-0.5 text-[10px] text-primary">
        <Clock className="h-3 w-3 shrink-0" />
        <span className="truncate">
          {t("chat.queuedNotice", "已排队，回复结束后自动发送")}：{queuedText}
        </span>
      </span>
      <button
        onClick={onSendNow}
        className="flex h-5 shrink-0 items-center gap-1 rounded-full border border-primary/30 bg-primary/10 px-2 text-[10px] font-medium text-primary transition-colors hover:bg-primary/20"
        title={t("chat.interruptSendDesc", "停止当前回复，立刻发送这条消息")}
      >
        <Zap className="h-2.5 w-2.5" />
        {t("chat.queuedSendNow", "立即发送")}
      </button>
      <button
        onClick={onClear}
        className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        aria-label={t("common.cancel")}
      >
        <X className="h-3 w-3" />
      </button>
    </div>
  );
}

export function NoModelNotice({
  attention,
  onConfigure,
}: {
  attention: boolean;
  onConfigure: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex items-center gap-2 px-3 pt-2">
      <span
        className={cn(
          "flex min-w-0 flex-1 items-center gap-1.5 rounded-lg border bg-primary/5 px-2.5 py-1.5 text-[11px] text-foreground/80",
          attention
            ? "animate-[setup-pulse_0.8s_ease-in-out_2] border-primary/60"
            : "border-primary/25",
        )}
      >
        <Settings2 className="h-3.5 w-3.5 shrink-0 text-primary" />
        <span className="truncate">
          {t("chat.noModelBanner", "尚未配置模型——添加 API Key 后即可开始使用")}
        </span>
      </span>
      <Button
        size="sm"
        variant="outline"
        className="h-6 shrink-0 gap-1 border-primary/30 px-2 text-[10px] text-primary hover:bg-primary/10 hover:text-primary"
        onClick={onConfigure}
      >
        {t("chat.configureModel", "去配置")}
      </Button>
    </div>
  );
}

export function StreamControls({
  hasText,
  isPaused,
  onQueue,
  onInterrupt,
  onPauseResume,
  onStop,
}: {
  hasText: boolean;
  isPaused: boolean;
  onQueue: () => void;
  onInterrupt: () => void;
  onPauseResume: () => void;
  onStop: () => void;
}) {
  const { t } = useTranslation();
  return (
    <>
      {hasText && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button
              size="icon"
              className="h-7 w-7 shrink-0 rounded-full bg-primary text-primary-foreground shadow-md shadow-primary/30 transition-all duration-200 hover:scale-105"
              aria-label={t("chat.busySendOptions", "排队或打断发送")}
            >
              <ArrowUp className="h-3 w-3" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-56">
            <DropdownMenuItem className="flex items-start gap-2 py-2 text-xs" onClick={onQueue}>
              <Clock className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              <div className="flex-1">
                <p className="font-medium">{t("chat.queueSend", "排队发送")}</p>
                <p className="text-[10px] text-muted-foreground">
                  {t("chat.queueSendDesc", "等当前回复结束后自动发送")}
                </p>
              </div>
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem className="flex items-start gap-2 py-2 text-xs" onClick={onInterrupt}>
              <Zap className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" />
              <div className="flex-1">
                <p className="font-medium text-destructive">
                  {t("chat.interruptSend", "立即打断并发送")}
                </p>
                <p className="text-[10px] text-muted-foreground">
                  {t("chat.interruptSendDesc", "停止当前回复，立刻发送这条消息")}
                </p>
              </div>
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      )}
      <Button
        size="icon"
        variant="ghost"
        className="h-7 w-7 shrink-0 rounded-full text-foreground transition-all duration-200 hover:scale-105 hover:bg-muted"
        onClick={onPauseResume}
        aria-label={t(isPaused ? "chat.resume" : "chat.pause")}
        title={t(isPaused ? "chat.resume" : "chat.pause")}
      >
        {isPaused ? (
          <Play className="h-3 w-3 fill-current" />
        ) : (
          <Pause className="h-3 w-3 fill-current" />
        )}
      </Button>
      <Button
        size="icon"
        className="h-7 w-7 shrink-0 rounded-full bg-destructive text-destructive-foreground shadow-md shadow-destructive/30 transition-all duration-200 hover:scale-105"
        onClick={onStop}
        aria-label={t("common.stop")}
      >
        <Square className="h-3 w-3 fill-current" />
      </Button>
    </>
  );
}
