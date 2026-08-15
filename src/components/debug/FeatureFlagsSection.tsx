/**
 * FeatureFlagsSection — developer feature-flag toggles, surfaced in the
 * DebugPanel (moved out of the About settings panel). Collapsed by default;
 * only rendered when the backend exposes flags.
 */

import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Flag } from "lucide-react";
import { useAppStore } from "@/stores/appStore";
import { featureFlagApi, type FeatureFlag } from "@/lib/tauri";
import { Switch } from "@/components/ui/switch";

export function FeatureFlagsSection() {
  const { t } = useTranslation();
  const setFeatureFlag = useAppStore((s) => s.setFeatureFlag);
  const featureFlags = useAppStore((s) => s.featureFlags);
  const [flagList, setFlagList] = useState<FeatureFlag[]>([]);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    featureFlagApi
      .list()
      .then((flags) => setFlagList(flags))
      .catch(() => setFlagList([]));
  }, []);

  if (flagList.length === 0) return null;

  return (
    <div className="border-b border-border px-3 py-1.5">
      <button
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-1.5 text-[10px] text-muted-foreground transition-colors hover:text-foreground"
        aria-expanded={open}
      >
        <Flag className="h-3 w-3" />
        {t("settings.about.featureFlags")}
      </button>
      {open && (
        <div className="mt-1.5 space-y-1.5">
          {flagList.map((flag) => (
            <div key={flag.key} className="flex items-center gap-2">
              <div className="min-w-0 flex-1">
                <p className="truncate font-mono text-[10px]">{flag.key}</p>
                {flag.description && (
                  <p className="truncate text-[9px] text-muted-foreground">
                    {flag.description}
                  </p>
                )}
              </div>
              <Switch
                checked={featureFlags[flag.key] ?? flag.enabled}
                onCheckedChange={(v) => void setFeatureFlag(flag.key, v)}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
