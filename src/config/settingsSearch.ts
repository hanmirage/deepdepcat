import { SETTINGS_CATEGORIES, SETTINGS_GROUPS, type SettingsCategory, type SettingsCategoryDef } from "./settings";

export interface SettingsSearchEntry {
  /** i18n key of the setting label, e.g. "settings.general.theme". */
  key: string;
  /** Optional i18n key of the setting description. */
  descKey?: string;
}

export interface SettingsSearchMatch {
  entry: SettingsSearchEntry;
  /** Translated label (current UI language) for display. */
  label: string;
  /** Translated description (current UI language) for display. */
  desc?: string;
}

export interface SettingsSearchResult {
  category: SettingsCategoryDef;
  /** Translated group label, shown as context. */
  groupLabel: string;
  /** True when the category or its group name matched the query. */
  categoryMatched: boolean;
  entryMatches: SettingsSearchMatch[];
}

export type Translate = (key: string) => string;

/**
 * Static search index for the settings view. Each category lists the i18n
 * keys of its settings (label + optional description). The index is what
 * makes search possible without eagerly loading every lazy category page.
 */
export const SETTINGS_SEARCH_INDEX: Record<SettingsCategory, SettingsSearchEntry[]> = {
  appearance: [
    { key: "settings.general.theme", descKey: "settings.general.themeDesc" },
    { key: "settings.general.titleBarStyle", descKey: "settings.general.titleBarStyleDesc" },
    { key: "settings.general.workMode", descKey: "settings.general.workModeDesc" },
    { key: "settings.general.language", descKey: "settings.general.languageDesc" },
    { key: "settings.general.showThinking", descKey: "settings.general.showThinkingDesc" },
    { key: "settings.general.showTodo", descKey: "settings.general.showTodoDesc" },
    { key: "settings.general.streamingSpeed", descKey: "settings.general.streamingSpeedDesc" },
    { key: "settings.general.deepseekAutoReasoning", descKey: "settings.general.deepseekAutoReasoningDesc" },
    { key: "settings.about.onboarding", descKey: "settings.about.onboardingDesc" },
  ],
  models: [
    { key: "settings.modelProviders.headerDesc" },
    { key: "settings.modelProviders.name" },
    { key: "settings.modelProviders.baseUrl" },
    { key: "settings.modelProviders.apiKey" },
    { key: "settings.modelProviders.apiFormat" },
    { key: "settings.modelProviders.fetchModels" },
    { key: "settings.modelProviders.manualAddModel" },
    { key: "settings.modelProviders.contextWindow" },
    { key: "settings.modelProviders.modelList" },
    { key: "settings.modelProviders.deleteProvider" },
    { key: "settings.modelProviders.disabled" },
  ],
  vision: [
    { key: "settings.modelProviders.visionDesc" },
    { key: "settings.modelProviders.visionEnabled" },
    { key: "settings.modelProviders.visionPreset" },
    { key: "settings.modelProviders.visionApiKey" },
    { key: "settings.modelProviders.visionFreeHint" },
    { key: "settings.modelProviders.visionCustomHint" },
  ],
  limits: [
    { key: "settings.general.agentLimits" },
    { key: "settings.general.sessionTokenLimit", descKey: "settings.general.sessionTokenLimitDesc" },
    { key: "settings.general.sessionCostLimit", descKey: "settings.general.sessionCostLimitDesc" },
    { key: "settings.general.maxConcurrentTools", descKey: "settings.general.maxConcurrentToolsDesc" },
    { key: "settings.general.turnOutputLimit", descKey: "settings.general.turnOutputLimitDesc" },
  ],
  network: [
    { key: "settings.general.httpProxy", descKey: "settings.general.httpProxyDesc" },
    { key: "settings.general.noProxy", descKey: "settings.general.noProxyDesc" },
    { key: "settings.general.cloudSync", descKey: "settings.general.cloudSyncDesc" },
    { key: "settings.general.cloudSyncButton" },
    { key: "settings.general.privacy" },
    { key: "settings.general.diagnosticsEnabled", descKey: "settings.general.diagnosticsEnabledDesc" },
    { key: "settings.general.acp" },
    { key: "settings.general.acpEnabled", descKey: "settings.general.acpEnabledDesc" },
    { key: "settings.general.acpPort", descKey: "settings.general.acpPortDesc" },
    { key: "settings.general.acpHint" },
    { key: "settings.general.a2a" },
    { key: "settings.general.a2aEnabled", descKey: "settings.general.a2aEnabledDesc" },
    { key: "settings.general.a2aPort", descKey: "settings.general.a2aPortDesc" },
    { key: "settings.general.a2aHint" },
  ],
  memory: [
    { key: "settings.memory.title" },
    { key: "settings.memory.search" },
    { key: "settings.memory.dream" },
    { key: "settings.memory.store" },
    { key: "settings.memory.category" },
    { key: "settings.memory.total" },
    { key: "settings.general.memoryWeights", descKey: "settings.general.memoryWeightsDesc" },
    { key: "settings.general.weightBm25" },
    { key: "settings.general.weightCosine" },
    { key: "settings.general.weightRecency" },
    { key: "settings.general.recencyHalfLife", descKey: "settings.general.recencyHalfLifeDesc" },
  ],
  permissions: [
    { key: "settings.permissions.grantsTitle", descKey: "settings.permissions.grantsDesc" },
    { key: "settings.permissions.policyTitle", descKey: "settings.permissions.policyDesc" },
    { key: "settings.permissions.rulesTitle", descKey: "settings.permissions.rulesDesc" },
    { key: "settings.permissions.addRule" },
    { key: "settings.permissions.saveRules" },
    { key: "settings.permissions.clearAll" },
    { key: "settings.permissions.revoke" },
    { key: "settings.general.circuitBreakers", descKey: "settings.general.circuitBreakersDesc" },
  ],
  agents: [
    { key: "settings.agents.title", descKey: "settings.agents.desc" },
    { key: "settings.agents.builtin" },
    { key: "settings.agents.custom" },
    { key: "settings.agents.tools" },
    { key: "settings.agents.promptMode" },
    { key: "settings.agents.promptFull" },
    { key: "settings.agents.promptExtend" },
  ],
  skills: [
    { key: "settings.skillsDesc" },
    { key: "settings.skillsEcoCompat", descKey: "settings.skillsEcoCompatDesc" },
  ],
  hooks: [
    { key: "settings.hooksDesc" },
    { key: "settings.hooksCondition" },
    { key: "settings.hooksEvent" },
    { key: "settings.hooksType" },
    { key: "settings.hooksTimeout" },
    { key: "settings.hooksPreview" },
  ],
  "mcp-servers": [
    { key: "settings.mcp.headerDesc" },
    { key: "settings.mcp.emptyTitle", descKey: "settings.mcp.emptyDesc" },
    { key: "settings.mcp.addServer" },
    { key: "settings.mcp.serverName" },
    { key: "settings.mcp.command" },
    { key: "settings.mcp.args" },
    { key: "settings.mcp.transport" },
    { key: "settings.mcp.presets" },
    { key: "settings.mcp.connect" },
    { key: "settings.mcp.disconnect" },
    { key: "settings.mcp.credentialButton" },
    { key: "settings.mcp.toolsCount" },
  ],
  connectors: [
    { key: "customize.connectors" },
    { key: "customize.connectorsActive" },
    { key: "customize.connect" },
    { key: "customize.connected" },
  ],
  plugins: [
    { key: "customize.plugins" },
    { key: "customize.pluginsInstalled" },
    { key: "customize.available" },
    { key: "customize.install" },
    { key: "customize.installed" },
  ],
  usage: [
    { key: "settings.usage.headerDesc" },
    { key: "settings.usage.estimatedCost" },
    { key: "settings.usage.totalTokens" },
    { key: "settings.usage.promptTokens" },
    { key: "settings.usage.completionTokens" },
    { key: "settings.usage.toolCalls" },
    { key: "settings.usage.turns" },
    { key: "settings.usage.cacheHitRate" },
    { key: "settings.usage.deepseekNative" },
    { key: "settings.about.crashReports" },
  ],
  about: [
    { key: "settings.about.title" },
    { key: "settings.about.currentVersion" },
    { key: "settings.about.checkForUpdate" },
    { key: "settings.about.updates" },
    { key: "settings.about.systemInfo" },
    { key: "settings.about.feedback" },
    { key: "settings.about.officialSite" },
  ],
};

function normalize(value: string): string {
  return value.trim().toLowerCase();
}

function textMatches(text: string, query: string): boolean {
  return normalize(text).includes(query);
}

/**
 * Search the settings index. `translate` renders keys in the current UI
 * language (used for display); `translateAlt` renders them in the other
 * bundled language so technical English terms (proxy, token, theme) stay
 * findable even when the UI is Chinese.
 */
export function searchSettings(
  query: string,
  translate: Translate,
  translateAlt: Translate,
): SettingsSearchResult[] {
  const q = normalize(query);
  if (!q) return [];

  const results: SettingsSearchResult[] = [];
  for (const category of SETTINGS_CATEGORIES) {
    const group = SETTINGS_GROUPS.find((g) => g.items.some((item) => item.id === category.id));
    if (!group) continue;
    const groupLabel = translate(group.label);
    const categoryMatched =
      textMatches(translate(category.label), q) ||
      textMatches(translateAlt(category.label), q) ||
      textMatches(groupLabel, q) ||
      textMatches(translateAlt(group.label), q);

    const entryMatches: SettingsSearchMatch[] = [];
    for (const entry of SETTINGS_SEARCH_INDEX[category.id] ?? []) {
      const label = translate(entry.key);
      const desc = entry.descKey ? translate(entry.descKey) : undefined;
      const labelMatched =
        textMatches(label, q) || textMatches(translateAlt(entry.key), q);
      const descMatched = entry.descKey
        ? textMatches(desc ?? "", q) || textMatches(translateAlt(entry.descKey), q)
        : false;
      if (labelMatched || descMatched) {
        entryMatches.push({ entry, label, desc });
      }
    }

    if (categoryMatched || entryMatches.length > 0) {
      results.push({ category, groupLabel, categoryMatched, entryMatches });
    }
  }
  return results;
}
