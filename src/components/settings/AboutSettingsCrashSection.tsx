/**
 * CrashReportsSection — crash report list + viewer, extracted from
 * AboutSettings.
 */

import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { RefreshCw, FileWarning, Eye, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { crashApi, type CrashReportInfo } from "@/lib/tauri";
import { cn } from "@/lib/utils";

export function CrashReportsSection() {
  const { t } = useTranslation();
  // ── Crash reports ─────────────────────────────────────────
  const [crashReports, setCrashReports] = useState<CrashReportInfo[]>([]);
  const [crashLoading, setCrashLoading] = useState(false);
  const [crashContent, setCrashContent] = useState<string | null>(null);
  const [armedCrashDelete, setArmedCrashDelete] = useState<string | null>(null);

  const loadCrashReports = useCallback(async () => {
    setCrashLoading(true);
    try {
      setCrashReports(await crashApi.list());
    } catch {
      setCrashReports([]);
    } finally {
      setCrashLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadCrashReports();
  }, [loadCrashReports]);

  const handleViewCrash = useCallback(async (filename: string) => {
    try {
      const content = await crashApi.read(filename);
      setCrashContent(content ?? t("crashDialog.readFailed"));
    } catch {
      setCrashContent(t("crashDialog.readFailed"));
    }
  }, [t]);

  const handleDeleteCrash = useCallback(
    async (filename: string) => {
      // Two-step confirm — crash reports are the only evidence of a bug;
      // a mis-click loses it permanently.
      if (armedCrashDelete !== filename) {
        setArmedCrashDelete(filename);
        setTimeout(() => setArmedCrashDelete((cur) => (cur === filename ? null : cur)), 3000);
        return;
      }
      setArmedCrashDelete(null);
      try {
        const ok = await crashApi.delete(filename);
        if (ok) {
          setCrashReports((prev) => prev.filter((r) => r.filename !== filename));
          setCrashContent(null);
        }
      } catch {
        // Deletion failed — keep the row.
      }
    },
    [armedCrashDelete],
  );

  return (
    <section>
    <div className="mb-3 flex items-center justify-between">
        <h3 className="text-sm font-semibold">
          {t("settings.about.crashReports")}
        </h3>
        <Button
          variant="ghost"
          size="sm"
          className="h-6 gap-1 px-2 text-[10px]"
          onClick={loadCrashReports}
          disabled={crashLoading}
        >
          <RefreshCw className={cn("h-3 w-3", crashLoading && "animate-spin")} />
          {t("settings.about.refresh")}
        </Button>
      </div>

      {crashReports.length === 0 ? (
        <p className="rounded-md bg-muted/40 px-3 py-2 text-[11px] text-muted-foreground">
          {crashLoading
            ? t("settings.about.loading")
            : t("settings.about.noCrashReports")}
        </p>
      ) : (
        <div className="space-y-1.5">
          {crashReports.map((report) => (
            <div
              key={report.filename}
              className="flex items-center gap-2 rounded-md border border-border bg-background px-2.5 py-1.5"
            >
              <FileWarning className="h-3.5 w-3.5 shrink-0 text-amber-500" />
              <div className="min-w-0 flex-1">
                <p className="truncate text-xs font-medium">{report.timestamp}</p>
                <p className="text-[10px] text-muted-foreground">
                  {(report.file_size / 1024).toFixed(1)} KB
                </p>
              </div>
              <Button
                variant="ghost"
                size="sm"
                className="h-6 gap-1 px-2 text-[10px]"
                onClick={() => void handleViewCrash(report.filename)}
              >
                <Eye className="h-3 w-3" />
                {t("settings.about.view")}
              </Button>
              <Button
                variant={armedCrashDelete === report.filename ? "destructive" : "ghost"}
                size="sm"
                className="h-6 gap-1 px-2 text-[10px] text-destructive hover:bg-destructive/10"
                onClick={() => void handleDeleteCrash(report.filename)}
                title={
                  armedCrashDelete === report.filename
                    ? t("settings.about.confirmDeleteCrash", { defaultValue: "再次点击确认删除" })
                    : t("common.delete")
                }
              >
                <Trash2 className="h-3 w-3" />
              </Button>
            </div>
          ))}
        </div>
      )}

      {crashContent && (
        <div className="mt-2 rounded-md border border-border bg-muted/40 p-2">
          <div className="mb-1 flex items-center justify-between">
            <p className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground/60">
              {t("settings.about.crashContent")}
            </p>
            <Button
              variant="ghost"
              size="sm"
              className="h-5 gap-1 px-1.5 text-[10px]"
              onClick={() => setCrashContent(null)}
            >
              {t("common.close")}
            </Button>
          </div>
          <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-words font-mono text-[10px] text-muted-foreground">
            {crashContent}
          </pre>
        </div>
      )}
    </section>

  );
}
