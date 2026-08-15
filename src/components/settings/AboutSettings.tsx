/**
 * AboutSettings — app version, update checking, and system info panel.
 *
 * Shows version, system info (from backend), and an update check section
 * that uses the Tauri v2 updater plugin to download and install updates.
 */

import { useState, useCallback, useEffect } from "react";
import { useTranslation } from "react-i18next";
import { useAppStore } from "@/stores/appStore";
import { useAuthStore } from "@/stores/authStore";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { cn } from "@/lib/utils";
import {
  RefreshCw,
  Download,
  CheckCircle2,
  AlertCircle,
  PackageCheck,
  History,
  Globe,
  ExternalLink,
} from "lucide-react";
import appIcon from "/icon.png";
import { cloudApi, type ChangelogEntry, type SiteConfig } from "@/lib/tauri";

export interface AboutSettingsProps {
  className?: string;
}

export function AboutSettings({ className }: AboutSettingsProps) {
  const { t } = useTranslation();
  const systemInfo = useAppStore((s) => s.systemInfo);
  const updateInfo = useAppStore((s) => s.updateInfo);
  const updateChecking = useAppStore((s) => s.updateChecking);
  const updateDownloading = useAppStore((s) => s.updateDownloading);
  const updateProgress = useAppStore((s) => s.updateProgress);
  const updateError = useAppStore((s) => s.updateError);
  const silentUpdate = useAppStore((s) => s.silentUpdate);
  const checkForUpdate = useAppStore((s) => s.checkForUpdate);
  const downloadAndInstallUpdate = useAppStore((s) => s.downloadAndInstallUpdate);
  const clearUpdateError = useAppStore((s) => s.clearUpdateError);

  const version = systemInfo?.app_version ?? "1.0.0";
  const os = systemInfo ? `${systemInfo.os} ${systemInfo.arch}` : "Unknown";
  const cpuCount = systemInfo?.cpu_count ?? 0;
  const totalMemory = systemInfo?.total_memory_mb ?? 0;

  const downloadFraction =
    updateProgress?.phase === "progress" ? updateProgress.fraction : 0;

  // ── Cloud: changelog + site config (public website endpoints) ──
  const serverUrl = useAuthStore((s) => s.serverUrl);
  const [changelog, setChangelog] = useState<ChangelogEntry[]>([]);
  const [siteConfig, setSiteConfig] = useState<SiteConfig | null>(null);

  const loadCloudInfo = useCallback(() => {
    void cloudApi
      .fetchChangelog(serverUrl)
      .then((r) => setChangelog(r.updates ?? []))
      .catch(() => setChangelog([]));
    void cloudApi
      .fetchSiteConfig(serverUrl)
      .then(setSiteConfig)
      .catch(() => setSiteConfig(null));
  }, [serverUrl]);

  useEffect(() => {
    loadCloudInfo();
  }, [loadCloudInfo]);

  return (
    <div className={cn("space-y-6", className)}>
      <section>
        <h3 className="mb-3 text-sm font-semibold">{t("settings.about.title")}</h3>
        <div className="space-y-3">
          <div className="flex items-center gap-3">
            <div className="flex h-12 w-12 items-center justify-center rounded-lg bg-primary/10 p-1">
              <img src={appIcon} alt="DeepDepCat" className="h-full w-full rounded-md" />
            </div>
            <div>
              <p className="text-sm font-semibold">{t("settings.about.appName")}</p>
              <p className="text-[10px] text-muted-foreground">v{version}</p>
            </div>
          </div>
        </div>
      </section>

      {/* ── Update section ──────────────────────────────────── */}
      <section>
        <h3 className="mb-3 text-sm font-semibold">
          {t("settings.about.updates")}
        </h3>

        {updateError && (
          <div className="mb-3 flex items-start gap-2 rounded-md border border-destructive/30 bg-destructive/5 p-2">
            <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" />
            <p className="flex-1 text-[10px] text-destructive">{updateError}</p>
            <Button
              variant="ghost"
              size="sm"
              className="h-5 px-1.5 text-[10px]"
              onClick={clearUpdateError}
            >
              {t("common.dismiss")}
            </Button>
          </div>
        )}

        {/* No update checked yet */}
        {!updateInfo && !updateChecking && !updateDownloading && silentUpdate.state === "idle" && (
          <Button
            variant="outline"
            size="sm"
            className="h-8 gap-1.5 text-xs"
            onClick={checkForUpdate}
          >
            <RefreshCw className="h-3.5 w-3.5" />
            {t("settings.about.checkForUpdate")}
          </Button>
        )}

        {/* Silent update staged / downloading (backend-only release) */}
        {silentUpdate.state !== "idle" && !updateInfo && !updateDownloading && (
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            {silentUpdate.state === "downloading" ? (
              <>
                <RefreshCw className="h-3.5 w-3.5 animate-spin text-primary" />
                {t("settings.about.silentDownloading")}
              </>
            ) : (
              <>
                <PackageCheck className="h-3.5 w-3.5 text-primary" />
                {t("settings.about.silentStaged", {
                  version: silentUpdate.version ?? "",
                })}
              </>
            )}
          </div>
        )}

        {/* Checking */}
        {updateChecking && (
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <RefreshCw className="h-3.5 w-3.5 animate-spin" />
            {t("settings.about.checking")}
          </div>
        )}

        {/* Update available */}
        {updateInfo && !updateDownloading && (
          <div className="space-y-2">
            <div className="flex items-center gap-2 text-xs">
              <span className="font-medium text-primary">
                {t("settings.about.newVersion")}: v{updateInfo.version}
              </span>
              <span className="text-muted-foreground">
                ({t("settings.about.currentVersion")}: v{updateInfo.current_version})
              </span>
            </div>
            {updateInfo.body && (
              <p className="max-h-32 overflow-y-auto rounded-md bg-muted/50 p-2 text-[10px] text-muted-foreground">
                {updateInfo.body}
              </p>
            )}
            <Button
              variant="default"
              size="sm"
              className="h-8 gap-1.5 text-xs"
              onClick={downloadAndInstallUpdate}
            >
              <Download className="h-3.5 w-3.5" />
              {t("settings.about.downloadAndInstall")}
            </Button>
          </div>
        )}

        {/* Downloading */}
        {updateDownloading && (
          <div className="space-y-2">
            <div className="flex items-center justify-between text-xs">
              <span className="flex items-center gap-1.5">
                <Download className="h-3.5 w-3.5 animate-pulse" />
                {t("settings.about.downloading")}
              </span>
              <span className="text-muted-foreground">
                {Math.round(downloadFraction * 100)}%
              </span>
            </div>
            <Progress value={downloadFraction * 100} className="h-1.5" />
          </div>
        )}

        {/* Finished */}
        {updateProgress?.phase === "finished" && (
          <div className="flex items-center gap-2 rounded-md border border-primary/30 bg-primary/5 p-2 text-xs">
            <CheckCircle2 className="h-3.5 w-3.5 text-primary" />
            <span>
              {t("settings.about.updateReady")}
            </span>
          </div>
        )}
      </section>

      {/* ── Site info (cloud) ───────────────────────────────── */}
      {siteConfig && (
        <section>
          <h3 className="mb-3 text-sm font-semibold">{t("settings.about.siteInfo")}</h3>
          <div className="space-y-2">
            {siteConfig.latestVersion && (
              <div className="flex items-center gap-2 text-xs">
                <span className="text-muted-foreground">{t("settings.about.latestVersion")}</span>
                <span className="font-medium">v{siteConfig.latestVersion}</span>
                {siteConfig.latestDate && (
                  <span className="text-[10px] text-muted-foreground">
                    {siteConfig.latestDate}
                  </span>
                )}
              </div>
            )}
            {siteConfig.siteUrl && (
              <a
                href={siteConfig.siteUrl}
                target="_blank"
                rel="noreferrer"
                className="flex w-fit items-center gap-1.5 text-xs text-primary hover:underline"
              >
                <Globe className="h-3.5 w-3.5" />
                {t("settings.about.officialSite")}
                <ExternalLink className="h-3 w-3" />
              </a>
            )}
            {siteConfig.githubIssuesUrl && (
              <a
                href={siteConfig.githubIssuesUrl}
                target="_blank"
                rel="noreferrer"
                className="flex w-fit items-center gap-1.5 text-xs text-muted-foreground hover:text-primary hover:underline"
              >
                {t("settings.about.feedback")}
                <ExternalLink className="h-3 w-3" />
              </a>
            )}
            {siteConfig.contactEmail && (
              <p className="text-[10px] text-muted-foreground">
                {t("settings.about.contactEmail", { email: siteConfig.contactEmail })}
              </p>
            )}
          </div>
        </section>
      )}

      {/* ── Update log (cloud changelog) ────────────────────── */}
      {changelog.length > 0 && (
        <section>
          <div className="mb-3 flex items-center justify-between">
            <h3 className="text-sm font-semibold">{t("settings.about.changelog")}</h3>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 gap-1 px-2 text-[10px]"
              onClick={loadCloudInfo}
            >
              <RefreshCw className="h-3 w-3" />
              {t("settings.about.refresh")}
            </Button>
          </div>
          <div className="max-h-64 space-y-2 overflow-y-auto pr-1">
            {changelog.map((entry) => (
              <div
                key={entry.version}
                className="rounded-md border border-border bg-background px-2.5 py-2"
              >
                <div className="flex items-center gap-2">
                  <History className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  <span className="text-xs font-semibold">
                    v{entry.version} {entry.title}
                  </span>
                  <span className="ml-auto text-[10px] text-muted-foreground">
                    {entry.date}
                  </span>
                </div>
                {entry.items.length > 0 && (
                  <ul className="mt-1 space-y-0.5 pl-5">
                    {entry.items.map((item, i) => (
                      <li key={i} className="list-disc text-[10px] text-muted-foreground">
                        {item}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            ))}
          </div>
        </section>
      )}

      <section>
        <h3 className="mb-3 text-sm font-semibold">{t("settings.about.systemInfo")}</h3>
        <dl className="space-y-2">
          <div className="flex justify-between">
            <dt className="text-xs text-muted-foreground">{t("settings.about.os")}</dt>
            <dd className="text-xs">{os}</dd>
          </div>
          <div className="flex justify-between">
            <dt className="text-xs text-muted-foreground">{t("settings.about.cpuCores")}</dt>
            <dd className="text-xs">{cpuCount}</dd>
          </div>
          <div className="flex justify-between">
            <dt className="text-xs text-muted-foreground">{t("settings.about.totalMemory")}</dt>
            <dd className="text-xs">{(totalMemory / 1024).toFixed(1)} GB</dd>
          </div>
        </dl>
      </section>

      <section>
        <p className="text-[10px] text-muted-foreground">
          {t("settings.about.footer")}
        </p>
      </section>
    </div>
  );
}
