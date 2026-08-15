/**
 * usePlugins — loads plugins from backend and provides
 * install/toggle operations.
 *
 * Components call this hook; they never touch connectorApi directly.
 */

import { useState, useEffect, useCallback } from "react";
import { connectorApi } from "@/lib/tauri";
import type { Plugin } from "@/types";

export interface UsePluginsResult {
  plugins: Plugin[];
  loading: boolean;
  install: (pluginId: string) => Promise<void>;
  toggle: (pluginId: string, enabled: boolean) => void;
}

export function usePlugins(): UsePluginsResult {
  const [plugins, setPlugins] = useState<Plugin[]>([]);

  useEffect(() => {
    const load = async () => {
      try {
        const data = await connectorApi.getPluginList();
        setPlugins(data);
      } catch {
        // Silently ignore — plugins feature is optional
      }
    };
    load();
  }, []);

  const install = useCallback(async (pluginId: string) => {
    try {
      await connectorApi.installPlugin(pluginId);
      setPlugins((prev) =>
        prev.map((p) =>
          p.id === pluginId ? { ...p, installed: true, enabled: true } : p,
        ),
      );
    } catch {
      // Ignore — UI stays in uninstalled state
    }
  }, []);

  const toggle = useCallback(async (pluginId: string, enabled: boolean) => {
    // Optimistic local update, rolled back if the backend rejects — but only
    // if the UI hasn't moved on to a newer toggle. Rapid double-click fires
    // two requests; without this guard a late failure would clobber the
    // latest intent with its own stale rollback.
    setPlugins((prev) =>
      prev.map((p) => (p.id === pluginId ? { ...p, enabled } : p)),
    );
    try {
      await connectorApi.togglePlugin(pluginId, enabled);
    } catch {
      setPlugins((prev) =>
        prev.map((p) =>
          p.id === pluginId && p.enabled === enabled ? { ...p, enabled: !enabled } : p,
        ),
      );
    }
  }, []);

  return {
    plugins,
    loading: false,
    install,
    toggle,
  };
}
