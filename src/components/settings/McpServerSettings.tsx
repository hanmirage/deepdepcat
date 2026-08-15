/**
 * McpServerSettings — MCP server management settings page.
 *
 * Displays a list of configured MCP servers with connection status,
 * allows adding/removing servers, and shows tools exposed by each
 * connected server. All logic is in `useMcpServers` — this component
 * only renders state.
 */

import { useTranslation } from "react-i18next";
import { Server, Loader2, AlertCircle, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useMcpServers } from "@/hooks/useMcpServers";
import { McpServerCard } from "@/components/settings/McpServerCard";
import { McpAddServerForm } from "@/components/settings/McpAddServerForm";

export interface McpServerSettingsProps {
  className?: string;
}

export function McpServerSettings({ className }: McpServerSettingsProps) {
  const { t } = useTranslation();
  const {
    servers,
    loading,
    error,
    credentialed,
    refresh,
    connect,
    disconnect,
    addServer,
    removeServer,
    saveCredential,
    deleteCredential,
  } = useMcpServers();

  const hasServers = servers.length > 0;

  return (
    <div className={cn("space-y-4", className)}>
      {/* Header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-2">
          <Server className="h-4 w-4 text-muted-foreground" />
          <p className="text-[11px] text-muted-foreground">{t("settings.mcp.headerDesc")}</p>
        </div>
        <Button
          variant="ghost"
          size="sm"
          className="h-7 gap-1 text-[11px]"
          disabled={loading}
          onClick={() => void refresh()}
        >
          <RefreshCw className={cn("h-3 w-3", loading && "animate-spin")} />
          {t("common.retry")}
        </Button>
      </div>

      {/* Add server form */}
      <McpAddServerForm onAdd={addServer} />

      {/* Error banner */}
      {error && (
        <div className="flex items-center gap-2 rounded-lg border border-destructive/30 bg-destructive/5 px-3 py-2">
          <AlertCircle className="h-3.5 w-3.5 text-destructive" />
          <p className="text-[11px] text-destructive">{error}</p>
        </div>
      )}

      {/* Loading state */}
      {loading && (
        <div className="flex items-center justify-center py-12">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
        </div>
      )}

      {/* Server list */}
      {!loading && hasServers && (
        <div className="space-y-2">
          {servers.map((server) => (
            <McpServerCard
              key={server.name}
              server={server}
              hasCredential={credentialed.includes(server.name)}
              onConnect={connect}
              onDisconnect={disconnect}
              onRemove={removeServer}
              onSaveCredential={(input) => saveCredential(server, input)}
              onDeleteCredential={() => deleteCredential(server)}
            />
          ))}
        </div>
      )}

      {/* Empty state */}
      {!loading && !hasServers && !error && (
        <div className="flex flex-col items-center justify-center py-12 text-center">
          <div className="mb-2 flex h-10 w-10 items-center justify-center rounded-full bg-secondary">
            <Server className="h-5 w-5 text-muted-foreground" />
          </div>
          <p className="text-xs font-medium text-foreground">{t("settings.mcp.emptyTitle")}</p>
          <p className="mt-1 max-w-xs text-[11px] leading-relaxed text-muted-foreground">
            {t("settings.mcp.emptyDesc")}
          </p>
        </div>
      )}
    </div>
  );
}
