import type { LucideIcon } from "lucide-react";
import {
  Cpu,
  Server,
  BarChart3,
  Info,
  ListChecks,
  Plug,
  Puzzle,
  Gauge,
  Globe,
  Palette,
  Brain,
  Eye,
  Database,
  UserRound,
  Shield,
} from "lucide-react";

export type SettingsCategory =
  | "appearance"
  | "models"
  | "vision"
  | "limits"
  | "network"
  | "memory"
  | "permissions"
  | "agents"
  | "skills"
  | "hooks"
  | "mcp-servers"
  | "connectors"
  | "plugins"
  | "usage"
  | "about";

export interface SettingsCategoryDef {
  id: SettingsCategory;
  label: string;
  icon: LucideIcon;
}

export interface SettingsGroup {
  /** i18n key for the group header. */
  label: string;
  items: SettingsCategoryDef[];
}

/** Settings navigation — grouped by mental model:
 *  - config: how the app runs (look, models, execution limits, network/data)
 *  - agent: what the agent can do (skills, hooks, MCP, connectors, plugins)
 *  - system: observing and maintaining (usage, about) */
export const SETTINGS_GROUPS: SettingsGroup[] = [
  {
    label: "settingsGroups.config",
    items: [
      { id: "appearance", label: "settingsCategories.appearance", icon: Palette },
      { id: "models", label: "settingsCategories.models", icon: Cpu },
      { id: "vision", label: "settingsCategories.vision", icon: Eye },
      { id: "limits", label: "settingsCategories.limits", icon: Gauge },
      { id: "network", label: "settingsCategories.network", icon: Globe },
      { id: "memory", label: "settingsCategories.memory", icon: Database },
      { id: "permissions", label: "settingsCategories.permissions", icon: Shield },
    ],
  },
  {
    label: "settingsGroups.agent",
    items: [
      { id: "skills", label: "settingsCategories.skills", icon: Brain },
      { id: "agents", label: "settingsCategories.agents", icon: UserRound },
      { id: "hooks", label: "settingsCategories.hooks", icon: ListChecks },
      { id: "mcp-servers", label: "settingsCategories.mcpServers", icon: Server },
      { id: "connectors", label: "settingsCategories.connectors", icon: Plug },
      { id: "plugins", label: "settingsCategories.plugins", icon: Puzzle },
    ],
  },
  {
    label: "settingsGroups.system",
    items: [
      { id: "usage", label: "settingsCategories.usage", icon: BarChart3 },
      { id: "about", label: "settingsCategories.about", icon: Info },
    ],
  },
];

/** Flat lookup for external navigation (openSettings("models")). */
export const SETTINGS_CATEGORIES: SettingsCategoryDef[] = SETTINGS_GROUPS.flatMap(
  (g) => g.items,
);

/** First category in the first group — the settings view's landing page. */
export const DEFAULT_SETTINGS_CATEGORY: SettingsCategory = "appearance";
