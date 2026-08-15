/**
 * PermissionDialog — floating panel that rises from the chat input.
 *
 * When the backend emits a permission-request event, this dialog slides
 * up from the input bar area, showing:
 * - Tool icon + name + summary
 * - Expandable detail (JSON arguments)
 * - Three actions: Deny / Always allow / Allow
 *
 * Keyboard:
 * - Enter     → Allow
 * - Escape    → Deny
 * - Shift+Enter → Always allow
 *
 * The dialog is positioned absolutely, floating above the ChatInput.
 * A semi-transparent backdrop dims the message list to draw focus.
 */

import { useState, useEffect, useCallback, useMemo } from "react";
import { useTranslation } from "react-i18next";
import * as Collapsible from "@radix-ui/react-collapsible";
import {
  Check,
  X,
  ShieldCheck,
  ShieldAlert,
  ChevronRight,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { JsonHighlight } from "@/components/chat/JsonHighlight";
import { usePermissionStore, useCurrentPermissionRequest } from "@/stores/permissionStore";
import { useFocusTrap } from "@/hooks/useFocusTrap";
import { getToolIcon } from "@/config/toolIcons";
import { cn, isEditableKeyEvent } from "@/lib/utils";

/** Always-allow scope choice — shown only when a real choice exists
 *  (path / MCP server vs whole tool). The user picks explicitly before
 *  anything is remembered; a bare click never expands a grant's scope. */
function AlwaysScopeRow({
  grantScope,
  onExact,
  onWholeTool,
  onCancel,
}: {
  grantScope: string;
  onExact: () => void;
  onWholeTool: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="mx-3.5 mt-2 rounded-md border border-border bg-muted/30 p-2">
      <p className="text-[11px] text-muted-foreground">
        {t("permission.alwaysWillRemember", { defaultValue: "「始终允许」将记住" })}：
        {grantScope}
      </p>
      <div className="mt-1.5 flex flex-wrap gap-1.5">
        <Button
          variant="outline"
          size="sm"
          className="text-[11px]"
          onClick={onExact}
        >
          {t("permission.alwaysExact", { defaultValue: "仅本次范围" })}
        </Button>
        <Button
          variant="outline"
          size="sm"
          className="text-[11px] text-primary"
          onClick={onWholeTool}
        >
          {t("permission.alwaysWholeTool", { defaultValue: "整个工具" })}
        </Button>
        <Button
          variant="ghost"
          size="sm"
          className="text-[11px] text-muted-foreground"
          onClick={onCancel}
        >
          {t("permission.cancel", { defaultValue: "取消" })}
        </Button>
      </div>
    </div>
  );
}

/** Deny-with-reason — optional feedback fed back to the agent so a
 *  denied call explains itself instead of being retried blindly. */
function DenyReasonRow({
  value,
  onChange,
  onConfirm,
  onCancel,
}: {
  value: string;
  onChange: (value: string) => void;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  return (
    <div className="mx-3.5 mt-2 rounded-md border border-border bg-muted/30 p-2">
      <textarea
        value={value}
        onChange={(e) => onChange(e.target.value)}
        rows={2}
        className="w-full resize-none rounded-md border border-border bg-background/60 p-2 font-mono text-[11px] text-foreground outline-none focus:border-primary/50"
        placeholder={t("permission.denyReasonPlaceholder", {
          defaultValue: "为什么拒绝？（可选，将反馈给 agent）",
        })}
        autoFocus
      />
      <div className="mt-1.5 flex justify-end gap-1.5">
        <Button
          variant="ghost"
          size="sm"
          className="text-[11px] text-muted-foreground"
          onClick={onCancel}
        >
          {t("permission.cancel", { defaultValue: "取消" })}
        </Button>
        <Button
          variant="destructive"
          size="sm"
          className="text-[11px]"
          onClick={onConfirm}
        >
          {t("permission.confirmDeny", { defaultValue: "确认拒绝" })}
        </Button>
      </div>
    </div>
  );
}

export function PermissionDialog({ sessionId }: { sessionId?: string | null }) {
  const { t } = useTranslation();
  const pendingRequest = useCurrentPermissionRequest(sessionId);
  const respond = usePermissionStore((s) => s.respond);
  const pruneExpired = usePermissionStore((s) => s.pruneExpired);
  const [argsOpen, setArgsOpen] = useState(false);
  // "Always allow" has two granularities: the exact pattern shown in the
  // dialog (path / bash first word / MCP server) or the whole tool (`*`).
  // The user picks explicitly before anything is remembered — a bare
  // click never silently expands a grant's scope.
  const [alwaysScopeOpen, setAlwaysScopeOpen] = useState(false);
  // Reject can carry an optional reason; it is fed back to the agent so a
  // denied call explains itself instead of being retried blindly.
  const [denyReasonOpen, setDenyReasonOpen] = useState(false);
  const [denyReason, setDenyReason] = useState("");

  // ── Keyboard shortcuts ──────────────────────────────────────
  // Guarded: keys typed into inputs/textareas (or during IME composition)
  // are typing, not shortcuts — Enter in the chat box must not approve a
  // tool call that happened to queue while the user was writing.
  const dialogRef = useFocusTrap<HTMLDivElement>(!!pendingRequest);
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (!pendingRequest) return;
      if (isEditableKeyEvent(e)) return;
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        if (alwaysScopeOpen) {
          void respond("always_allow", { scope: "pattern" }, pendingRequest.request_id);
          setAlwaysScopeOpen(false);
        } else if (denyReasonOpen) {
          void respond("deny", { reason: denyReason.trim() || undefined }, pendingRequest.request_id);
          setDenyReasonOpen(false);
        } else {
          void respond("allow", undefined, pendingRequest.request_id);
        }
      } else if (e.key === "Escape") {
        e.preventDefault();
        if (alwaysScopeOpen) {
          setAlwaysScopeOpen(false);
        } else if (denyReasonOpen) {
          setDenyReasonOpen(false);
        } else {
          void respond("deny", undefined, pendingRequest.request_id);
        }
      } else if (e.key === "Enter" && e.shiftKey) {
        e.preventDefault();
        void respond("always_allow", { scope: "pattern" }, pendingRequest.request_id);
      }
    },
    [pendingRequest, respond, alwaysScopeOpen, denyReasonOpen, denyReason],
  );

  useEffect(() => {
    if (!pendingRequest) return;
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [pendingRequest, handleKeyDown]);

  // Reset expand state when a new request arrives
  useEffect(() => {
    setArgsOpen(false);
    setAlwaysScopeOpen(false);
    setDenyReasonOpen(false);
    setDenyReason("");
  }, [pendingRequest?.request_id]);

  // Switching to a different conversation: drop requests the backend already
  // timed out on (they're dead — the dialog would otherwise surface a stale
  // permission the agent gave up on).
  useEffect(() => {
    pruneExpired();
  }, [sessionId, pruneExpired]);

  // Sensitive-file guard. The summary/detail values and the useMemo MUST
  // stay above the early return below: React requires a constant hook
  // count across renders, and the request queue going empty → non-empty
  // would otherwise flip the hook count and crash the whole app (white
  // screen — there is no error boundary).
  const summary = pendingRequest?.summary ?? pendingRequest?.args_summary;
  const detail = pendingRequest?.detail ?? pendingRequest?.args_summary;
  const sensitiveTarget = useMemo(() => {
    const haystack = `${summary ?? ""} ${detail ?? ""}`.toLowerCase();
    return (
      /(^|[\\/])\.env([.\w-]*)?([\\/\s]|$)/.test(haystack) ||
      /\.(pem|key|pfx)([\s"']|$)/.test(haystack) ||
      /(^|[\\/])(id_rsa|id_ed25519|id_dsa|id_ecdsa|\.git-credentials|\.netrc)([\s"']|$)/.test(
        haystack,
      ) ||
      /(credentials|secrets?|tokens?)\.\w+/.test(haystack)
    );
  }, [summary, detail]);

  if (!pendingRequest) return null;

  const Icon = getToolIcon(pendingRequest.tool_name);
  // Native path sends args_summary; the ACP path sends summary/detail.
  const agentName = pendingRequest.agent_name ?? "Agent";

  return (
    <>
      {/* ── Permission card — paper-cut. Anchored by ChatViewShell directly
          above the chat input (no fixed offsets — the input's height varies
          with chips/queue notices, and the card must never cover it). */}
      <div
        ref={dialogRef}
        className="relative z-40"
        role="dialog"
        aria-modal="true"
        aria-label={t("permission.title")}
      >
        <div className="decision-card animate-in slide-in-from-bottom-3 fade-in duration-200">
          {/* ── Sensitive-file warning ─────────────────────────────── */}
          {sensitiveTarget && (
            <div className="flex items-center gap-2 border-b border-destructive/30 bg-destructive/5 px-4 py-2">
              <ShieldAlert className="h-3.5 w-3.5 shrink-0 text-destructive" />
              <span className="text-[11px] text-destructive/90">
                {t("permission.sensitiveFile", {
                  defaultValue:
                    "正在修改敏感文件（密钥/凭据类）——批准前请确认变更内容",
                })}
              </span>
            </div>
          )}
          {/* ── Tool info ─────────────────────────────────────── */}
          <Collapsible.Root open={argsOpen} onOpenChange={setArgsOpen}>
            <div className="flex items-center gap-3 px-4 pt-3.5">
              <div className="decision-icon">
                <Icon className="h-4 w-4" />
              </div>
              <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2">
                  <p className="truncate font-mono text-sm font-medium">{pendingRequest.tool_name}</p>
                  <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[9px] font-medium uppercase tracking-wider text-muted-foreground">
                    {agentName}
                  </span>
                  {pendingRequest.parent_session_id &&
                    pendingRequest.parent_session_id !== pendingRequest.session_id && (
                      <span className="shrink-0 rounded bg-amber-500/10 px-1.5 py-0.5 text-[9px] font-medium text-amber-700 dark:text-amber-400">
                        子代理
                      </span>
                    )}
                </div>
                {summary && (
                  <p className="mt-0.5 truncate text-xs text-muted-foreground">{summary}</p>
                )}
              </div>

              {/* ── Expandable arguments toggle ────────────────── */}
              {detail && (
                <Collapsible.Trigger asChild>
                  <button className="flex shrink-0 items-center gap-1 rounded-md px-2 py-1 text-[11px] text-muted-foreground transition-colors hover:bg-muted/50 hover:text-foreground">
                    <ChevronRight
                      className={cn(
                        "h-3 w-3 transition-transform",
                        argsOpen && "rotate-90",
                      )}
                    />
                    {argsOpen ? t("permission.collapseArgs") : t("permission.viewArgs")}
                  </button>
                </Collapsible.Trigger>
              )}
            </div>

            {detail && (
              <Collapsible.Content>
                <pre className="mx-3.5 mb-2 mt-2 max-h-48 overflow-auto rounded-md border border-border bg-muted/40 p-2 font-mono text-[11px] text-muted-foreground">
                  <JsonHighlight json={detail} />
                </pre>
              </Collapsible.Content>
            )}
          </Collapsible.Root>

          {/* ── Always-allow scope choice — only when a real choice exists
              (path / MCP server vs whole tool). Whole-tool tools (`*`) and
              bash (first-word only) respond immediately. */}
          {alwaysScopeOpen && pendingRequest.grant_scope && (
            <AlwaysScopeRow
              grantScope={pendingRequest.grant_scope}
              onExact={() =>
                void respond("always_allow", { scope: "pattern" }, pendingRequest.request_id)
              }
              onWholeTool={() =>
                void respond("always_allow", { scope: "tool" }, pendingRequest.request_id)
              }
              onCancel={() => setAlwaysScopeOpen(false)}
            />
          )}

          {/* ── Deny-with-reason — optional feedback fed back to the agent. */}
          {denyReasonOpen && (
            <DenyReasonRow
              value={denyReason}
              onChange={setDenyReason}
              onConfirm={() => {
                void respond(
                  "deny",
                  { reason: denyReason.trim() || undefined },
                  pendingRequest.request_id,
                );
                setDenyReasonOpen(false);
              }}
              onCancel={() => setDenyReasonOpen(false)}
            />
          )}

          {/* ── Action strip — the lower sheet of paper. flex-wrap so the
              three actions never squeeze the tool info on narrow windows. */}
          <div className="decision-card-footer mt-2 flex-wrap">
            <Button
              variant="ghost"
              size="sm"
              className="gap-1.5 border border-destructive/30 text-xs text-destructive hover:bg-destructive/10 hover:text-destructive"
              onClick={() => {
                setDenyReasonOpen((v) => !v);
                setAlwaysScopeOpen(false);
              }}
            >
              <X className="h-3.5 w-3.5" />
              {t("permission.deny")}
              <Kbd className="border-destructive/30 text-destructive/70">Esc</Kbd>
            </Button>
            <div className="flex-1" />
            <Button
              variant="outline"
              size="sm"
              className="gap-1.5 border-primary/40 text-xs text-primary hover:bg-primary/10 hover:text-primary"
              onClick={() => {
                // Fast path: whole-tool grants (already `*`) and bash
                // (first-word only) have no meaningful scope choice; ACP
                // requests without grant metadata also respond directly.
                const needsScopeChoice =
                  !!pendingRequest.grant_scope &&
                  pendingRequest.grant_pattern !== "*" &&
                  pendingRequest.tool_name !== "bash";
                if (!needsScopeChoice) {
                  void respond(
                    "always_allow",
                    { scope: "pattern" },
                    pendingRequest.request_id,
                  );
                  return;
                }
                setAlwaysScopeOpen((v) => !v);
                setDenyReasonOpen(false);
              }}
            >
              <ShieldCheck className="h-3.5 w-3.5" />
              {t("permission.alwaysAllow")}
              <Kbd className="border-primary/30 text-primary/70">⇧↵</Kbd>
            </Button>
            <Button
              size="sm"
              className="gap-1.5 text-xs"
              onClick={() => void respond("allow", undefined, pendingRequest.request_id)}
              autoFocus={!document.activeElement || document.activeElement === document.body}
            >
              <Check className="h-3.5 w-3.5" />
              {t("permission.allow")}
              <Kbd className="border-primary-foreground/30 text-primary-foreground/70">↵</Kbd>
            </Button>
          </div>
        </div>
      </div>
    </>
  );
}
