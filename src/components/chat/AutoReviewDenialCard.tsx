/**
 * AutoReviewDenialCard — surfaces independent-reviewer denials.
 *
 * Auto-Review swaps the human out of gray-zone approvals; when it denies,
 * the user still gets a narrow escape hatch: 「仍要允许一次」records an
 * exact-action session grant so the agent's retry of the SAME call passes
 * (one retry, session-scoped, dangerous classes stay un-grantable).
 */

import { useTranslation } from "react-i18next";
import { ShieldAlert, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  denialKey,
  usePermissionStore,
  visibleDenials,
} from "@/stores/permissionStore";

export function AutoReviewDenialCard({
  sessionId,
}: {
  sessionId: string | null | undefined;
}) {
  const { t } = useTranslation();
  const denials = usePermissionStore((s) => s.denials);
  const visible = visibleDenials(denials, sessionId);
  if (visible.length === 0) return null;

  return (
    <div className="space-y-2">
      {visible.map((denial) => {
        const key = denialKey(denial);
        return (
          <div
            key={key}
            className="flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2"
          >
            <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0 text-amber-600" />
            <div className="min-w-0 flex-1">
              <p className="text-xs font-medium">
                {t("permission.autoReviewDeniedTitle", { defaultValue: "Auto-Review 拒绝了操作" })}
              </p>
              <p className="mt-0.5 break-words text-[11px] text-muted-foreground">
                <span className="font-mono">{denial.tool_name}</span>
                {denial.reason ? ` — ${denial.reason}` : ""}
              </p>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <Button
                size="sm"
                variant="outline"
                className="h-6 px-2 text-[11px]"
                onClick={() => void usePermissionStore.getState().overrideDenial(denial)}
              >
                {t("permission.autoReviewAllowOnce", { defaultValue: "仍要允许一次" })}
              </Button>
              <button
                className="rounded p-1 text-muted-foreground/60 hover:text-foreground"
                onClick={() => usePermissionStore.getState().dismissDenial(key)}
                aria-label={t("common.close")}
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          </div>
        );
      })}
    </div>
  );
}
