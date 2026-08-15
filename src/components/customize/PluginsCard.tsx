/**
 * PluginsCard — plugin marketplace & installed plugins.
 *
 * Shows:
 * - "Browse plugins" marketplace entry
 * - List of installed plugins with enable/disable toggle
 * - One-click install for available plugins
 */

import { useTranslation } from "react-i18next";
import { Puzzle, Download } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { CollapsibleCard } from "@/components/customize/CollapsibleCard";
import { usePlugins } from "@/hooks/usePlugins";

export function PluginsCard() {
  const { t } = useTranslation();
  const { plugins, install, toggle } = usePlugins();

  const installed = plugins.filter((p) => p.installed);
  const available = plugins.filter((p) => !p.installed);

  return (
    <CollapsibleCard
      icon={Puzzle}
      title={t("customize.plugins")}
      badge={t("customize.pluginsInstalled", { count: installed.length })}
      contentClassName="space-y-3"
    >
      {/* ── Installed plugins ───────────────────────────────── */}
      {installed.length > 0 && (
        <div className="space-y-1.5">
          <p className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            {t("customize.installed")}
          </p>
          {installed.map((plugin) => (
            <div
              key={plugin.id}
              className="flex items-center justify-between rounded-md border border-border p-2"
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-1.5">
                  <span className="text-xs font-medium">{plugin.name}</span>
                  <Badge variant="secondary" className="text-[8px]">v{plugin.version}</Badge>
                </div>
                <p className="truncate text-[10px] text-muted-foreground">
                  {plugin.description}
                </p>
              </div>
              <Switch
                checked={plugin.enabled}
                onCheckedChange={(v) => void toggle(plugin.id, v)}
                className="scale-75 shrink-0"
              />
            </div>
          ))}
        </div>
      )}

      {/* ── Available plugins ───────────────────────────────── */}
      {available.length > 0 && (
        <div className="space-y-1.5">
          <p className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
            {t("customize.available")}
          </p>
          {available.map((plugin) => (
            <div
              key={plugin.id}
              className="flex items-center justify-between rounded-md border border-dashed border-border p-2"
            >
              <div className="min-w-0 flex-1">
                <div className="flex items-center gap-1.5">
                  <span className="text-xs font-medium">{plugin.name}</span>
                  <Badge variant="outline" className="text-[8px]">v{plugin.version}</Badge>
                </div>
                <p className="truncate text-[10px] text-muted-foreground">
                  {plugin.description}
                </p>
              </div>
              <Button
                size="sm"
                variant="ghost"
                className="h-6 shrink-0 gap-1 text-[10px]"
                onClick={() => install(plugin.id)}
              >
                <Download className="h-3 w-3" />
                {t("customize.install")}
              </Button>
            </div>
          ))}
        </div>
      )}
    </CollapsibleCard>
  );
}
