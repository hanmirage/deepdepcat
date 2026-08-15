/**
 * McpServerCard — a single MCP server row with status, actions, and
 * expandable tool list.
 *
 * Shows: server name, transport type, connection status badge,
 * connect/disconnect button, remove button. When connected and
 * expanded, displays the list of tools exposed by the server.
 */

import { useState, useCallback } from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronDown,
  ChevronRight,
  KeyRound,
  Plug,
  PlugZap,
  Trash2,
  Loader2,
  Wrench,
  AlertTriangle,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import { cn } from "@/lib/utils";
import { analyzeMcpError } from "@/lib/mcpError";
import type { McpServerWithStatus, McpCredentialInput } from "@/types";
import { McpCredentialDialog } from "@/components/settings/McpCredentialDialog";

/** Friendly error display — recognized failures get a short hint + fix step
 *  up front, with the raw backend detail folded underneath. */
function McpErrorHintView({ raw }: { raw: string }) {
  const { t } = useTranslation();
  const hint = analyzeMcpError(raw);
  const [open, setOpen] = useState(false);

  if (!hint) {
    return <p className="text-[10px] text-destructive">{raw}</p>;
  }

  return (
    <div className="rounded-md border border-destructive/30 bg-destructive/5 px-2 py-1.5">
      <div className="flex items-start gap-1.5">
        <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0 text-destructive" />
        <div className="min-w-0 flex-1">
          <p className="text-[11px] font-medium text-destructive">{t(hint.titleKey)}</p>
          {hint.actionKey && (
            <p className="mt-0.5 text-[10px] leading-relaxed text-destructive/80">
              {t(hint.actionKey)}
            </p>
          )}
          {/* Raw detail — collapsed so the friendly hint leads. */}
          <button
            onClick={() => setOpen((v) => !v)}
            className="mt-1 flex items-center gap-0.5 text-[10px] text-destructive/60 hover:text-destructive"
          >
            {open ? <ChevronDown className="h-3 w-3" /> : <ChevronRight className="h-3 w-3" />}
            {t("settings.mcp.errorDetail", { defaultValue: "查看详情" })}
          </button>
          {open && (
            <pre className="mt-1 max-h-24 overflow-auto whitespace-pre-wrap break-words rounded bg-background/60 p-1 font-mono text-[9.5px] leading-relaxed text-destructive/70">
              {raw}
            </pre>
          )}
        </div>
      </div>
    </div>
  );
}

export interface McpServerCardProps {
  server: McpServerWithStatus;
  /** Whether the server has stored OAuth credentials. */
  hasCredential?: boolean;
  onConnect: (name: string) => void;
  onDisconnect: (name: string) => void;
  onRemove: (name: string) => void;
  onSaveCredential: (input: McpCredentialInput) => Promise<void>;
  onDeleteCredential: () => Promise<void>;
}

export function McpServerCard({
  server,
  hasCredential = false,
  onConnect,
  onDisconnect,
  onRemove,
  onSaveCredential,
  onDeleteCredential,
}: McpServerCardProps) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const [armedRemove, setArmedRemove] = useState(false);
  const [credentialOpen, setCredentialOpen] = useState(false);
  const oauthCapable = server.type !== "stdio" && !!server.url;

  const handleRemove = useCallback(() => {
    // Two-step confirm — removing a server also drops its stored
    // credentials and OAuth tokens.
    if (!armedRemove) {
      setArmedRemove(true);
      setTimeout(() => setArmedRemove(false), 3000);
      return;
    }
    setArmedRemove(false);
    onRemove(server.name);
  }, [armedRemove, onRemove, server.name]);

  const handleToggle = useCallback(() => {
    if (server.status === "connected") {
      void onDisconnect(server.name);
    } else if (server.status === "disconnected" || server.status === "error") {
      void onConnect(server.name);
    }
  }, [server.status, server.name, onConnect, onDisconnect]);

  const statusConfig = {
    connected: { label: t("settings.mcp.statusConnected"), variant: "default" as const },
    connecting: { label: t("settings.mcp.statusConnecting"), variant: "secondary" as const },
    installing: { label: t("settings.mcp.statusInstalling"), variant: "secondary" as const },
    disconnected: { label: t("settings.mcp.statusDisconnected"), variant: "outline" as const },
    error: { label: t("settings.mcp.statusError"), variant: "destructive" as const },
  };

  const status = statusConfig[server.status];
  const isConnected = server.status === "connected";
  // Installing (auto-setup of the bundled WPS venv) behaves like connecting:
  // an in-flight operation — connect/remove disabled, spinner shown.
  const isConnecting =
    server.status === "connecting" || server.status === "installing";
  const hasTools = server.tools.length > 0;

  return (
    <Collapsible
      open={expanded}
      onOpenChange={setExpanded}
      className={cn(
        "rounded-lg border border-border bg-card transition-colors",
        "hover:border-border/80 dark:hover:border-border/60",
      )}
    >
      <div className="flex items-center gap-2 px-3 py-2.5">
        {/* Expand toggle (only if connected with tools) */}
        {isConnected && hasTools ? (
          <CollapsibleTrigger asChild>
            <button
              className="flex h-5 w-5 items-center justify-center rounded text-muted-foreground hover:bg-secondary"
              aria-label={expanded ? t("common.collapse") : t("common.expand")}
            >
              {expanded ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
            </button>
          </CollapsibleTrigger>
        ) : (
          <div className="w-5" />
        )}

        {/* Server name + transport */}
        <div className="flex flex-1 items-center gap-2 min-w-0">
          <span className="truncate text-xs font-medium text-foreground">{server.name}</span>
          <Badge variant="outline" className="shrink-0 text-[9px] font-normal">
            {server.type}
          </Badge>
          {hasCredential && (
            <Badge
              variant="outline"
              className="shrink-0 gap-0.5 text-[9px] font-normal text-emerald-600 dark:text-emerald-400"
              title={t("settings.mcp.credentialStored")}
            >
              <KeyRound className="h-2.5 w-2.5" />
              {t("settings.mcp.hasCredential")}
            </Badge>
          )}
        </div>

        {/* Status badge */}
        <Badge variant={status.variant} className="shrink-0 text-[9px]">
          {isConnecting && <Loader2 className="mr-1 h-2.5 w-2.5 animate-spin" />}
          {status.label}
        </Badge>

        {/* Connect / Disconnect button */}
        {oauthCapable && (
          <Button
            variant="ghost"
            size="sm"
            className="h-7 gap-1 text-[11px] text-muted-foreground"
            title={t("settings.mcp.credentialButton")}
            onClick={() => setCredentialOpen(true)}
          >
            <KeyRound className="h-3 w-3" />
            {t("settings.mcp.credentialButton")}
          </Button>
        )}
        <Button
          variant="ghost"
          size="sm"
          className="h-7 gap-1 text-[11px]"
          disabled={isConnecting}
          onClick={handleToggle}
        >
          {isConnected ? (
            <>
              <PlugZap className="h-3 w-3" />
              {t("settings.mcp.disconnect")}
            </>
          ) : (
            <>
              <Plug className="h-3 w-3" />
              {t("settings.mcp.connect")}
            </>
          )}
        </Button>

        {/* Remove button — disabled while connecting: removing a server whose
            connection is still in flight would leave an orphaned backend
            process with no UI to disconnect it. */}
        <Button
          variant={armedRemove ? "destructive" : "ghost"}
          size="sm"
          className="h-7 text-muted-foreground hover:text-destructive"
          disabled={isConnecting}
          onClick={handleRemove}
          aria-label={
            armedRemove
              ? t("settings.mcp.confirmRemove", { defaultValue: "再次点击确认移除" })
              : t("common.delete")
          }
          title={
            armedRemove
              ? t("settings.mcp.confirmRemove", { defaultValue: "再次点击确认移除" })
              : undefined
          }
        >
          <Trash2 className="h-3 w-3" />
        </Button>
      </div>

      {/* Error message — friendly hint first, raw detail collapsible */}
      {server.errorMessage && (
        <div className="px-3 pb-2">
          <McpErrorHintView raw={server.errorMessage} />
        </div>
      )}

      {oauthCapable && (
        <McpCredentialDialog
          server={server}
          open={credentialOpen}
          onOpenChange={setCredentialOpen}
          onSave={onSaveCredential}
          onDelete={onDeleteCredential}
          hasCredential={hasCredential}
        />
      )}

      {/* Tool list (collapsible) */}
      {isConnected && hasTools && (
        <CollapsibleContent>
          <div className="border-t border-border px-3 py-2">
            <div className="mb-1.5 flex items-center gap-1.5 text-[10px] font-medium text-muted-foreground">
              <Wrench className="h-3 w-3" />
              {t("settings.mcp.toolsCount", { count: server.tools.length })}
            </div>
            <div className="space-y-1">
              {server.tools.map((tool) => (
                <div key={tool.name} className="flex items-start gap-2 rounded px-2 py-1 hover:bg-secondary/50">
                  <code className="text-[10px] font-medium text-foreground">{tool.name}</code>
                  <span className="flex-1 text-[10px] text-muted-foreground line-clamp-2">
                    {tool.description}
                  </span>
                  {tool.annotations?.readOnlyHint && (
                    <Badge variant="outline" className="shrink-0 text-[8px] font-normal">
                      RO
                    </Badge>
                  )}
                </div>
              ))}
            </div>
          </div>
        </CollapsibleContent>
      )}
    </Collapsible>
  );
}
