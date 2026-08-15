/**
 * ForceUpdateDialog — a blocking mandatory-update screen.
 *
 * When the server marks a release as `force` (via min_version), the running
 * client is too old to be supported. This dialog has no cancel path: the only
 * actions are "Update now" (downloads + installs) or retrying the download.
 * The user cannot keep using an unsupported version.
 */

import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import { Download, Loader2, RefreshCw, RotateCw, ShieldAlert } from "lucide-react";
import { useAppStore } from "@/stores/appStore";
import { Button } from "@/components/ui/button";

export function ForceUpdateDialog() {
  const { t } = useTranslation();
  const forceUpdate = useAppStore((s) => s.forceUpdate);
  const updateInfo = useAppStore((s) => s.updateInfo);
  const updateDownloading = useAppStore((s) => s.updateDownloading);
  const updateError = useAppStore((s) => s.updateError);
  const updateProgress = useAppStore((s) => s.updateProgress);
  const updateInstalled = useAppStore((s) => s.updateInstalled);
  const downloadAndInstallUpdate = useAppStore((s) => s.downloadAndInstallUpdate);
  const checkForUpdate = useAppStore((s) => s.checkForUpdate);
  const relaunchApp = useAppStore((s) => s.relaunchApp);
  const dialogRef = useRef<HTMLDivElement>(null);

  // Focus trap + initial focus: keyboard users must stay inside the dialog
  // while it blocks the app, and focus must land on the primary action
  // instead of staying on the (covered) background page.
  useEffect(() => {
    if (!forceUpdate || !updateInfo) return;
    const primary = dialogRef.current?.querySelector<HTMLElement>("button");
    primary?.focus();

    const handleTab = (e: KeyboardEvent) => {
      if (e.key !== "Tab" || !dialogRef.current) return;
      const focusables = dialogRef.current.querySelectorAll<HTMLElement>(
        'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
      );
      if (focusables.length === 0) return;
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", handleTab);
    return () => window.removeEventListener("keydown", handleTab);
  }, [forceUpdate, updateInfo]);

  if (!forceUpdate || !updateInfo) return null;

  const fraction =
    updateProgress?.phase === "progress" ? Math.round(updateProgress.fraction * 100) : 0;

  return (
    <div className="fixed inset-0 z-[100] flex items-center justify-center bg-black/70 backdrop-blur-sm">
      <div
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="force-update-title"
        className="w-[400px] rounded-2xl border border-border bg-card p-6 shadow-2xl"
      >
        <div className="mb-4 flex items-center gap-2">
          <ShieldAlert className="h-5 w-5 text-primary" />
          <h2 id="force-update-title" className="text-base font-semibold">{t("update.forceTitle")}</h2>
        </div>

        {updateInstalled ? (
          <p className="text-sm text-muted-foreground">
            {t("update.installedRestart", { version: updateInfo.version })}
          </p>
        ) : (
          <p className="text-sm text-muted-foreground">
            {t("update.forceBody", {
              version: updateInfo.version,
              current: updateInfo.current_version,
            })}
          </p>
        )}

        {updateInfo.body && !updateInstalled && (
          <div className="mt-3 max-h-28 overflow-y-auto rounded-lg bg-muted/50 p-3 text-xs text-foreground/80">
            {updateInfo.body}
          </div>
        )}

        {updateDownloading && fraction > 0 && (
          <div className="mt-4">
            <div className="mb-1 flex justify-between text-xs text-muted-foreground">
              <span>{t("update.downloading")}</span>
              <span>{fraction}%</span>
            </div>
            <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
              <div className="h-full bg-primary transition-all" style={{ width: `${fraction}%` }} />
            </div>
          </div>
        )}

        {updateError && (
          <p className="mt-3 text-xs text-destructive">{updateError}</p>
        )}

        <div className="mt-5 flex gap-2">
          {updateInstalled ? (
            <Button className="flex-1 gap-1.5" onClick={() => void relaunchApp()}>
              <RotateCw className="h-4 w-4" />
              {t("update.restartNow")}
            </Button>
          ) : (
            <Button
              className="flex-1 gap-1.5"
              onClick={() => void downloadAndInstallUpdate()}
              disabled={updateDownloading}
            >
              {updateDownloading ? (
                <Loader2 className="h-4 w-4 animate-spin" />
              ) : (
                <Download className="h-4 w-4" />
              )}
              {updateDownloading ? t("update.downloading") : t("update.updateNow")}
            </Button>
          )}
          {updateError && !updateInstalled && (
            <Button
              variant="outline"
              className="gap-1.5"
              onClick={() => void checkForUpdate()}
            >
              <RefreshCw className="h-4 w-4" />
              {t("update.retry")}
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
