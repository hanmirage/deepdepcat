import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useSettingsStore } from "@/stores/settingsStore";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { SettingSelect } from "@/components/settings/SettingSelect";
import { Eye, EyeOff, CheckCircle2, ChevronDown, ChevronRight } from "lucide-react";
import { cn } from "@/lib/utils";
import {
  VISION_PRESETS,
  CUSTOM_VISION_PRESET_ID,
  matchVisionPreset,
} from "@/config/visionPresets";

function SecretInput({
  value,
  onChange,
  placeholder,
  className,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  className?: string;
}) {
  const { t } = useTranslation();
  const [show, setShow] = useState(false);
  return (
    <div className="relative">
      <Input
        type={show ? "text" : "password"}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        className={cn("pr-7", className)}
      />
      <button
        type="button"
        onClick={() => setShow(!show)}
        className="absolute right-2 top-1/2 -translate-y-1/2 text-muted-foreground hover:text-foreground"
        aria-label={show ? t("settings.modelProviders.hideKey") : t("settings.modelProviders.showKey")}
      >
        {show ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
      </button>
    </div>
  );
}

/**
 * Vision model settings — a self-contained OpenAI-compatible multimodal
 * endpoint used by the `visual_describe` tool. Kept separate from the chat
 * model providers because the main model may be text-only (DeepSeek, GLM
 * text) while this is what lets the agent "see" images.
 */
export function VisionSettings({ className }: { className?: string }) {
  const { t } = useTranslation();
  const vision = useSettingsStore((s) => s.vision);
  const updateVision = useSettingsStore((s) => s.updateVision);
  const [isExpanded, setIsExpanded] = useState(true);

  return (
    <div className={cn("space-y-4", className)}>
      <div className="flex items-start justify-between">
        <p className="text-xs text-muted-foreground">
          {t("settings.modelProviders.visionDesc")}
        </p>
      </div>

      <div className="overflow-hidden rounded-lg border border-border">
        <div className="flex items-center gap-2 p-3">
          <button
            onClick={() => setIsExpanded(!isExpanded)}
            className="text-muted-foreground hover:text-foreground"
            aria-label={isExpanded ? t("common.collapse") : t("common.expand")}
          >
            {isExpanded ? <ChevronDown className="h-3.5 w-3.5" /> : <ChevronRight className="h-3.5 w-3.5" />}
          </button>
          <div className="flex-1">
            <div className="flex items-center gap-1.5">
              <p className="text-sm font-semibold">{t("settings.modelProviders.visionBlock")}</p>
            </div>
          </div>
          <Switch
            checked={vision.enabled}
            onCheckedChange={(checked) => updateVision({ enabled: checked })}
            aria-label={t("settings.modelProviders.visionEnabled")}
          />
        </div>

        {isExpanded && (
          <div className="space-y-3 border-t border-border p-3">
            <div>
              <label className="mb-1 block text-xs text-muted-foreground">
                {t("settings.modelProviders.visionPreset")}
              </label>
              <SettingSelect
                value={matchVisionPreset(vision.baseUrl, vision.model)?.id ?? CUSTOM_VISION_PRESET_ID}
                onChange={(value) => {
                  if (value === CUSTOM_VISION_PRESET_ID) return;
                  const preset = VISION_PRESETS.find((p) => p.id === value);
                  if (preset) {
                    // Picking a free preset fills base URL + model and turns
                    // the vision model on — the user only adds their own key.
                    // Base URL / model are not exposed in the UI; presets (or
                    // a legacy custom config) own those values.
                    updateVision({
                      enabled: true,
                      baseUrl: preset.baseUrl,
                      model: preset.model,
                    });
                  }
                }}
                options={[
                  ...VISION_PRESETS.map((p) => ({
                    value: p.id,
                    label: `${t(p.labelKey)} — ${t(p.descKey)}`,
                  })),
                  // Keep a read-only custom entry so a legacy config that
                  // does not match any preset still displays correctly.
                  ...(matchVisionPreset(vision.baseUrl, vision.model)
                    ? []
                    : [
                        {
                          value: CUSTOM_VISION_PRESET_ID,
                          label: t("settings.modelProviders.visionPresetCustom"),
                        },
                      ]),
                ]}
              />
              {!matchVisionPreset(vision.baseUrl, vision.model) && (
                <p className="mt-1 text-[10px] text-muted-foreground/70">
                  {t("settings.modelProviders.visionCustomHint", {
                    defaultValue: "当前为自定义配置，请直接填写下方 Base URL 与模型名",
                  })}
                </p>
              )}
            </div>
            <div>
              <label className="mb-1 block text-xs text-muted-foreground">
                {t("settings.modelProviders.visionApiKey")}
              </label>
              <SecretInput
                value={vision.apiKey}
                onChange={(value) => {
                  // First non-empty key input flips the vision model on —
                  // users who only pasted a key shouldn't wonder why images
                  // still fail (the preset selector is pre-selected).
                  if (value.trim() && !vision.enabled) {
                    updateVision({ apiKey: value, enabled: true });
                  } else {
                    updateVision({ apiKey: value });
                  }
                }}
                placeholder={t("settings.modelProviders.apiKeyPlaceholder")}
              />
            </div>
            <p className="flex items-start gap-1.5 rounded-md bg-muted/40 p-2 text-[11px] text-muted-foreground">
              <CheckCircle2 className="mt-0.5 h-3 w-3 shrink-0 text-emerald-500" />
              {t("settings.modelProviders.visionFreeHint")}
            </p>
          </div>
        )}
      </div>
    </div>
  );
}
