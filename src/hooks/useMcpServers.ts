/**
 * useMcpServers — MCP server management hook.
 *
 * Encapsulates all MCP server lifecycle logic:
 * - Load configured servers from backend on mount
 * - Track live connection status per server
 * - Connect / disconnect / add / remove operations
 * - Fetch tools from connected servers
 *
 * Components call this hook and render the returned state — no
 * `invoke()` or `mcpApi` calls leak into components.
 */

import { useState, useEffect, useCallback } from "react";
import { mcpApi, isTauri, onEvent } from "@/lib/tauri";
import type {
  McpServerConfig,
  McpServerWithStatus,
  McpStatusEvent,
  McpTransportType,
  McpTool,
  McpCredentialInput,
} from "@/types";

export interface UseMcpServersReturn {
  servers: McpServerWithStatus[];
  loading: boolean;
  error: string | null;
  /** Server names that have stored OAuth credentials. */
  credentialed: string[];
  /** Refresh the server list from backend config + connected status. */
  refresh: () => Promise<void>;
  /** Connect to an MCP server. Updates status on success/failure. */
  connect: (name: string) => Promise<void>;
  /** Disconnect from a connected MCP server. */
  disconnect: (name: string) => Promise<void>;
  /** Add a new MCP server config to the backend. */
  addServer: (config: Omit<McpServerConfig, "enabled"> & { enabled?: boolean }) => Promise<void>;
  /** Remove a server (disconnect first if connected). */
  removeServer: (name: string) => Promise<void>;
  /** Save an OAuth credential for a server (token endpoint/client id
   *  included so the backend can auto-renew expired tokens). */
  saveCredential: (server: McpServerConfig, input: McpCredentialInput) => Promise<void>;
  /** Delete the stored credential for a server. */
  deleteCredential: (server: McpServerConfig) => Promise<void>;
  /** Fetch tools for a specific server. Returns cached if already fetched. */
  fetchTools: (name: string) => Promise<McpTool[]>;
}

export function useMcpServers(): UseMcpServersReturn {
  const [servers, setServers] = useState<McpServerWithStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [credentialed, setCredentialed] = useState<string[]>([]);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [configs, connectedNames, creds] = await Promise.all([
        mcpApi.listServers(),
        mcpApi.listConnected(),
        mcpApi.listCredentials(),
      ]);

      setCredentialed(creds);

      const withStatus: McpServerWithStatus[] = configs.map((cfg) => ({
        ...cfg,
        status: connectedNames.includes(cfg.name) ? "connected" : "disconnected",
        tools: [],
        errorMessage: null,
      }));

      setServers(withStatus);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const connect = useCallback(async (name: string) => {
    setServers((prev) =>
      prev.map((s) => (s.name === name ? { ...s, status: "connecting", errorMessage: null } : s)),
    );

    try {
      const config = servers.find((s) => s.name === name);
      if (!config) return;

      const { status: _status, tools: _tools, errorMessage: _err, ...cfg } = config;
      void _status; void _tools; void _err;
      await mcpApi.connect(cfg);

      // Fetch tools after successful connection
      let tools: McpTool[] = [];
      try {
        tools = await mcpApi.getTools(name);
      } catch {
        // Tools fetch failure is non-fatal — server is still connected
      }

      setServers((prev) =>
        prev.map((s) =>
          s.name === name ? { ...s, status: "connected", tools, errorMessage: null } : s,
        ),
      );
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setServers((prev) =>
        prev.map((s) => (s.name === name ? { ...s, status: "error", errorMessage: msg } : s)),
      );
    }
  }, [servers]);

  const disconnect = useCallback(async (name: string) => {
    try {
      await mcpApi.disconnect(name);
      setServers((prev) =>
        prev.map((s) =>
          s.name === name ? { ...s, status: "disconnected", tools: [], errorMessage: null } : s,
        ),
      );
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setServers((prev) =>
        prev.map((s) => (s.name === name ? { ...s, status: "error", errorMessage: msg } : s)),
      );
    }
  }, []);

  const addServer = useCallback(
    async (config: Omit<McpServerConfig, "enabled"> & { enabled?: boolean }) => {
      if (!isTauri) return;

      const fullConfig: McpServerConfig = {
        name: config.name,
        type: config.type as McpTransportType,
        command: config.command,
        args: config.args,
        env: config.env,
        url: config.url,
        enabled: config.enabled ?? true,
      };

      // Persist FIRST, connect SECOND — a connect failure (e.g. a stdio
      // server that cannot start) must never look like a save failure.
      // The saved server shows up in the list regardless.
      try {
        await mcpApi.addServer(fullConfig);
      } catch (e: unknown) {
        setError(e instanceof Error ? e.message : String(e));
        return;
      }

      // List the saved server immediately — "saved" is a visible fact. If
      // the refresh fails, still surface the server locally so a connect
      // failure has a card to land on (otherwise it's invisible + silent).
      try {
        await refresh();
      } catch {
        setServers((prev) =>
          prev.some((s) => s.name === fullConfig.name)
            ? prev
            : [
                ...prev,
                {
                  ...fullConfig,
                  status: "disconnected",
                  tools: [],
                  errorMessage: null,
                },
              ],
        );
      }

      try {
        await mcpApi.connect(fullConfig);
        await refresh();
      } catch (e: unknown) {
        // Connect failure lands on the server card (status: error) with the
        // message — retryable, and distinct from the save result.
        const msg = e instanceof Error ? e.message : String(e);
        setServers((prev) =>
          prev.map((s) =>
            s.name === fullConfig.name ? { ...s, status: "error", errorMessage: msg } : s,
          ),
        );
      }
    },
    [refresh],
  );

  const removeServer = useCallback(
    async (name: string) => {
      // Disconnect first if connected OR still connecting — a server whose
      // connection is mid-flight must not be removed while its backend
      // process keeps running with no UI left to disconnect it.
      const server = servers.find((s) => s.name === name);
      if (server && (server.status === "connected" || server.status === "connecting")) {
        try {
          await mcpApi.disconnect(name);
        } catch {
          // Ignore disconnect errors — we're removing anyway
        }
      }

      // Remove from the persisted backend config so it does not come back
      // after a restart.
      try {
        await mcpApi.removeServer(name);
        setServers((prev) => prev.filter((s) => s.name !== name));
      } catch (e: unknown) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [servers],
  );

  const saveCredential = useCallback(
    async (server: McpServerConfig, input: McpCredentialInput) => {
      await mcpApi.saveCredential(
        server.name,
        server.url ?? "",
        input.accessToken,
        input.tokenType || "Bearer",
        input.expiresAt || undefined,
        input.refreshToken || undefined,
        input.tokenEndpoint || undefined,
        input.clientId || undefined,
      );
      setCredentialed((prev) =>
        prev.includes(server.name) ? prev : [...prev, server.name],
      );
    },
    [],
  );

  const deleteCredential = useCallback(
    async (server: McpServerConfig) => {
      await mcpApi.deleteCredential(server.name, server.url ?? "");
      setCredentialed((prev) => prev.filter((n) => n !== server.name));
    },
    [],
  );

  const fetchTools = useCallback(
    async (name: string): Promise<McpTool[]> => {
      try {
        const tools = await mcpApi.getTools(name);
        setServers((prev) =>
          prev.map((s) => (s.name === name ? { ...s, tools } : s)),
        );
        return tools;
      } catch {
        return [];
      }
    },
    [],
  );

  // Live status: the backend pushes connect / disconnect / reconnect
  // outcomes. Without this, a background drop-and-reconnect (or a failed
  // auto-reconnect) leaves the settings page showing a stale status and
  // the user never learns why the server stopped responding.
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void onEvent<McpStatusEvent>("mcp-status-changed", (payload) => {
      if (disposed) return;
      setServers((prev) =>
        prev.map((s) =>
          s.name === payload.name
            ? { ...s, status: payload.status, errorMessage: payload.error ?? null }
            : s,
        ),
      );
      if (payload.status === "connected") {
        void fetchTools(payload.name);
      }
    }).then((fn) => {
      if (disposed) fn();
      else unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [fetchTools]);

  return {
    servers,
    loading,
    error,
    credentialed,
    refresh,
    connect,
    disconnect,
    addServer,
    removeServer,
    saveCredential,
    deleteCredential,
    fetchTools,
  };
}
