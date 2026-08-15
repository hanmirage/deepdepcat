/**
 * ElicitationDialog — modal for MCP server-initiated input requests.
 *
 * When an MCP server sends an `elicitation/create` request, the backend
 * emits a `StreamEvent::Elicitation` event. This dialog captures the
 * user's response and sends it back via `elicitationApi.respond()`.
 *
 * Location: rendered at the chat view root, above the message list.
 * Design: part of the decision-card family — neutral card + primary accent.
 */

import { useEffect, useState, useCallback } from "react";
import { useTranslation } from "react-i18next";
import { X, Send, MessageSquare, Plug } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Kbd } from "@/components/ui/kbd";
import { cn } from "@/lib/utils";

export interface ElicitationDialogProps {
  /** Unique ID for this elicitation request. */
  elicitationId: string;
  /** Name of the MCP server that issued the request. */
  serverName: string;
  /** Human-readable message from the server. */
  message: string;
  /** Sends the response to the backend and clears the pending request.
   *  The dialog must NOT send directly AND via onClose — that double-sends. */
  respond: (
    elicitationId: string,
    action: "accept" | "decline" | "cancel",
    content?: unknown,
  ) => Promise<void>;
  className?: string;
}

export function ElicitationDialog({
  elicitationId,
  serverName,
  message,
  respond,
  className,
}: ElicitationDialogProps) {
  const { t } = useTranslation();
  const [value, setValue] = useState("");
  const [submitting, setSubmitting] = useState(false);

  // Each action sends exactly ONE response through the store action, which
  // also clears pendingElicitation (unmounting this dialog). No onClose
  // cancel round-trip. `send` is invoked via `void send(...)` — it MUST
  // swallow its own rejection, otherwise an invoke failure surfaces as an
  // unhandled rejection with no user feedback.
  const send = useCallback(
    async (action: "accept" | "decline" | "cancel", content?: unknown) => {
      setSubmitting(true);
      try {
        await respond(elicitationId, action, content);
      } catch {
        // Invoke failure — keep the dialog open so the user can retry.
      } finally {
        setSubmitting(false);
      }
    },
    [elicitationId, respond],
  );

  const handleAccept = useCallback(
    () => void send("accept", value),
    [send, value],
  );

  const handleDecline = useCallback(() => void send("decline"), [send]);

  const handleCancel = useCallback(() => void send("cancel"), [send]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        // Match the button's disabled guard — Enter on an empty input must
        // not send a blank accept.
        if (value.trim()) handleAccept();
      }
    },
    [handleAccept, value],
  );

  // Esc = cancel the request (true modal — Esc must close it like every
  // other decision surface).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        handleCancel();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [handleCancel]);

  return (
    <div className={cn("fixed inset-0 z-50 flex items-center justify-center bg-black/50", className)}>
      <div className="decision-card w-full max-w-md animate-in fade-in zoom-in-95 duration-200">
        {/* Header */}
        <div className="flex items-center gap-2.5 px-4 pt-3.5">
          <div className="decision-icon">
            <MessageSquare className="h-4 w-4" />
          </div>
          <span className="text-sm font-semibold">
            {serverName}
          </span>
          <span className="ml-auto flex shrink-0 items-center gap-1 rounded bg-muted px-1.5 py-0.5 text-[9px] font-medium uppercase tracking-wider text-muted-foreground">
            <Plug className="h-2.5 w-2.5" />
            MCP
          </span>
          <Button variant="ghost" size="icon-sm" onClick={handleCancel} disabled={submitting}>
            <X className="h-3.5 w-3.5" />
          </Button>
        </div>

        {/* Message */}
        <p className="mb-3 mt-2 px-4 text-sm leading-relaxed text-foreground">
          {message}
        </p>

        {/* Input */}
        <div className="px-4">
          <Input
            value={value}
            onChange={(e) => setValue(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder={t("chat.elicitationPlaceholder")}
            className="h-9 text-sm"
            autoFocus
            disabled={submitting}
          />
        </div>

        {/* Actions — the lower sheet of paper */}
        <div className="decision-card-footer mt-3">
          <span className="flex items-center gap-1 text-[10px] text-muted-foreground/60">
            {t("chat.elicitationKbdHint", "Enter 发送")}
            <Kbd className="border-border/60 text-muted-foreground/60">↵</Kbd>
          </span>
          <div className="flex-1" />
          <Button
            variant="ghost"
            size="sm"
            onClick={handleCancel}
            disabled={submitting}
          >
            {t("chat.elicitationCancel")}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="border border-destructive/30 text-destructive hover:bg-destructive/10 hover:text-destructive"
            onClick={handleDecline}
            disabled={submitting}
          >
            {t("chat.elicitationDecline")}
          </Button>
          <Button
            size="sm"
            onClick={handleAccept}
            disabled={submitting || !value}
            className="gap-1.5"
          >
            <Send className="h-3 w-3" />
            {t("chat.elicitationAccept")}
          </Button>
        </div>
      </div>
    </div>
  );
}
