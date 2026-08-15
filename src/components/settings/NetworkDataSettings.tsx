/**
 * NetworkDataSettings — network plumbing and data privacy:
 * HTTP proxy (+ bypass list), cloud sync, anonymous diagnostics, and the
 * ACP/A2A local integration services (external clients drive the app over a
 * localhost port).
 */

import { useEffect, useState } from "react";
import { logError } from "@/lib/logger";
import { useTranslation } from "react-i18next";
import { CloudUpload, Loader2 } from "lucide-react";
import { useSettingsStore } from "@/stores/settingsStore";
import { useAuthStore } from "@/stores/authStore";
import { Switch } from "@/components/ui/switch";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { SettingRow } from "@/components/settings/SettingRow";
import { NumberField } from "@/components/settings/NumberField";
import { useConfigSection } from "@/hooks/useConfigSection";
import { setClientErrorReporting } from "@/lib/clientErrorReporter";
import { diagnosticsApi, isTauri, syncApi } from "@/lib/tauri";
import { cn } from "@/lib/utils";

/** 云端同步 — 手动推送/拉取会话与设置（需登录）。 */
function CloudSyncRow() {
  const { t } = useTranslation();
  const loggedIn = useAuthStore((s) => s.user !== null);
  const serverUrl = useAuthStore((s) => s.serverUrl);
  const [syncing, setSyncing] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [syncFailed, setSyncFailed] = useState(false);

  const runSync = async () => {
    if (!isTauri) return;
    setSyncing(true);
    setResult(null);
    setSyncFailed(false);
    try {
      const token = (await useAuthStore.getState().accessToken()) ?? "";
      const summary = await syncApi.syncNow(serverUrl, token);
      setResult(
        t("settings.general.cloudSyncDone", {
          pushed: summary.pushed,
          pulled: summary.pulled,
        }),
      );
    } catch (e) {
      logError("CloudSync", "sync failed:", e);
      setResult(t("settings.general.cloudSyncFailed"));
      setSyncFailed(true);
    } finally {
      setSyncing(false);
    }
  };

  if (!loggedIn) {
    return (
      <p className="text-[11px] text-muted-foreground">
        {t("settings.general.cloudSyncLoginHint")}
      </p>
    );
  }

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <Button
          size="sm"
          className="h-8 gap-1 text-xs"
          onClick={() => void runSync()}
          disabled={syncing}
        >
          {syncing ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <CloudUpload className="h-3.5 w-3.5" />
          )}
          {t("settings.general.cloudSyncButton")}
        </Button>
      </div>
      {result && (
        <p className={cn("text-[11px]", syncFailed ? "text-destructive" : "text-muted-foreground")}>
          {result}
        </p>
      )}
      <p className="text-[10px] text-muted-foreground/60">
        {t("settings.general.cloudSyncDesc")}
      </p>
    </div>
  );
}

/** ACP 服务开关与端口 — 让外部客户端（IDE/脚本）通过本机端口驱动应用。 */
function AcpSettingsRow() {
  const { t } = useTranslation();
  const { load, patch } = useConfigSection();
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [port, setPort] = useState<number>(31524);

  useEffect(() => {
    void (async () => {
      const app = await load("app");
      if (!app) return; // backend unavailable — keep the section hidden
      setEnabled(Boolean(app.acp_enabled ?? false));
      setPort(Number(app.acp_port ?? 31524) || 31524);
    })();
  }, [load]);

  const persist = (data: Record<string, unknown>) => {
    if (enabled === null) return;
    void patch("app", data);
  };

  if (enabled === null) return null;

  return (
    <div className="space-y-2">
      <SettingRow
        searchKey="settings.general.acpEnabled"
        label={t("settings.general.acpEnabled")}
        description={t("settings.general.acpEnabledDesc")}
      >
        <Switch
          checked={enabled}
          onCheckedChange={(v) => {
            setEnabled(v);
            persist({ acp_enabled: v });
          }}
        />
      </SettingRow>
      <SettingRow
        searchKey="settings.general.acpPort"
        label={t("settings.general.acpPort")}
        description={t("settings.general.acpPortDesc")}
      >
        <NumberField
          min={1}
          max={65535}
          value={port}
          onCommit={(v) => {
            setPort(v);
            persist({ acp_port: v });
          }}
        />
      </SettingRow>
      <p className="text-[10px] text-muted-foreground/60">
        {t("settings.general.acpHint")}
      </p>
    </div>
  );
}

/** A2A 入站服务开关与端口 — 让其他 agent 通过 A2A 协议编排本应用。 */
function A2aSettingsRow() {
  const { t } = useTranslation();
  const { load, patch } = useConfigSection();
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [port, setPort] = useState<number>(31525);

  useEffect(() => {
    void (async () => {
      const app = await load("app");
      if (!app) return;
      setEnabled(Boolean(app.a2a_enabled ?? false));
      setPort(Number(app.a2a_port ?? 31525) || 31525);
    })();
  }, [load]);

  const persist = (data: Record<string, unknown>) => {
    if (enabled === null) return;
    void patch("app", data);
  };

  if (enabled === null) return null;

  return (
    <div className="space-y-2">
      <SettingRow
        searchKey="settings.general.a2aEnabled"
        label={t("settings.general.a2aEnabled")}
        description={t("settings.general.a2aEnabledDesc")}
      >
        <Switch
          checked={enabled}
          onCheckedChange={(v) => {
            setEnabled(v);
            persist({ a2a_enabled: v });
          }}
        />
      </SettingRow>
      <SettingRow
        searchKey="settings.general.a2aPort"
        label={t("settings.general.a2aPort")}
        description={t("settings.general.a2aPortDesc")}
      >
        <NumberField
          min={1}
          max={65535}
          value={port}
          onCommit={(v) => {
            setPort(v);
            persist({ a2a_port: v });
          }}
        />
      </SettingRow>
      <p className="text-[10px] text-muted-foreground/60">
        {t("settings.general.a2aHint")}
      </p>
    </div>
  );
}

export interface NetworkDataSettingsProps {
  className?: string;
}

export function NetworkDataSettings({ className }: NetworkDataSettingsProps) {
  const { t } = useTranslation();
  const general = useSettingsStore((s) => s.general);
  const updateGeneral = useSettingsStore((s) => s.updateGeneral);

  return (
    <div className={cn("space-y-1", className)}>
      <SettingRow
        searchKey="settings.general.httpProxy"
        label={t("settings.general.httpProxy")}
        description={t("settings.general.httpProxyDesc")}
      >
        <Input
          value={general.proxyUrl}
          onChange={(e) => updateGeneral({ proxyUrl: e.target.value })}
          placeholder={t("settings.general.httpProxyPlaceholder")}
          className="h-8 w-72 text-xs"
        />
      </SettingRow>

      <SettingRow
        searchKey="settings.general.noProxy"
        label={t("settings.general.noProxy")}
        description={t("settings.general.noProxyDesc")}
      >
        <Input
          value={general.noProxyList}
          onChange={(e) => updateGeneral({ noProxyList: e.target.value })}
          placeholder={t("settings.general.noProxyPlaceholder")}
          className="h-8 w-72 text-xs"
        />
      </SettingRow>

      {/* ── 云端同步 (P0-1) ── */}
      <div className="border-t border-[hsl(var(--border))] pt-4">
        <h3 className="mb-3 text-xs font-semibold">
          {t("settings.general.cloudSync")}
        </h3>
        <CloudSyncRow />
      </div>

      {/* ── 隐私 (anonymous diagnostics) ── */}
      <div className="border-t border-[hsl(var(--border))] pt-4">
        <h3 className="mb-3 text-xs font-semibold">
          {t("settings.general.privacy")}
        </h3>
        <SettingRow
          searchKey="settings.general.diagnosticsEnabled"
          label={t("settings.general.diagnosticsEnabled")}
          description={t("settings.general.diagnosticsEnabledDesc")}
        >
          <Switch
            checked={general.diagnosticsEnabled}
            onCheckedChange={(v) => {
              updateGeneral({ diagnosticsEnabled: v });
              // Sync the toggle to the backend so the Rust reporter respects
              // the user's choice immediately (opt-out = no data leaves).
              diagnosticsApi.setEnabled(v).catch(() => {});
              setClientErrorReporting(v);
            }}
          />
        </SettingRow>
      </div>

      {/* ── ACP/A2A 集成服务 ── */}
      <div className="border-t border-[hsl(var(--border))] pt-4">
        <h3 className="mb-3 text-xs font-semibold">
          {t("settings.general.acp")}
        </h3>
        <AcpSettingsRow />
        <div className="mt-3 border-t border-[hsl(var(--border))] pt-3">
          <h4 className="mb-2 text-xs font-semibold">
            {t("settings.general.a2a")}
          </h4>
          <A2aSettingsRow />
        </div>
      </div>
    </div>
  );
}
