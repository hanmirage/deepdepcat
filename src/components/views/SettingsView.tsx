import { useState, useEffect, useMemo, Suspense, lazy } from "react";
import { useTranslation } from "react-i18next";
import { ArrowLeft, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { SettingsSearchNav } from "@/components/settings/SettingsSearch";
import { useScrollToSearchKey } from "@/components/settings/useScrollToSearchKey";
import { useAppStore } from "@/stores/appStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { searchSettings } from "@/config/settingsSearch";
import {
  SETTINGS_GROUPS,
  SETTINGS_CATEGORIES,
  DEFAULT_SETTINGS_CATEGORY,
  type SettingsCategory,
} from "@/config/settings";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { cn } from "@/lib/utils";

// ── Lazy category pages ─────────────────────────────────────
// Each settings category is its own chunk — opening the settings view no
// longer loads every page's providers/MCP/hook editors up front.
const AppearanceSettings = lazy(() =>
  import("@/components/settings/AppearanceSettings").then((m) => ({ default: m.AppearanceSettings })),
);
const ModelProviderSettings = lazy(() =>
  import("@/components/settings/ModelProviderSettings").then((m) => ({ default: m.ModelProviderSettings })),
);
const VisionSettings = lazy(() =>
  import("@/components/settings/VisionSettings").then((m) => ({ default: m.VisionSettings })),
);
const LimitsSettings = lazy(() =>
  import("@/components/settings/LimitsSettings").then((m) => ({ default: m.LimitsSettings })),
);
const NetworkDataSettings = lazy(() =>
  import("@/components/settings/NetworkDataSettings").then((m) => ({ default: m.NetworkDataSettings })),
);
const SkillsSettings = lazy(() =>
  import("@/components/settings/SkillsSettings").then((m) => ({ default: m.SkillsSettings })),
);
const HookSettings = lazy(() =>
  import("@/components/settings/HookSettings").then((m) => ({ default: m.HookSettings })),
);
const McpServerSettings = lazy(() =>
  import("@/components/settings/McpServerSettings").then((m) => ({ default: m.McpServerSettings })),
);
const UsageSettings = lazy(() =>
  import("@/components/settings/UsageSettings").then((m) => ({ default: m.UsageSettings })),
);
const MemorySettings = lazy(() =>
  import("@/components/settings/MemorySettings").then((m) => ({ default: m.MemorySettings })),
);
const PermissionsSettings = lazy(() =>
  import("@/components/settings/PermissionsSettings").then((m) => ({ default: m.PermissionsSettings })),
);
const AgentSettings = lazy(() =>
  import("@/components/settings/AgentSettings").then((m) => ({ default: m.AgentSettings })),
);
const AboutSettings = lazy(() =>
  import("@/components/settings/AboutSettings").then((m) => ({ default: m.AboutSettings })),
);
const ConnectorsCard = lazy(() =>
  import("@/components/customize/ConnectorsCard").then((m) => ({ default: m.ConnectorsCard })),
);
const PluginsCard = lazy(() =>
  import("@/components/customize/PluginsCard").then((m) => ({ default: m.PluginsCard })),
);

/** localStorage key for the last visited settings category. */
const PREF_SETTINGS_CATEGORY = "deepdepcat.settingsCategory";

function loadLastSettingsCategory(): SettingsCategory {
  try {
    const saved = localStorage.getItem(PREF_SETTINGS_CATEGORY);
    if (saved && SETTINGS_CATEGORIES.some((c) => c.id === saved)) {
      return saved as SettingsCategory;
    }
  } catch {
    /* storage unavailable — fall back to the default */
  }
  return DEFAULT_SETTINGS_CATEGORY;
}

function saveSettingsCategory(category: SettingsCategory): void {
  try {
    localStorage.setItem(PREF_SETTINGS_CATEGORY, category);
  } catch {
    /* storage unavailable — the in-memory selection still works */
  }
}

export function SettingsView() {
  const { t, i18n } = useTranslation();
  const [activeCategory, setActiveCategoryState] =
    useState<SettingsCategory>(loadLastSettingsCategory);
  const [query, setQuery] = useState("");
  const [focusSearchKey, setFocusSearchKey] = useState<string | null>(null);
  const setActiveCategory = (category: SettingsCategory) => {
    setActiveCategoryState(category);
    saveSettingsCategory(category);
  };
  const setSettingsOpen = useAppStore((s) => s.setSettingsOpen);
  const settingsCategory = useAppStore((s) => s.settingsCategory);
  const clearSettingsCategory = useAppStore((s) => s.clearSettingsCategory);
  const initSettings = useSettingsStore((s) => s.init);
  // The vision-model category only appears when DeepSeek auto-optimization is
  // on (Appearance → DeepSeek 自动优化). The vision model exists to give a
  // TEXT main model (DeepSeek) an "eye" — most other models are multimodal
  // and need no separate vision config, so the category stays hidden when
  // the DeepSeek pipeline is off.
  const otherLang = i18n.language.startsWith("en") ? "zh" : "en";
  const translateAlt = i18n.getFixedT(otherLang);
  const searchResults = useMemo(
    () => searchSettings(query, t, translateAlt),
    [query, t, translateAlt],
  );
  const searching = query.trim().length > 0;

  const handleSelectResult = (category: SettingsCategory, entryKey?: string) => {
    setActiveCategory(category);
    setFocusSearchKey(entryKey ?? null);
  };

  useEffect(() => {
    void initSettings();
  }, [initSettings]);

  // External navigation (e.g. "manage" links from the right panel) can land
  // on a specific category via appStore.openSettings(category). It is a
  // one-shot request — apply it, then clear it so the next open of the
  // settings view starts at the default category.
  useEffect(() => {
    if (settingsCategory) {
      setActiveCategory(settingsCategory);
      clearSettingsCategory();
    }
  }, [settingsCategory, clearSettingsCategory]);

  const effectiveCategory: SettingsCategory = activeCategory;

  useScrollToSearchKey(focusSearchKey, effectiveCategory);

  const renderContent = () => {
    switch (effectiveCategory) {
      case "appearance":
        return <AppearanceSettings />;
      case "models":
        return <ModelProviderSettings />;
      case "vision":
        return <VisionSettings />;
      case "limits":
        return <LimitsSettings />;
      case "network":
        return <NetworkDataSettings />;
      case "memory":
        return <MemorySettings />;
      case "permissions":
        return <PermissionsSettings />;
      case "agents":
        return <AgentSettings />;
      case "skills":
        return <SkillsSettings />;
      case "hooks":
        return <HookSettings />;
      case "mcp-servers":
        return <McpServerSettings />;
      case "connectors":
        return <ConnectorsCard />;
      case "plugins":
        return <PluginsCard />;
      case "usage":
        return <UsageSettings />;
      case "about":
        return <AboutSettings />;
    }
  };

  const activeItem = SETTINGS_GROUPS.flatMap((g) => g.items).find(
    (c) => c.id === effectiveCategory,
  );

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-2 px-4 py-2.5 shadow-[var(--shadow-paper-sm)]">
        <Button
          variant="ghost"
          size="icon-sm"
          onClick={() => setSettingsOpen(false)}
          aria-label={t("settings.backToWorkspace")}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <h2 className="text-sm font-semibold">
          {activeItem ? t(activeItem.label) : t("settings.title")}
        </h2>
      </div>

      <div className="flex flex-1 overflow-hidden">
        <nav
          className={cn(
            "shrink-0 border-r border-[hsl(var(--border))] bg-[hsl(var(--sidebar-bg))] p-2 transition-[width] duration-150",
            searching ? "w-64" : "w-48",
          )}
        >
          <SettingsSearchNav
            query={query}
            onQueryChange={setQuery}
            results={searchResults}
            activeCategory={effectiveCategory}
            hideVision={false}
            onSelect={handleSelectResult}
          />
        </nav>

        <div className="flex-1 overflow-hidden bg-[hsl(var(--background))]">
          <ScrollArea className="h-full">
            <div className="mx-auto max-w-2xl p-6">
              <div className="paper-card p-4">
                <ErrorBoundary resetKey={effectiveCategory}>
                  <Suspense
                    fallback={
                      <div className="flex min-h-40 items-center justify-center">
                        <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
                      </div>
                    }
                  >
                    {renderContent()}
                  </Suspense>
                </ErrorBoundary>
              </div>
            </div>
          </ScrollArea>
        </div>
      </div>
    </div>
  );
}
