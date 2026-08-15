/**
 * ConnectorsCard — manage external service connections.
 *
 * Shows a tree view of connectors with their permission levels
 * (read-only / read-write). Each connector can be connected/disconnected.
 */

import { useTranslation } from "react-i18next";
import { Plug, Loader2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CollapsibleCard } from "@/components/customize/CollapsibleCard";
import { ConnectorTree } from "@/components/customize/ConnectorTree";
import { useConnectors } from "@/hooks/useConnectors";
import { cn } from "@/lib/utils";

export function ConnectorsCard() {
  const { t } = useTranslation();
  const { connectors, connecting, connect } = useConnectors();

  return (
    <CollapsibleCard
      icon={Plug}
      title={t("customize.connectors")}
      badge={t("customize.connectorsActive", {
        count: connectors.filter((c) => c.status === "connected").length,
      })}
    >
      {connectors.map((conn) => (
        <div key={conn.id} className="space-y-1.5">
          {/* ── Connector header ───────────────────────────── */}
          <div className="flex items-center justify-between">
            <div className="flex items-center gap-2">
              <span
                className={cn(
                  "h-1.5 w-1.5 rounded-full",
                  conn.status === "connected" ? "bg-green-500" : "bg-muted-foreground",
                )}
              />
              <span className="text-xs font-medium">{conn.name}</span>
            </div>
            {conn.status === "connected" ? (
              <Badge variant="success" className="text-[9px]">{t("customize.connected")}</Badge>
            ) : (
              <Button
                size="sm"
                variant="outline"
                className="h-6 gap-1 text-[10px]"
                onClick={() => connect(conn.id)}
                disabled={connecting === conn.id}
              >
                {connecting === conn.id && (
                  <Loader2 className="h-3 w-3 animate-spin" />
                )}
                {t("customize.connect")}
              </Button>
            )}
          </div>

          {/* ── Permission tree ─────────────────────────────── */}
          {conn.permissions.length > 0 && (
            <ConnectorTree permissions={conn.permissions} />
          )}
        </div>
      ))}
    </CollapsibleCard>
  );
}
