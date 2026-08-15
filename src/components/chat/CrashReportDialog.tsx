/**
 * CrashReportDialog — shows after a crash, asks the user how to report it.
 *
 * Privacy-first: DeepDepCat 非常尊重您的隐私. Nothing is sent to the server
 * unless the user explicitly picks one of the two opt-in options here:
 *   1. Send only the error code (panic message + backtrace + system info)
 *   2. Also attach a JSON export of the current conversation
 * Or the user can dismiss without sending anything.
 */

import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, Loader2, Send, ShieldCheck } from "lucide-react";
import { useTranslation } from "react-i18next";
import { crashApi, type PendingCrash } from "@/lib/tauri";
import { useSettingsStore } from "@/stores/settingsStore";
import { useChatStore } from "@/stores/chatStore";
import { useSessionRestore } from "@/hooks/useSessionRestore";
import {
  startSessionTracking,
  prepareCrashRecovery,
} from "@/lib/sessionTracker";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

type SendState = "idle" | "sending" | "sent" | "error";

export function CrashReportDialog() {
  const { t } = useTranslation();
  const serverUrl = useSettingsStore((s) => s.general.serverUrl);
  const currentSessionId = useChatStore((s) => s.currentSessionId);
  const { selectSessionById } = useSessionRestore();
  const [pending, setPending] = useState<PendingCrash | null>(null);
  const [includeConversation, setIncludeConversation] = useState(false);
  const [sendState, setSendState] = useState<SendState>("idle");
  const [error, setError] = useState<string | null>(null);
  /** True once the pre-crash session has been restored into the chat store. */
  const [restored, setRestored] = useState(false);

  useEffect(() => {
    let cancelled = false;
    // Ensure the tracker is registered before recovery re-activates a session
    // (idempotent — initSystem usually already registered it).
    startSessionTracking();
    crashApi
      .getPending()
      .then(async (p) => {
        if (cancelled || !p) return;
        setPending(p);
        const recovered = await prepareCrashRecovery(selectSessionById);
        if (!cancelled) setRestored(recovered);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [selectSessionById]);

  const handleSend = useCallback(async () => {
    setSendState("sending");
    setError(null);
    try {
      // If the user opted into sharing the conversation but there's no
      // active session, tell them plainly rather than silently sending
      // without the conversation.
      let conversationJson: string | null = null;
      if (includeConversation) {
        if (!currentSessionId) {
          setSendState("error");
          setError(t("crashDialog.noSessionToShare", "当前没有可导出的对话，已只发送报错代码"));
          // Still send the bare crash — don't block on the conversation.
        } else {
          conversationJson = await crashApi.exportSessionConversation(currentSessionId);
        }
      }
      await crashApi.submit(serverUrl, includeConversation && conversationJson !== null, conversationJson);
      // Sending succeeded — dismiss the dialog and clear the pending payload.
      setPending(null);
      crashApi.dismissPending().catch(() => {});
    } catch (e) {
      setSendState("error");
      setError(String(e));
    }
  }, [includeConversation, currentSessionId, serverUrl, t]);

  const handleDismiss = useCallback(() => {
    crashApi.dismissPending().catch(() => {});
    setPending(null);
  }, []);

  if (!pending) return null;

  return (
    <Dialog open={!!pending} onOpenChange={(open) => !open && handleDismiss()}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <AlertTriangle className="h-4 w-4 text-amber-500" />
            {t("crashDialog.title", "抱歉，DeepDepCat 遇到了意外崩溃")}
          </DialogTitle>
          <DialogDescription>
            {t(
              "crashDialog.privacy",
              "DeepDepCat 非常尊重您的隐私。您可以选择是否帮助我们发现并修复这个问题——以下内容都不会自动发送。",
            )}
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-3 py-1">
          {restored && (
            <p className="text-xs text-muted-foreground">
              {t("crashDialog.restored", "已恢复上次会话。")}
            </p>
          )}

          {/* Option 1: error code only */}
          <button
            type="button"
            disabled={sendState === "sending" || sendState === "sent"}
            onClick={() => {
              setIncludeConversation(false);
              setSendState("idle");
            }}
            className={`flex w-full items-start gap-3 rounded-lg border p-3 text-left transition-colors disabled:opacity-50 ${
              !includeConversation
                ? "border-primary/60 bg-primary/5"
                : "border-border hover:bg-muted/40"
            }`}
          >
            <ShieldCheck
              className={`mt-0.5 h-4 w-4 shrink-0 ${!includeConversation ? "text-primary" : "text-muted-foreground"}`}
            />
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium">
                {t("crashDialog.optionErrorOnly", "仅发送报错代码")}
              </p>
              <p className="text-xs text-muted-foreground">
                {t(
                  "crashDialog.optionErrorOnlyDesc",
                  "发送崩溃信息（报错内容、系统环境）帮助我们定位问题。不包含您的对话内容。",
                )}
              </p>
            </div>
          </button>

          {/* Option 2: error + conversation */}
          <button
            type="button"
            disabled={sendState === "sending" || sendState === "sent"}
            onClick={() => {
              setIncludeConversation(true);
              setSendState("idle");
            }}
            className={`flex w-full items-start gap-3 rounded-lg border p-3 text-left transition-colors disabled:opacity-50 ${
              includeConversation
                ? "border-primary/60 bg-primary/5"
                : "border-border hover:bg-muted/40"
            }`}
          >
            <Send
              className={`mt-0.5 h-4 w-4 shrink-0 ${includeConversation ? "text-primary" : "text-muted-foreground"}`}
            />
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium">
                {t("crashDialog.optionWithConversation", "携带 JSON 对话文件")}
              </p>
              <p className="text-xs text-muted-foreground">
                {t(
                  "crashDialog.optionWithConversationDesc",
                  "在报错代码基础上，额外附上本次对话内容（含工具调用），便于我们完整复现问题。仅在您选择时才会发送。",
                )}
              </p>
            </div>
          </button>

          {sendState === "sent" ? (
            <p className="text-xs text-emerald-600">
              {t("crashDialog.sent", "已发送，感谢您的反馈！")}
            </p>
          ) : sendState === "error" ? (
            <p className="text-xs text-destructive">
              {t("crashDialog.sendError", "发送失败：")} {error}
            </p>
          ) : null}
        </div>

        <div className="flex items-center justify-between gap-2 pt-1">
          <Button variant="ghost" size="sm" onClick={handleDismiss} disabled={sendState === "sending"}>
            {t("crashDialog.notNow", "暂不发送")}
          </Button>
          <Button
            size="sm"
            onClick={() => void handleSend()}
            disabled={sendState === "sending" || sendState === "sent"}
          >
            {sendState === "sending" ? (
              <>
                <Loader2 className="mr-1.5 h-3.5 w-3.5 animate-spin" />
                {t("crashDialog.sending", "发送中…")}
              </>
            ) : (
              t("crashDialog.send", "发送报告")
            )}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
