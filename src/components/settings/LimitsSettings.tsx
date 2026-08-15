/**
 * LimitsSettings — agent execution boundaries:
 * session token/cost limits, concurrent-tool & turn-output caps, and the
 * run timeout (backend [agent] section).
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSettingsStore } from "@/stores/settingsStore";
import { SettingRow } from "@/components/settings/SettingRow";
import { NumberField } from "@/components/settings/NumberField";
import { useConfigSection } from "@/hooks/useConfigSection";
import { cn } from "@/lib/utils";

/** Agent 执行上限 — max_concurrent_tools / turn_output_token_limit。 */
function AgentLimitsEditor() {
  const { t } = useTranslation();
  const { load, patch } = useConfigSection();
  const [maxConcurrent, setMaxConcurrent] = useState<number | null>(null);
  const [turnOutputLimit, setTurnOutputLimit] = useState<number | null>(null);
  const [runTimeoutSecs, setRunTimeoutSecs] = useState<number | null>(null);

  useEffect(() => {
    void (async () => {
      const agent = await load("agent");
      if (!agent) return;
      setMaxConcurrent(Number(agent.max_concurrent_tools ?? 5));
      setTurnOutputLimit(
        agent.turn_output_token_limit != null ? Number(agent.turn_output_token_limit) : 0,
      );
      setRunTimeoutSecs(agent.run_timeout_secs != null ? Number(agent.run_timeout_secs) : 0);
    })();
  }, [load]);

  const persist = (data: Record<string, unknown>) => {
    void patch("agent", data);
  };

  return (
    <div className="space-y-2">
      <SettingRow
        searchKey="settings.general.maxConcurrentTools"
        label={t("settings.general.maxConcurrentTools")}
        description={t("settings.general.maxConcurrentToolsDesc")}
      >
        <NumberField
          min={1}
          value={maxConcurrent ?? 5}
          onCommit={(v) => {
            setMaxConcurrent(v);
            persist({ max_concurrent_tools: v });
          }}
        />
      </SettingRow>
      <SettingRow
        searchKey="settings.general.turnOutputLimit"
        label={t("settings.general.turnOutputLimit")}
        description={t("settings.general.turnOutputLimitDesc")}
      >
        <NumberField
          min={0}
          value={turnOutputLimit ?? 0}
          placeholder="0"
          onCommit={(v) => {
            setTurnOutputLimit(v);
            persist({ turn_output_token_limit: v > 0 ? v : null });
          }}
        />
      </SettingRow>
      <SettingRow
        searchKey="settings.general.runTimeout"
        label={t("settings.general.runTimeout")}
        description={t("settings.general.runTimeoutDesc")}
      >
        <NumberField
          min={0}
          value={runTimeoutSecs ?? 0}
          placeholder="0"
          onCommit={(v) => {
            setRunTimeoutSecs(v);
            persist({ run_timeout_secs: v > 0 ? v : null });
          }}
        />
      </SettingRow>
    </div>
  );
}

export interface LimitsSettingsProps {
  className?: string;
}

export function LimitsSettings({ className }: LimitsSettingsProps) {
  const { t } = useTranslation();
  const general = useSettingsStore((s) => s.general);
  const updateGeneral = useSettingsStore((s) => s.updateGeneral);

  return (
    <div className={cn("space-y-1", className)}>
      <SettingRow
        searchKey="settings.general.sessionTokenLimit"
        label={t("settings.general.sessionTokenLimit")}
        description={t("settings.general.sessionTokenLimitDesc")}
      >
        <NumberField
          min={0}
          value={general.sessionTokenLimit}
          placeholder="0"
          onCommit={(v) => updateGeneral({ sessionTokenLimit: v })}
        />
      </SettingRow>

      <SettingRow
        searchKey="settings.general.sessionCostLimit"
        label={t("settings.general.sessionCostLimit")}
        description={t("settings.general.sessionCostLimitDesc")}
      >
        <NumberField
          min={0}
          step="0.1"
          value={general.sessionCostLimit}
          placeholder="0.0"
          onCommit={(v) => updateGeneral({ sessionCostLimit: v })}
        />
      </SettingRow>

      {/* ── Agent 执行上限 ── */}
      <div className="border-t border-[hsl(var(--border))] pt-4">
        <h3 className="mb-3 text-xs font-semibold">
          {t("settings.general.agentLimits")}
        </h3>
        <AgentLimitsEditor />
      </div>
    </div>
  );
}
