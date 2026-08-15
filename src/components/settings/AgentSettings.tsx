/**
 * AgentSettings — agent persona definitions.
 *
 * Lists the agent definitions (personas) available to the current work mode
 * and their details. Persona and execution-strategy selection are done via
 * slash commands in the input bar (/code-reviewer, /standard, …).
 */

import { useState, useCallback, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { Loader2, Bot, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { agentApi, type AgentDefinition } from "@/lib/tauri";
import { useAppStore } from "@/stores/appStore";
import { cn } from "@/lib/utils";

export function AgentSettings() {
  const { t } = useTranslation();
  const mode = useAppStore((s) => s.mode);
  const [definitions, setDefinitions] = useState<AgentDefinition[]>([]);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setDefinitions((await agentApi.listDefinitions(mode)).filter((d) => d.id !== "default"));
    } catch {
      setDefinitions([]);
    } finally {
      setLoading(false);
    }
  }, [mode]);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className="space-y-6">
      <section>
        <div className="mb-3 flex items-center justify-between">
          <h3 className="text-sm font-semibold">{t("settings.agents.title")}</h3>
          <Button
            variant="outline"
            size="sm"
            className="h-7 gap-1.5 text-xs"
            onClick={() => void load()}
            disabled={loading}
          >
            <RefreshCw className={cn("h-3.5 w-3.5", loading && "animate-spin")} />
            {t("settings.memory.refresh")}
          </Button>
        </div>
        <p className="mb-3 text-xs text-muted-foreground">
          {t("settings.agents.desc", { mode })}
        </p>

        {loading ? (
          <div className="flex items-center justify-center py-10">
            <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        ) : definitions.length === 0 ? (
          <p className="rounded-md bg-muted/40 px-3 py-4 text-center text-xs text-muted-foreground">
            {t("settings.agents.empty")}
          </p>
        ) : (
          <div className="space-y-2">
            {definitions.map((def) => (
              <div
                key={def.id}
                className="rounded-md border border-border bg-background px-3 py-2.5"
              >
                <div className="mb-1 flex items-center gap-2">
                  <Bot className="h-4 w-4 shrink-0 text-primary/70" />
                  <span className="truncate text-xs font-medium">{def.name}</span>
                  {def.is_builtin ? (
                    <Badge variant="secondary" className="text-[9px]">
                      {t("settings.agents.builtin")}
                    </Badge>
                  ) : (
                    <Badge variant="outline" className="text-[9px]">
                      {t("settings.agents.custom")}
                    </Badge>
                  )}
                </div>
                {def.description && (
                  <p className="mb-1 line-clamp-2 text-[11px] text-muted-foreground">
                    {def.description}
                  </p>
                )}
                <div className="flex flex-wrap gap-1.5">
                  <Badge variant="secondary" className="text-[9px]">
                    {t("settings.agents.promptMode")}:{" "}
                    {def.prompt_mode === "full"
                      ? t("settings.agents.promptFull")
                      : t("settings.agents.promptExtend")}
                  </Badge>
                  {def.model && (
                    <Badge variant="secondary" className="text-[9px]">
                      {def.model}
                    </Badge>
                  )}
                  {def.allowed_tools.length > 0 && (
                    <Badge variant="secondary" className="text-[9px]">
                      {def.allowed_tools.length}{" "}
                      {t("settings.agents.tools", { count: def.allowed_tools.length })}
                    </Badge>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
