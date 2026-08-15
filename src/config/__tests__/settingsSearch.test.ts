import { describe, it, expect } from "vitest";
import { zh } from "@/i18n/zh";
import { en } from "@/i18n/en";
import {
  SETTINGS_SEARCH_INDEX,
  searchSettings,
  type Translate,
} from "@/config/settingsSearch";

function resolveBundle(obj: unknown, path: string): unknown {
  let value: unknown = obj;
  for (const part of path.split(".")) {
    if (value == null || typeof value !== "object") return undefined;
    value = (value as Record<string, unknown>)[part];
  }
  return value;
}

const translateZh: Translate = (key) => {
  const value = resolveBundle(zh, key);
  return typeof value === "string" ? value : key;
};

const translateEn: Translate = (key) => {
  const value = resolveBundle(en, key);
  return typeof value === "string" ? value : key;
};

describe("settings search index", () => {
  it("resolves every indexed key in both zh and en bundles", () => {
    for (const entries of Object.values(SETTINGS_SEARCH_INDEX)) {
      for (const entry of entries) {
        for (const key of [entry.key, entry.descKey]) {
          if (!key) continue;
          expect(translateZh(key), `${key} (zh)`).not.toBe(key);
          expect(translateEn(key), `${key} (en)`).not.toBe(key);
        }
      }
    }
  });

  it("finds a setting by its Chinese label", () => {
    const results = searchSettings("主题", translateZh, translateEn);
    const appearance = results.find((r) => r.category.id === "appearance");
    expect(appearance).toBeDefined();
    expect(appearance?.entryMatches.some((m) => m.entry.key === "settings.general.theme")).toBe(true);
  });

  it("finds a technical English term even when the UI is Chinese", () => {
    const results = searchSettings("proxy", translateZh, translateEn);
    const network = results.find((r) => r.category.id === "network");
    expect(network).toBeDefined();
    expect(network?.entryMatches.some((m) => m.entry.key === "settings.general.httpProxy")).toBe(true);
  });

  it("matches text inside a description", () => {
    const results = searchSettings("重启", translateZh, translateEn);
    expect(results.some((r) => r.entryMatches.some((m) => m.entry.key === "settings.general.httpProxy"))).toBe(true);
  });

  it("matches a category name without any entry match", () => {
    const results = searchSettings("行为", translateZh, translateEn);
    const appearance = results.find((r) => r.category.id === "appearance");
    expect(appearance?.categoryMatched).toBe(true);
    expect(appearance?.entryMatches.length).toBe(0);
  });

  it("matches every category under a group name", () => {
    const results = searchSettings("智能体能力", translateZh, translateEn);
    const ids = new Set(results.map((r) => r.category.id));
    const groupIds: Array<(typeof results)[number]["category"]["id"]> = [
      "skills",
      "agents",
      "hooks",
      "mcp-servers",
      "connectors",
      "plugins",
    ];
    for (const id of groupIds) {
      expect(ids.has(id), id).toBe(true);
    }
  });

  it("is case-insensitive and ignores surrounding whitespace", () => {
    const results = searchSettings("  PROXY ", translateZh, translateEn);
    expect(results.length).toBeGreaterThan(0);
  });

  it("returns nothing for an unknown term or a blank query", () => {
    expect(searchSettings("zzzz-no-such-setting", translateZh, translateEn)).toEqual([]);
    expect(searchSettings("   ", translateZh, translateEn)).toEqual([]);
  });
});
