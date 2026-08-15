/**
 * ErrorBlock — turn-level failure banner (backend StreamEvent::Error).
 *
 * Rendered as a distinct destructive-styled banner so an error is never
 * mistaken for model output. Includes the message text (plain, no markdown)
 * plus a copy action.
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Copy, Check } from "lucide-react";
import { cn } from "@/lib/utils";

export function ErrorBlock({ content }: { content: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(content);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard unavailable — the error text stays selectable.
    }
  };

  return (
    <div
      role="alert"
      className="flex items-start gap-2.5 rounded-lg border border-destructive/40 bg-destructive/5 px-3 py-2.5"
    >
      <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0 text-destructive" />
      <div className="min-w-0 flex-1">
        <p className="text-xs font-medium text-destructive">{t("chat.errorTitle")}</p>
        {/* Long backend errors must never blow up the message column. */}
        <p className="mt-0.5 max-h-40 overflow-auto break-words text-xs text-destructive/90">
          {content}
        </p>
      </div>
      <button
        onClick={() => void handleCopy()}
        className={cn(
          "mt-0.5 flex h-6 items-center gap-1 rounded px-1.5 text-[11px] transition-colors",
          copied
            ? "text-green-600"
            : "text-muted-foreground hover:bg-muted hover:text-foreground",
        )}
        title={copied ? t("chat.copied") : t("chat.copy")}
      >
        {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
        {copied ? t("chat.copied") : t("chat.copy")}
      </button>
    </div>
  );
}
