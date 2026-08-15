/**
 * useConfigSection — read/write one `[section]` of the backend AppConfig.
 *
 * Eliminates the repeated "getConfig → patch section → updateConfig"
 * read-modify-write dance previously duplicated across every settings
 * editor (ACP, agent limits, memory weights).
 */

import { useCallback } from "react";
import { isTauri, configApi } from "@/lib/tauri";

export function useConfigSection() {
  /** Load one section (e.g. "agent") from the backend AppConfig. */
  const load = useCallback(
    async (section: string): Promise<Record<string, unknown> | null> => {
      if (!isTauri) return null;
      try {
        const current = (await configApi.getConfig()) as Record<string, unknown>;
        return ((current[section] ?? {}) as Record<string, unknown>) ?? null;
      } catch {
        return null;
      }
    },
    [],
  );

  /** Merge `patch` into a section and write the full config back. */
  const patch = useCallback(
    async (section: string, patch: Record<string, unknown>) => {
      if (!isTauri) return;
      try {
        const current = (await configApi.getConfig()) as Record<string, unknown>;
        const prev = (current[section] ?? {}) as Record<string, unknown>;
        current[section] = { ...prev, ...patch };
        await configApi.updateConfig(current);
      } catch {
        /* ignore transient failures */
      }
    },
    [],
  );

  return { load, patch };
}
