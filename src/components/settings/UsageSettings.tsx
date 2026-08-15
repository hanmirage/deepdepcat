import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { BarChart3, TrendingUp, Coins, Wrench, RefreshCw } from "lucide-react";
import { sessionApi, type GlobalUsageSummary, type SessionUsageSummary } from "@/lib/tauri";
import { useChatStore } from "@/stores/chatStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { DeepSeekIcon } from "@/components/icons/DeepSeekIcon";
import { CrashReportsSection } from "@/components/settings/AboutSettingsCrashSection";
import { calculateCost, DEEPSEEK_PRICING } from "@/config/models";
import { cn } from "@/lib/utils";

export interface UsageSettingsProps {
  className?: string;
}

export function UsageSettings({ className }: UsageSettingsProps) {
  const { t } = useTranslation();
  const [usage, setUsage] = useState<GlobalUsageSummary | null>(null);
  const [sessionUsage, setSessionUsage] = useState<SessionUsageSummary | null>(null);
  const currentSessionId = useChatStore((s) => s.currentSessionId);
  const providers = useSettingsStore((s) => s.providers);
  const isStreaming = useChatStore((s) => s.isStreaming);
  const prevStreaming = useRef(isStreaming);

  const fetchUsage = () => {
    sessionApi
      .getGlobalUsage()
      .then(setUsage)
      .catch(() => setUsage(null));
    // The current session's per-request cache history powers the prefix
    // stability strip — only meaningful for the active session.
    if (currentSessionId) {
      sessionApi
        .getSessionUsage(currentSessionId)
        .then(setSessionUsage)
        .catch(() => setSessionUsage(null));
    }
  };

  // Fetch on mount.
  useEffect(() => {
    fetchUsage();
  }, []);

  // Refetch when a stream turn ends — new usage is only final then.
  useEffect(() => {
    const ended = prevStreaming.current && !isStreaming;
    prevStreaming.current = isStreaming;
    if (ended) fetchUsage();
  }, [isStreaming]);

  const totalTokens = usage
    ? usage.prompt_tokens + usage.completion_tokens
    : 0;
  // Tool results are ESTIMATED (chars/4) — never mixed into the real total.
  const toolResultTokens = usage?.tool_result_tokens ?? 0;
  // Cost estimate — use DeepSeek's REAL pricing (V4-Pro as the conservative
  // upper bound) instead of a generic $10/$30 placeholder that overstated
  // the cost ~25–75× (the "$41" scare). Billed tokens only (prompt +
  // completion), never the locally-estimated tool-result tokens.
  const estimatedCost = usage
    ? calculateCost(
        usage.prompt_tokens,
        usage.completion_tokens,
        DEEPSEEK_PRICING["deepseek-v4-pro"],
      ).toFixed(2)
    : "0.00";

  const deepseekEnabled = providers.some((p) => p.id === "deepseek" && p.enabled);
  const cacheHit = usage?.cache_hit_tokens ?? 0;
  const cacheMiss = usage?.cache_miss_tokens ?? 0;
  const cacheTotal = cacheHit + cacheMiss;
  const hitRate = cacheTotal > 0 ? (cacheHit / cacheTotal) * 100 : 0;
  const savedCost = (cacheHit / 1_000_000) * (3 - 0.025);
  const brokeCount =
    sessionUsage?.cache_history?.filter((r) => r.invalidated).length ?? 0;

  return (
    <div className={cn("space-y-6", className)}>
      <div className="flex items-center justify-between">
        <p className="text-xs text-muted-foreground">
          {t("settings.usage.headerDesc")}
        </p>
        <button
          onClick={fetchUsage}
          className="text-xs text-muted-foreground hover:text-foreground"
          title={t("common.refresh")}
        >
          <RefreshCw className="h-3.5 w-3.5" />
        </button>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div className="rounded-lg border border-border p-3">
          <div className="flex items-center gap-2">
            <BarChart3 className="h-4 w-4 text-muted-foreground" />
            <p className="text-[10px] text-muted-foreground">{t("settings.usage.totalTokens")}</p>
          </div>
          <p className="mt-1 text-lg font-semibold">
            {totalTokens.toLocaleString()}
          </p>
        </div>
        <div className="rounded-lg border border-border p-3">
          <div className="flex items-center gap-2">
            <Coins className="h-4 w-4 text-muted-foreground" />
            <p className="text-[10px] text-muted-foreground">{t("settings.usage.estimatedCost")}</p>
          </div>
          <p className="mt-1 text-lg font-semibold">${estimatedCost}</p>
          <p className="mt-0.5 text-[9px] text-muted-foreground">
            {t("settings.usage.estimatedCostNote")}
          </p>
        </div>
        <div className="rounded-lg border border-border p-3">
          <div className="flex items-center gap-2">
            <Wrench className="h-4 w-4 text-muted-foreground" />
            <p className="text-[10px] text-muted-foreground">{t("settings.usage.toolCalls")}</p>
          </div>
          <p className="mt-1 text-lg font-semibold">{(usage?.tool_calls ?? 0).toLocaleString()}</p>
        </div>
        <div className="rounded-lg border border-border p-3">
          <div className="flex items-center gap-2">
            <TrendingUp className="h-4 w-4 text-muted-foreground" />
            <p className="text-[10px] text-muted-foreground">{t("settings.usage.turns")}</p>
          </div>
          <p className="mt-1 text-lg font-semibold">{(usage?.turns ?? 0).toLocaleString()}</p>
        </div>
      </div>

      <div className="rounded-lg border border-border p-4">
        <p className="mb-3 text-xs font-semibold">{t("settings.usage.tokenDetail")}</p>
        <div className="space-y-2">
          <div className="flex justify-between text-xs">
            <span className="text-muted-foreground">{t("settings.usage.promptTokens")}</span>
            <span>{(usage?.prompt_tokens ?? 0).toLocaleString()}</span>
          </div>
          <div className="flex justify-between text-xs">
            <span className="text-muted-foreground">{t("settings.usage.completionTokens")}</span>
            <span>{(usage?.completion_tokens ?? 0).toLocaleString()}</span>
          </div>
          <div className="flex justify-between text-xs">
            <span className="text-muted-foreground">
              {t("settings.usage.toolResultTokens")} <span className="text-[9px]">({t("settings.usage.estimated")})</span>
            </span>
            <span>{toolResultTokens.toLocaleString()}</span>
          </div>
          <div className="flex justify-between border-t border-border pt-2 text-xs font-medium">
            <span>{t("settings.usage.totalBilled")}</span>
            <span>{totalTokens.toLocaleString()}</span>
          </div>
        </div>

        {/* Why prompt tokens look big — fixed overhead per request */}
        <p className="mt-3 rounded-md bg-muted/40 px-2.5 py-2 text-[10px] leading-relaxed text-muted-foreground">
          {t("settings.usage.fixedOverhead")}
        </p>
      </div>

      {deepseekEnabled && (
        <div className="rounded-lg border border-emerald-500/30 bg-emerald-500/5 p-4">
          <div className="mb-3 flex items-center gap-2">
            <DeepSeekIcon className="h-4 w-4 text-emerald-500" />
            <p className="text-xs font-semibold text-emerald-600 dark:text-emerald-400">
              {t("settings.usage.deepseekNative")}
            </p>
          </div>

          <div className="mb-3">
            <div className="mb-1 flex items-center justify-between text-xs">
              <span className="text-muted-foreground">{t("settings.usage.cacheHitRate")}</span>
              <span className="font-medium">{hitRate.toFixed(1)}%</span>
            </div>
            <div className="h-2 rounded-full bg-muted">
              <div
                className="h-full rounded-full bg-emerald-500 transition-all"
                style={{ width: `${hitRate}%` }}
              />
            </div>
          </div>

          <div className="grid grid-cols-3 gap-3 text-xs">
            <div>
              <p className="flex items-center gap-1 text-muted-foreground">
                <DeepSeekIcon className="h-3 w-3 text-emerald-500" />
                {t("settings.usage.cacheHit")}
              </p>
              <p className="mt-0.5 font-medium">{cacheHit.toLocaleString()}</p>
            </div>
            <div>
              <p className="text-muted-foreground">{t("settings.usage.cacheMiss")}</p>
              <p className="mt-0.5 font-medium">{cacheMiss.toLocaleString()}</p>
            </div>
            <div>
              <p className="text-muted-foreground">{t("settings.usage.savedCost")}</p>
              <p className="mt-0.5 font-medium text-emerald-600 dark:text-emerald-400">
                ¥{savedCost.toFixed(2)}
              </p>
            </div>
          </div>

          <div className="mt-3 border-t border-border/50 pt-2">
            <p className="text-[10px] text-muted-foreground">{t("settings.usage.pricingRef")}</p>
            <div className="mt-1 space-y-0.5 text-[10px] text-muted-foreground">
              <p>{t("settings.usage.pricingPro")}</p>
              <p>{t("settings.usage.pricingFlash")}</p>
            </div>
          </div>

          {sessionUsage?.cache_history && sessionUsage.cache_history.length > 0 && (
            <div className="mt-3 border-t border-border/50 pt-2">
              <div className="mb-1 flex items-center justify-between text-[10px] text-muted-foreground">
                <span>{t("settings.usage.cacheHistory", { defaultValue: "近期请求缓存稳定性" })}</span>
                <span>
                  {brokeCount > 0
                    ? t("settings.usage.cacheBroke", {
                        defaultValue: "前缀失效 {{count}} 次",
                        count: brokeCount,
                      })
                    : t("settings.usage.cacheStable", { defaultValue: "前缀稳定" })}
                </span>
              </div>
              <div className="flex h-3 gap-px overflow-hidden rounded-md bg-muted/60">
                {sessionUsage.cache_history.map((r, i) => {
                  const total = r.hit_tokens + r.miss_tokens;
                  const hitPct = total > 0 ? (r.hit_tokens / total) * 100 : 0;
                  return (
                    <div
                      key={i}
                      className="relative h-full min-w-[3px] flex-1"
                      title={`#${i + 1}: hit ${r.hit_tokens} / miss ${r.miss_tokens}`}
                    >
                      <div
                        className="absolute inset-y-0 left-0 bg-emerald-500/80"
                        style={{ width: `${hitPct}%` }}
                      />
                      {r.invalidated && (
                        <div className="absolute inset-0 ring-1 ring-inset ring-amber-500" />
                      )}
                    </div>
                  );
                })}
              </div>
              <p className="mt-1 text-[9px] leading-snug text-muted-foreground">
                {t("settings.usage.cacheHistoryDesc", {
                  defaultValue:
                    "每条 = 一次请求：绿色=缓存命中、底色=重算；琥珀描边=该请求前缀失效（提示词/上下文变动导致）",
                })}
              </p>
            </div>
          )}
        </div>
      )}

      <CrashReportsSection />
    </div>
  );
}
