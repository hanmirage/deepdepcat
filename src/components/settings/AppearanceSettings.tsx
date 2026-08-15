/**
 * AppearanceSettings — how the app looks and behaves day-to-day:
 * theme, title bar style, work mode, language, and display toggles
 * (thinking panel, todo list, DeepSeek auto-reasoning).
 */

import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Lightbulb } from "lucide-react";
import { useAppStore } from "@/stores/appStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useOnboardingStore } from "@/stores/onboardingStore";
import type { AccentName } from "@/stores/appStoreSlices/types";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { SettingRow } from "@/components/settings/SettingRow";
import { SettingSelect } from "@/components/settings/SettingSelect";
import { cn } from "@/lib/utils";

/** Accent swatch preview colors — the app's primary hue per preset (light). */
const ACCENT_COLORS: Record<AccentName, string> = {
  violet: "hsl(262 70% 58%)",
  blue: "hsl(221 83% 53%)",
  teal: "hsl(180 70% 42%)",
  green: "hsl(145 60% 40%)",
  amber: "hsl(32 85% 50%)",
  rose: "hsl(335 70% 50%)",
};

const ACCENT_NAMES: AccentName[] = ["violet", "blue", "teal", "green", "amber", "rose"];

export interface AppearanceSettingsProps {
  className?: string;
}

export function AppearanceSettings({ className }: AppearanceSettingsProps) {
  const { t } = useTranslation();
  const general = useSettingsStore((s) => s.general);
  const updateGeneral = useSettingsStore((s) => s.updateGeneral);
  const theme = useAppStore((s) => s.theme);
  const setTheme = useAppStore((s) => s.setTheme);
  const accent = useAppStore((s) => s.accent);
  const setAccent = useAppStore((s) => s.setAccent);
  const mode = useAppStore((s) => s.mode);
  const setMode = useAppStore((s) => s.setMode);
  const resetOnboarding = useOnboardingStore((s) => s.reset);
  // Two-step confirm — re-running onboarding resets the first-run guide.
  const [armedResetOnboarding, setArmedResetOnboarding] = useState(false);
  const handleResetOnboarding = () => {
    if (!armedResetOnboarding) {
      setArmedResetOnboarding(true);
      setTimeout(() => setArmedResetOnboarding(false), 3000);
      return;
    }
    setArmedResetOnboarding(false);
    resetOnboarding();
  };

  return (
    <div className={cn("space-y-1", className)}>
      <SettingRow
        searchKey="settings.general.theme"
        label={t("settings.general.theme")}
        description={t("settings.general.themeDesc")}
      >
        <SettingSelect
          value={theme}
          onChange={(v) => setTheme(v as "light" | "dark")}
          options={[
            { value: "light", label: t("settings.general.themeLight") },
            { value: "dark", label: t("settings.general.themeDark") },
          ]}
        />
      </SettingRow>

      {/* Accent color — swatch buttons, not a dropdown (colors are visual). */}
      <SettingRow
        searchKey="settings.general.accent"
        label={t("settings.general.accent")}
        description={t("settings.general.accentDesc")}
      >
        <div className="flex flex-wrap items-center gap-2">
          {ACCENT_NAMES.map((name) => (
            <button
              key={name}
              type="button"
              onClick={() => setAccent(name)}
              aria-pressed={accent === name}
              title={t(`settings.general.accent${name[0].toUpperCase()}${name.slice(1)}`)}
              className={cn(
                "flex items-center gap-1.5 rounded-md border px-1.5 py-1 transition-colors",
                accent === name
                  ? "border-ring bg-muted/60"
                  : "border-border hover:bg-muted/30",
              )}
            >
              <span
                className="h-3.5 w-3.5 shrink-0 rounded-full"
                style={{ background: ACCENT_COLORS[name] }}
                aria-hidden="true"
              />
              <span className="text-[11px] text-foreground/80">
                {t(`settings.general.accent${name[0].toUpperCase()}${name.slice(1)}`)}
              </span>
            </button>
          ))}
        </div>
      </SettingRow>

      <SettingRow
        searchKey="settings.general.titleBarStyle"
        label={t("settings.general.titleBarStyle")}
        description={t("settings.general.titleBarStyleDesc")}
      >
        <SettingSelect
          value={general.titleBarStyle}
          onChange={(v) => updateGeneral({ titleBarStyle: v as "mac" | "windows" })}
          options={[
            { value: "windows", label: t("settings.general.titleBarWindows") },
            { value: "mac", label: t("settings.general.titleBarMac") },
          ]}
        />
      </SettingRow>

      <SettingRow
        searchKey="settings.general.workMode"
        label={t("settings.general.workMode")}
        description={t("settings.general.workModeDesc")}
      >
        <SettingSelect
          value={mode}
          onChange={(v) => setMode(v as "code" | "depwork")}
          options={[
            { value: "code", label: t("settings.general.workModeCode") },
            { value: "depwork", label: t("settings.general.workModeDepwork") },
          ]}
        />
      </SettingRow>

      <SettingRow
        searchKey="settings.general.language"
        label={t("settings.general.language")}
        description={t("settings.general.languageDesc")}
      >
        <SettingSelect
          value={general.language}
          onChange={(v) => updateGeneral({ language: v as "en" | "zh" })}
          options={[
            { value: "zh", label: t("settings.general.languageZh") },
            { value: "en", label: t("settings.general.languageEn") },
          ]}
        />
      </SettingRow>

      <SettingRow
        searchKey="settings.general.showThinking"
        label={t("settings.general.showThinking")}
        description={t("settings.general.showThinkingDesc")}
      >
        <Switch
          checked={general.showThinking}
          onCheckedChange={(v) => updateGeneral({ showThinking: v })}
        />
      </SettingRow>

      <SettingRow
        searchKey="settings.general.showTodo"
        label={t("settings.general.showTodo")}
        description={t("settings.general.showTodoDesc")}
      >
        <Switch
          checked={general.showTodo}
          onCheckedChange={(v) => updateGeneral({ showTodo: v })}
        />
      </SettingRow>

      <SettingRow
        searchKey="settings.general.streamingSpeed"
        label={t("settings.general.streamingSpeed")}
        description={t("settings.general.streamingSpeedDesc")}
      >
        <SettingSelect
          value={general.streamingSpeed}
          onChange={(v) => updateGeneral({ streamingSpeed: v as "smooth" | "instant" })}
          options={[
            { value: "smooth", label: t("settings.general.streamingSmooth") },
            { value: "instant", label: t("settings.general.streamingInstant") },
          ]}
        />
      </SettingRow>

      <SettingRow
        searchKey="settings.general.deepseekAutoReasoning"
        label={t("settings.general.deepseekAutoReasoning")}
        description={t("settings.general.deepseekAutoReasoningDesc")}
      >
        <Switch
          checked={general.deepseekAutoReasoning}
          onCheckedChange={(v) => updateGeneral({ deepseekAutoReasoning: v })}
        />
      </SettingRow>

      {/* ── 重新运行引导（onboarding） ── */}
      <div className="border-t border-[hsl(var(--border))] pt-4">
        <div className="flex items-center gap-2">
          <p className="flex-1 text-[10px] text-muted-foreground">
            {t("settings.about.onboardingDesc")}
          </p>
          <Button
            variant={armedResetOnboarding ? "destructive" : "outline"}
            size="sm"
            className="h-7 gap-1.5 text-xs"
            onClick={handleResetOnboarding}
            title={armedResetOnboarding ? t("settings.about.confirmResetOnboarding", { defaultValue: "再次点击确认重置" }) : undefined}
          >
            <Lightbulb className="h-3.5 w-3.5" />
            {armedResetOnboarding
              ? t("settings.about.confirmResetOnboarding", { defaultValue: "再次点击确认重置" })
              : t("settings.about.reopenOnboarding")}
          </Button>
        </div>
      </div>
    </div>
  );
}
